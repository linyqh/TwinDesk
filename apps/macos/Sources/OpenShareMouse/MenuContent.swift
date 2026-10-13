import SwiftUI

struct MenuContent: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text(model.linkTitle)
        Text(model.roleTitle)

        if model.status.role != .local {
            Button("回到本机") { model.returnToLocal() }
        }

        Divider()
        Button("显示主窗口…") {
            openWindow(id: "main")
            NSApplication.shared.activate()
        }
        Button("退出 OpenShareMouse") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}
