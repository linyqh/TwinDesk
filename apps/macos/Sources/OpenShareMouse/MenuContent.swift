import SwiftUI

struct MenuContent: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Text(model.statusText)

        if !model.allPermissionsGranted {
            Divider()
            Text("缺少系统权限")
            Button("授予权限…") { model.requestPermissions() }
            Button("打开“辅助功能”设置…") { model.openPrivacySettings(.accessibility) }
        }

        Divider()
        SettingsLink { Text("设置…") }
            .keyboardShortcut(",")
        Button("退出 OpenShareMouse") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}
