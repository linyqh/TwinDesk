import AppKit
import Observation
import OSMCore
import SwiftUI

/// A native popover supports live progress and explicit presentation without
/// activating the main window or relying on private MenuBarExtra internals.
@MainActor
final class StatusBarController: NSObject {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let popover = NSPopover()
    private let model: AppModel
    private var presentation = TransferMenuPresentation()

    init(model: AppModel, openMainWindow: @escaping () -> Void) {
        self.model = model
        super.init()
        if let button = item.button {
            button.image = NSImage(systemSymbolName: "display.2", accessibilityDescription: "同桌 · TwinDesk")
            button.target = self
            button.action = #selector(toggle)
            button.toolTip = "同桌 · TwinDesk"
        }
        popover.behavior = .transient
        popover.contentViewController = NSHostingController(rootView:
            MenuContent { [weak self] in
                self?.popover.performClose(nil)
                openMainWindow()
            }.environment(model)
        )
        observeTransfers()
    }

    @objc private func toggle() {
        if popover.isShown { popover.performClose(nil) } else { show() }
    }

    private func show() {
        guard !popover.isShown, let button = item.button, button.window != nil else { return }
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }

    private func observeTransfers() {
        let shouldOpen = withObservationTracking {
            presentation.update(preparing: model.preparingTransfer, transfers: model.transfers)
        } onChange: { [weak self] in
            // Observation fires before mutation; inspect the new snapshot on
            // the next main-actor turn, then subscribe again.
            Task { @MainActor [weak self] in self?.observeTransfers() }
        }
        if shouldOpen { show() }
    }
}

/// Open once per burst of transfers. A user's dismissal survives progress
/// updates and additional files in the same batch. Even a tiny transfer that
/// finishes between polls opens the panel to display its result.
struct TransferMenuPresentation {
    private var wasBusy = false
    private var previousIDs: Set<UInt64> = []

    mutating func update(preparing: Bool, transfers: [TransferStatus]) -> Bool {
        let ids = Set(transfers.map(\.id))
        let busy = preparing || transfers.contains { $0.phase != .finished && $0.phase != .failed }
        let shouldOpen = !wasBusy && (busy || !ids.subtracting(previousIDs).isEmpty)
        wasBusy = busy
        previousIDs = ids
        return shouldOpen
    }
}
