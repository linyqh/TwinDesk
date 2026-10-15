import SwiftUI

@main
struct TwinDeskApp: App {
    @NSApplicationDelegateAdaptor(TwinDeskAppDelegate.self) private var appDelegate
    @State private var model = AppModel()
    @State private var statusBar: StatusBarController?

    var body: some Scene {
        Window("同桌 · TwinDesk", id: "main") {
            MainView()
                .environment(model)
                .background {
                    StatusBarSetup(model: model, controller: $statusBar)
                }
        }
        .windowResizability(.contentSize)
    }
}

private final class TwinDeskAppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}

private struct StatusBarSetup: View {
    @Environment(\.openWindow) private var openWindow
    let model: AppModel
    @Binding var controller: StatusBarController?

    var body: some View {
        Color.clear.frame(width: 0, height: 0)
            .onAppear {
                guard controller == nil else { return }
                controller = StatusBarController(model: model) {
                    openWindow(id: "main")
                    NSApplication.shared.activate()
                }
            }
    }
}
