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

        if !model.activeTransfers.isEmpty {
            Divider()
            ForEach(model.activeTransfers, id: \.id) { t in
                let pct = t.totalBytes > 0 ? Int(Double(t.doneBytes) * 100 / Double(t.totalBytes)) : 0
                Text("\(t.direction == .sending ? "发送" : "接收")：\(t.name) \(pct)%")
            }
        }

        Divider()
        Button("显示主窗口…") {
            openWindow(id: "main")
            NSApplication.shared.activate()
        }
        Button("退出 同桌 · TwinDesk") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}
