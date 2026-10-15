import AppKit
import OSMCore

/// All coordinates here are AppKit screen points. Input routing separately
/// keeps held-button drags on their current Mac, including injected drags.
enum PortalLayout {
    static func frame(screens: [CGRect], side: ScreenSide) -> CGRect? {
        guard let screen = screens.min(by: {
            side == .left ? $0.minX < $1.minX : $0.maxX > $1.maxX
        }) else { return nil }
        let width = min(140.0, screen.width)
        let height = min(204.0, screen.height)
        return CGRect(x: side == .left ? screen.minX : screen.maxX - width,
                      y: screen.midY - height / 2, width: width, height: height)
    }
}

/// Reports whether a file drag is under way in any app. The passive global
/// monitor needs no permission and never alters or consumes events.
@MainActor
final class FileDragWatcher {
    var onChange: ((Bool) -> Void)?
    private(set) var active = false
    private let pasteboard = NSPasteboard(name: .drag)
    private var baseline = 0
    private var monitor: Any?
    private var poll: Timer?

    init() {
        monitor = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown) { [weak self] _ in
            MainActor.assumeIsolated { self?.mouseDown() }
        }
    }

    /// A drag session rewrites the drag pasteboard when it starts; window
    /// moves and text selections leave it untouched. The source app's drag
    /// loop can swallow later mouse events, so poll while the button is held.
    private func mouseDown() {
        baseline = pasteboard.changeCount
        poll?.invalidate()
        poll = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    private func tick() {
        guard NSEvent.pressedMouseButtons & 1 != 0 else {
            poll?.invalidate()
            poll = nil
            return setActive(false)
        }
        guard !active, pasteboard.changeCount != baseline else { return }
        baseline = pasteboard.changeCount
        if pasteboard.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) {
            setActive(true)
        }
    }

    private func setActive(_ value: Bool) {
        guard value != active else { return }
        active = value
        onChange?(value)
    }
}

/// Owns one native drop destination at the edge facing the peer. It appears
/// only while a file drag is under way and briefly afterwards to show the
/// outcome. Does not activate the app, synthesize input or create remote
/// file promises.
@MainActor
final class TransferPortal {
    private let panel: NSPanel
    private let dropView = PortalDropView()
    private let watcher = FileDragWatcher()
    private var targetFrame: CGRect?
    private var side: ScreenSide = .right
    private var preparing = false
    private var lingerUntil = Date.distantPast
    private var shown = false

    init(onDrop: @escaping ([URL]) -> Void) {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                        backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        panel.hidesOnDeactivate = false
        panel.contentView = dropView
        dropView.onDrop = { [weak self] urls in
            self?.linger(seconds: 3)
            onDrop(urls)
        }
        // The drop lands just after the button is released; keep the panel
        // up long enough for it, or the panel slides out and back in.
        watcher.onChange = { [weak self] active in
            active ? self?.applyVisibility() : self?.linger(seconds: 0.5)
        }
    }

    func update(status: AgentStatus, preparing: Bool, activeCount: Int) {
        let eligible = status.link == .connected && status.sharingEnabled
        targetFrame = eligible
            ? PortalLayout.frame(screens: NSScreen.screens.map(\.frame), side: status.peerSide)
            : nil
        side = status.peerSide
        self.preparing = preparing
        dropView.update(
            ready: status.portalAvailable && !preparing,
            side: status.peerSide,
            peerName: status.peer?.name ?? "对方电脑",
            subtitle: !status.portalAvailable ? "通道未就绪"
                : preparing ? "正在准备…"
                : activeCount > 0 ? "\(activeCount) 项传输中" : "松手发送"
        )
        applyVisibility()
    }

    func showResult(_ text: String, failed: Bool) {
        dropView.showFeedback(text, failed: failed)
        if shown { linger(seconds: 2.5) }
    }

    private func linger(seconds: TimeInterval) {
        lingerUntil = max(lingerUntil, Date().addingTimeInterval(seconds))
        applyVisibility()
    }

    private func applyVisibility() {
        guard let frame = targetFrame, watcher.active || preparing || lingerUntil > Date() else {
            return setShown(false, at: panel.frame)
        }
        if shown, panel.frame != frame { panel.setFrame(frame, display: true) }
        setShown(true, at: frame)
    }

    /// Slides in from, and back out towards, the screen edge facing the peer.
    private func setShown(_ visible: Bool, at frame: CGRect) {
        guard visible != shown else { return }
        shown = visible
        let slide = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.0 : 28.0
        let offstage = frame.offsetBy(dx: side == .left ? -slide : slide, dy: 0)
        if visible {
            panel.setFrame(offstage, display: false)
            panel.alphaValue = 0
            panel.orderFrontRegardless()
        }
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = visible ? 0.24 : 0.18
            context.timingFunction = CAMediaTimingFunction(name: visible ? .easeOut : .easeIn)
            panel.animator().setFrame(visible ? frame : offstage, display: true)
            panel.animator().alphaValue = visible ? 1 : 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, !self.shown else { return }
                self.panel.orderOut(nil)
            }
        })
    }
}

@MainActor
final class PortalDropView: NSView {
    var onDrop: (([URL]) -> Void)?
    private var ready = false
    private var side: ScreenSide = .right
    private var subtitle = "松手发送"
    private var feedback: (text: String, failed: Bool, until: Date)?
    private var highlighted = false
    private var dropHint: String?

    private let background: NSView
    private let content = NSView()
    private let iconView = NSImageView()
    private let titleLabel = NSTextField(labelWithString: "传送通道")
    private let peerLabel = NSTextField(labelWithString: "对方电脑")
    private let statusLabel = NSTextField(labelWithString: "松手发送")
    private let footnoteLabel = NSTextField(labelWithString: "存入对方的下载目录")

    override init(frame frameRect: NSRect) {
        let content = self.content
        let radius: CGFloat
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            radius = 28
            glass.cornerRadius = radius
            glass.style = .regular
            glass.contentView = content
            background = glass
        } else {
            radius = 22
            let material = NSVisualEffectView()
            material.material = .hudWindow
            material.blendingMode = .behindWindow
            material.state = .active
            material.wantsLayer = true
            material.layer?.cornerRadius = radius
            material.layer?.cornerCurve = .continuous
            material.layer?.masksToBounds = true
            content.translatesAutoresizingMaskIntoConstraints = false
            material.addSubview(content)
            NSLayoutConstraint.activate([
                content.leadingAnchor.constraint(equalTo: material.leadingAnchor),
                content.trailingAnchor.constraint(equalTo: material.trailingAnchor),
                content.topAnchor.constraint(equalTo: material.topAnchor),
                content.bottomAnchor.constraint(equalTo: material.bottomAnchor),
            ])
            background = material
        }
        super.init(frame: frameRect)
        content.wantsLayer = true
        content.layer?.cornerRadius = radius
        content.layer?.cornerCurve = .continuous

        // The view itself, not just the glass, is the drop target so a cursor
        // pinned against the screen edge still lands on it.
        background.translatesAutoresizingMaskIntoConstraints = false
        addSubview(background)
        NSLayoutConstraint.activate([
            background.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            background.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            background.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            background.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4),
        ])

        titleLabel.font = .systemFont(ofSize: NSFont.systemFontSize + 1, weight: .semibold)
        peerLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        peerLabel.textColor = .secondaryLabelColor
        statusLabel.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .medium)
        footnoteLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize - 1)
        footnoteLabel.textColor = .tertiaryLabelColor
        for label in [titleLabel, peerLabel, statusLabel, footnoteLabel] {
            label.alignment = .center
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
        }

        iconView.unregisterDraggedTypes()
        let stack = NSStackView(views: [iconView, titleLabel, peerLabel, statusLabel, footnoteLabel])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 3
        stack.setCustomSpacing(12, after: iconView)
        stack.setCustomSpacing(14, after: peerLabel)
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: content.centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: content.leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor, constant: -10),
        ])

        registerForDraggedTypes([.fileURL])
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("文件传送通道")
        setAccessibilityHelp("将文件或文件夹拖到这里，松手后复制到对方电脑的下载目录")
        render()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var acceptsFirstResponder: Bool { false }

    func update(ready: Bool, side: ScreenSide, peerName: String, subtitle: String) {
        self.ready = ready
        self.side = side
        self.subtitle = subtitle
        peerLabel.stringValue = peerName
        render()
    }

    func showFeedback(_ text: String, failed: Bool) {
        feedback = (text, failed, Date().addingTimeInterval(5))
        render()
    }

    /// Restrict drops to actual file URLs; text, web URLs and promised files
    /// without a local representation cannot be sent by this transport.
    static func fileURLs(from pasteboard: NSPasteboard) -> [URL] {
        let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: options) as? [URL] ?? []
        return urls.filter(\.isFileURL)
    }

    private func accepts(_ sender: any NSDraggingInfo) -> Bool {
        guard ready, sender.draggingSourceOperationMask.contains(.copy) else { return false }
        let count = Self.fileURLs(from: sender.draggingPasteboard).count
        return count > 0 && count <= 16
    }

    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation {
        draggingUpdated(sender)
    }

    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation {
        setHighlighted(accepts(sender))
        let hint = Self.fileURLs(from: sender.draggingPasteboard).count > 16 ? "每次最多 16 项" : nil
        if hint != dropHint { dropHint = hint; render() }
        return highlighted ? .copy : []
    }

    override func draggingExited(_ sender: (any NSDraggingInfo)?) {
        dropHint = nil
        setHighlighted(false)
    }

    override func prepareForDragOperation(_ sender: any NSDraggingInfo) -> Bool { accepts(sender) }

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        guard accepts(sender) else { return false }
        let urls = Self.fileURLs(from: sender.draggingPasteboard)
        dropHint = nil
        setHighlighted(false)
        onDrop?(urls)
        return true
    }

    private func setHighlighted(_ value: Bool) {
        guard value != highlighted else { return }
        highlighted = value
        render()
        if value { iconView.addSymbolEffect(.bounce, options: .nonRepeating) }
    }

    private func render() {
        let accent: NSColor = ready ? .controlAccentColor : .secondaryLabelColor
        let current = feedback.flatMap { $0.until > Date() ? $0 : nil }
        let failed = current?.failed == true && !highlighted

        let symbol = highlighted ? "arrow.down.circle.fill"
            : side == .left ? "arrow.left.circle.fill" : "arrow.right.circle.fill"
        let config = NSImage.SymbolConfiguration(pointSize: 38, weight: .medium)
            .applying(.init(paletteColors: [.white, failed ? .systemRed : accent]))
        iconView.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(config)

        statusLabel.stringValue = highlighted ? "松手开始复制" : dropHint ?? current?.text ?? subtitle
        statusLabel.textColor = failed ? .systemRed : ready ? .controlAccentColor : .secondaryLabelColor

        // Glass tint alone is too subtle over bright content to signal a target.
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let layer = content.layer
            layer?.backgroundColor = highlighted ? NSColor.controlAccentColor.withAlphaComponent(0.14).cgColor : nil
            layer?.borderColor = NSColor.controlAccentColor.cgColor
            layer?.borderWidth = highlighted ? 2.5 : 0
        }
    }
}
