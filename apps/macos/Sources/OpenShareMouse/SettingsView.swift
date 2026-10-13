import OSMCore
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            PermissionsPane()
                .tabItem { Label("权限", systemImage: "lock.shield") }
            AboutPane()
                .tabItem { Label("关于", systemImage: "info.circle") }
        }
        .frame(width: 520, height: 360)
    }
}

/// Shows what is missing and how to fix it, without exposing internals
/// (report §13.1).
private struct PermissionsPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Section {
                PermissionRow(
                    title: "辅助功能",
                    detail: "截获并在本机屏蔽键鼠事件，用于控制其他设备",
                    granted: model.permissions.accessibility,
                    pane: .accessibility
                )
                PermissionRow(
                    title: "输入监控",
                    detail: "监听键盘事件",
                    granted: model.permissions.inputMonitoring,
                    pane: .inputMonitoring
                )
                PermissionRow(
                    title: "发送事件",
                    detail: "在本机注入来自其他设备的键鼠输入",
                    granted: model.permissions.postEvents,
                    pane: .accessibility
                )
            } footer: {
                if model.permissions.secureInputActive {
                    Label("其他应用正在使用安全输入（例如密码框），键盘截获暂时不可用。",
                          systemImage: "exclamationmark.lock")
                        .foregroundStyle(.orange)
                }
            }

            Section {
                Button("请求缺少的权限") { model.requestPermissions() }
                Text("授权后通常需要重新启动 OpenShareMouse 才能生效。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct PermissionRow: View {
    @Environment(AppModel.self) private var model
    let title: String
    let detail: String
    let granted: Bool
    let pane: PrivacyPane

    var body: some View {
        HStack {
            Image(systemName: granted ? "checkmark.circle.fill" : "xmark.circle")
                .foregroundStyle(granted ? .green : .red)
            VStack(alignment: .leading) {
                Text(title)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if !granted {
                Button("打开设置") { model.openPrivacySettings(pane) }
            }
        }
    }
}

private struct AboutPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            LabeledContent("版本", value: model.build.version)
            LabeledContent("协议", value: "OSM/\(model.build.protocolMajor).\(model.build.protocolMinor)")
            LabeledContent("默认端口", value: String(model.build.defaultPort))
            LabeledContent("发现服务", value: model.build.serviceType)
        }
        .formStyle(.grouped)
    }
}
