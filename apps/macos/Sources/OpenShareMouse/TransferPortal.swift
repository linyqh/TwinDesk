import AppKit
import OSMCore

/// All coordinates here are AppKit screen points. Input routing separately
/// keeps held-button drags on their current Mac, including injected drags.
enum PortalLayout {
    static func frame(screens: [CGRect], side: ScreenSide) -> CGRect? {
        guard let screen = screens.min(by: {
            side == .left ? $0.minX < $1.minX : $0.maxX > $1.maxX
        }) else { return nil }
        let width = min(104.0, screen.width)
        let height = min(176.0, screen.height)
        return CGRect(x: side == .left ? screen.minX : screen.maxX - width,
                      y: screen.midY - height / 2, width: width, height: height)
    }
}

/// Owns one visible native drop destination, at the edge facing the peer.
/// Does not activate the app, synthesize input or create remote file promises.
@MainActor
final class TransferPortal {
    private let panel: NSPanel
    private let dropView = PortalDropView()

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
        dropView.onDrop = onDrop
    }

    func update(status: AgentStatus, preparing: Bool, activeCount: Int) {
        guard status.link == .connected, status.sharingEnabled,
              let frame = PortalLayout.frame(screens: NSScreen.screens.map(\.frame), side: status.peerSide)
        else {
            panel.orderOut(nil)
            dropView.ready = false
            return
        }
        if panel.frame != frame { panel.setFrame(frame, display: true) }
        dropView.ready = status.portalAvailable && !preparing
        dropView.side = status.peerSide
        dropView.subtitle = !status.portalAvailable ? "通道未就绪" : preparing ? "正在准备…" : activeCount > 0 ? "\(activeCount) 项传输中" : "松手发送"
        dropView.peerName = status.peer?.name ?? "对方电脑"
        dropView.needsDisplay = true
        if !panel.isVisible { panel.orderFrontRegardless() }
    }

    func showResult(_ text: String, failed: Bool) {
        dropView.feedback = (text, failed, Date().addingTimeInterval(5))
        dropView.needsDisplay = true
    }
}

@MainActor
final class PortalDropView: NSView {
    var onDrop: (([URL]) -> Void)?
    var ready = false
    var side: ScreenSide = .right
    var subtitle = "松手发送"
    var peerName = "对方电脑"
    var feedback: (text: String, failed: Bool, until: Date)?
    private var highlighted = false
    private var dropHint: String?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL])
        setAccessibilityElement(true)
        setAccessibilityLabel("文件传送通道")
        setAccessibilityHelp("将文件或文件夹拖到这里，松手后复制到对方电脑的下载目录")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var acceptsFirstResponder: Bool { false }

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
        highlighted = accepts(sender)
        dropHint = Self.fileURLs(from: sender.draggingPasteboard).count > 16 ? "每次最多 16 项" : nil
        needsDisplay = true
        return highlighted ? .copy : []
    }

    override func draggingExited(_ sender: (any NSDraggingInfo)?) {
        highlighted = false
        dropHint = nil
        needsDisplay = true
    }

    override func prepareForDragOperation(_ sender: any NSDraggingInfo) -> Bool { accepts(sender) }

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        guard accepts(sender) else { return false }
        let urls = Self.fileURLs(from: sender.draggingPasteboard)
        highlighted = false
        dropHint = nil
        needsDisplay = true
        onDrop?(urls)
        return true
    }

    override func draw(_ dirtyRect: NSRect) {
        let rect = bounds.insetBy(dx: 3, dy: 3)
        let shape = NSBezierPath(roundedRect: rect, xRadius: 16, yRadius: 16)
        NSColor.windowBackgroundColor.withAlphaComponent(0.96).setFill()
        shape.fill()
        let accent: NSColor = ready ? .systemBlue : .secondaryLabelColor
        accent.withAlphaComponent(highlighted ? 0.25 : 0.08).setFill()
        shape.fill()
        accent.withAlphaComponent(highlighted ? 1 : 0.5).setStroke()
        shape.lineWidth = highlighted ? 3 : 1
        shape.stroke()
        let symbol = side == .left ? "arrow.left.circle.fill" : "arrow.right.circle.fill"
        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            ?? NSImage(systemSymbolName: "paperplane", accessibilityDescription: nil)
        image?.draw(in: NSRect(x: bounds.midX - 16, y: bounds.maxY - 48, width: 32, height: 32))
        label("传送通道", y: bounds.maxY - 73, size: 13, color: .labelColor, bold: true)
        label(peerName, y: bounds.maxY - 94, size: 10, color: .secondaryLabelColor)
        let current = feedback.flatMap { $0.until > Date() ? $0 : nil }
        let text = highlighted ? "松手开始复制" : dropHint ?? current?.text ?? subtitle
        label(text, y: 49, size: 11, color: current?.failed == true ? .systemRed : accent)
        label("→ 对方下载目录", y: 29, size: 10, color: .secondaryLabelColor)
        label("文件 / 文件夹", y: 13, size: 9, color: .tertiaryLabelColor)
    }

    private func label(_ text: String, y: CGFloat, size: CGFloat, color: NSColor, bold: Bool = false) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center
        paragraph.lineBreakMode = .byTruncatingTail
        (text as NSString).draw(in: NSRect(x: 7, y: y, width: bounds.width - 14, height: 18),
            withAttributes: [.font: NSFont.systemFont(ofSize: size, weight: bold ? .semibold : .regular),
                             .foregroundColor: color, .paragraphStyle: paragraph])
    }
}
