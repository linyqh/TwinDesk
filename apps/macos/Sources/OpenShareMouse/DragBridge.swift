import AppKit
import OSMCore
import UniformTypeIdentifiers

/// AppKit side of cross-machine drag and drop (report §10.3).
///
/// The Rust engine decides when a drag crosses the screen edge; this class
/// only puts up the invisible panels it asks for:
///
/// - A *shield* parks the drag left behind on this Mac. It refuses every
///   drop, so releasing over it ends the drag without dropping anything.
/// - A *pickup* panel sits under the cursor while the engine presses the
///   button on it. That press starts a native drag of file promises, so
///   Finder, the desktop, Mail etc. take the drop as if it were local and
///   the files are fetched from the other Mac only once a target accepts.
final class DragBridge: DragOverlay, @unchecked Sendable {
    /// Set once by AppModel right after the agent exists.
    nonisolated(unsafe) weak var agent: Agent?

    // Touched only on the main thread.
    nonisolated(unsafe) private var shields: [UInt64: NSPanel] = [:]
    nonisolated(unsafe) private var pickup: PickupPanel?

    /// Promise fulfilment runs here, off the main thread: each item blocks
    /// until its transfer completes. Two at a time (report §7.3, D).
    let promiseQueue: OperationQueue = {
        let q = OperationQueue()
        q.name = "OpenShareMouse.file-promises"
        q.maxConcurrentOperationCount = 2
        q.qualityOfService = .userInitiated
        return q
    }()

    // MARK: DragOverlay (called on the engine thread)

    func showShield(request: UInt64, x: Double, y: Double) {
        DispatchQueue.main.async { [self] in
            let panel = OverlayPanel(frame: Self.screenRect(around: CGPoint(x: x, y: y), radius: 48))
            panel.contentView = ShieldView()
            panel.orderFrontRegardless()
            shields[request] = panel
            agent?.overlayReady(request: request)
        }
    }

    func hideShield(request: UInt64) {
        DispatchQueue.main.async { [self] in
            shields.removeValue(forKey: request)?.orderOut(nil)
        }
    }

    func beginProxy(request: UInt64, dragId: UInt64, items: [DragItem], x: Double, y: Double) {
        DispatchQueue.main.async { [self] in
            pickup?.close()
            let point = CGPoint(x: x, y: y)
            let screen = Self.screen(containing: point)
            let panel = PickupPanel(frame: screen?.frame ?? Self.screenRect(around: point, radius: 200),
                                    dragId: dragId)
            panel.contentView = PickupView(dragId: dragId, items: items, bridge: self)
            panel.orderFrontRegardless()
            pickup = panel
            agent?.overlayReady(request: request)
        }
    }

    func endProxy(dragId: UInt64) {
        DispatchQueue.main.async { [self] in
            if pickup?.dragId == dragId {
                pickup?.close()
                pickup = nil
            }
        }
    }

    // MARK: Called by the pickup panel

    @MainActor
    func proxyEnded(dragId: UInt64, dropped: Bool) {
        if pickup?.dragId == dragId {
            pickup?.close()
            pickup = nil
        }
        agent?.proxyEnded(dragId: dragId, dropped: dropped)
    }

    // MARK: Geometry

    /// Quartz global coordinates (origin top-left of the main display, y
    /// down) to AppKit screen coordinates (origin bottom-left, y up).
    static func appKitPoint(_ p: CGPoint) -> CGPoint {
        let mainHeight = NSScreen.screens.first?.frame.height ?? 0
        return CGPoint(x: p.x, y: mainHeight - p.y)
    }

    static func screen(containing quartz: CGPoint) -> NSScreen? {
        let p = appKitPoint(quartz)
        return NSScreen.screens.first { NSMouseInRect(p, $0.frame, false) }
            ?? NSScreen.screens.min { distance(p, $0.frame) < distance(p, $1.frame) }
    }

    private static func distance(_ p: CGPoint, _ r: CGRect) -> CGFloat {
        let dx = max(r.minX - p.x, 0, p.x - r.maxX)
        let dy = max(r.minY - p.y, 0, p.y - r.maxY)
        return dx * dx + dy * dy
    }

    static func screenRect(around quartz: CGPoint, radius: CGFloat) -> CGRect {
        let p = appKitPoint(quartz)
        return CGRect(x: p.x - radius, y: p.y - radius, width: radius * 2, height: radius * 2)
    }
}

/// Invisible panel above everything, on every Space.
@MainActor
private class OverlayPanel: NSPanel {
    init(frame: CGRect) {
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isOpaque = false
        // Fully transparent windows do not receive mouse or drag events;
        // nearly transparent ones do and still look like nothing.
        backgroundColor = NSColor.black.withAlphaComponent(0.003)
        hasShadow = false
        level = .screenSaver
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
    }
}

/// Accepts drags as a destination but refuses every drop.
private final class ShieldView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL, .URL, .string, .tiff, .png, .pdf, .rtf, .html,
                                 NSPasteboard.PasteboardType(UTType.item.identifier),
                                 NSPasteboard.PasteboardType("com.apple.NSFilePromiseItemMetaData"),
                                 NSPasteboard.PasteboardType("com.apple.pasteboard.promised-file-url")])
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    // Claiming the drag (instead of returning []) keeps the window
    // underneath from reacting while the cursor is "away" on the other Mac.
    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation { .generic }
    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation { .generic }
    override func prepareForDragOperation(_ sender: any NSDraggingInfo) -> Bool { false }
    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool { false }
}

/// Full-screen invisible panel; the engine's press on it starts the drag.
private final class PickupPanel: OverlayPanel {
    let dragId: UInt64

    init(frame: CGRect, dragId: UInt64) {
        self.dragId = dragId
        super.init(frame: frame)
    }

    override var canBecomeKey: Bool { true }
}

private final class PickupView: NSView, NSDraggingSource {
    private let dragId: UInt64
    private let items: [DragItem]
    private weak var bridge: DragBridge?
    private var started = false
    private var delegates: [PromiseDelegate] = []

    init(dragId: UInt64, items: [DragItem], bridge: DragBridge) {
        self.dragId = dragId
        self.items = items
        self.bridge = bridge
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        guard !started, let bridge else { return }
        started = true
        let origin = convert(event.locationInWindow, from: nil)
        let dragItems = items.enumerated().map { index, item -> NSDraggingItem in
            let delegate = PromiseDelegate(
                dragId: dragId,
                index: UInt32(index),
                item: item,
                bridge: bridge
            )
            delegates.append(delegate)
            let type = item.isDirectory
                ? UTType.folder
                : (UTType(filenameExtension: (item.name as NSString).pathExtension) ?? .data)
            let provider = NSFilePromiseProvider(fileType: type.identifier, delegate: delegate)
            let dragItem = NSDraggingItem(pasteboardWriter: provider)
            let icon = NSWorkspace.shared.icon(for: type)
            icon.size = NSSize(width: 64, height: 64)
            let offset = CGFloat(index) * 6
            dragItem.setDraggingFrame(
                NSRect(x: origin.x - 32 + offset, y: origin.y - 32 - offset, width: 64, height: 64),
                contents: icon
            )
            return dragItem
        }
        let session = beginDraggingSession(with: dragItems, event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = false
        session.draggingFormation = .pile
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        // Always a copy: the source stays on the other Mac (report §10.2).
        context == .outsideApplication ? .copy : []
    }

    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint,
                         operation: NSDragOperation) {
        let dragId = dragId
        // Keep the promise delegates alive until every promise is written;
        // the providers only hold them weakly.
        let keep = delegates
        bridge?.promiseQueue.addBarrierBlock { _ = keep }
        MainActor.assumeIsolated {
            bridge?.proxyEnded(dragId: dragId, dropped: operation != [])
        }
    }
}

private final class PromiseDelegate: NSObject, NSFilePromiseProviderDelegate, @unchecked Sendable {
    let dragId: UInt64
    let index: UInt32
    let item: DragItem
    weak var bridge: DragBridge?

    init(dragId: UInt64, index: UInt32, item: DragItem, bridge: DragBridge) {
        self.dragId = dragId
        self.index = index
        self.item = item
        self.bridge = bridge
    }

    func filePromiseProvider(_ provider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        item.name
    }

    func operationQueue(for provider: NSFilePromiseProvider) -> OperationQueue {
        bridge?.promiseQueue ?? OperationQueue()
    }

    func filePromiseProvider(_ provider: NSFilePromiseProvider, writePromiseTo url: URL,
                             completionHandler: @escaping ((any Error)?) -> Void) {
        guard let agent = bridge?.agent else {
            completionHandler(CocoaError(.fileWriteUnknown))
            return
        }
        do {
            let committed = try agent.fetchDroppedItem(dragId: dragId, index: index, destination: url.path)
            if committed != url.path {
                // Name was taken; tell Finder where the item really is.
                NSWorkspace.shared.noteFileSystemChanged(url.deletingLastPathComponent().path)
            }
            completionHandler(nil)
        } catch FetchError.Cancelled {
            completionHandler(CocoaError(.userCancelled))
        } catch {
            completionHandler(error)
        }
    }
}
