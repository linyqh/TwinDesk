import OSMCore
import SwiftUI

struct MainView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            if !model.allPermissionsGranted || model.status.captureError != nil {
                PermissionSection()
            }
            CableSection()
            PeerSection()
            ControlSection()
            TransferSection()
            if let error = model.status.lastError {
                Section {
                    Label(error, systemImage: "exclamationmark.circle")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480)
        .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: - Cable

private struct CableSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section("雷雳数据线") {
            HStack(spacing: 10) {
                Circle()
                    .fill(Color(nsColor: model.linkColor))
                    .frame(width: 10, height: 10)
                Text(model.linkTitle).font(.headline)
            }

            if let port = model.connectedPort {
                LabeledContent("端口", value: "雷雳端口 \(port.receptacle)")
                LabeledContent("速率", value: port.speed)
                if let name = port.deviceName {
                    LabeledContent(
                        "对端设备",
                        value: [name, port.deviceModel].compactMap { $0 }.joined(separator: " · ")
                    )
                }
                LabeledContent("网络服务", value: port.ipService ? "支持（雷雳网桥）" : "不支持")
            }

            LabeledContent("雷雳网桥", value: bridgeText)

            if model.status.link == .cableNoNetwork {
                Button("打开网络设置…") { model.openNetworkSettings() }
                Text("请在“网络”设置中启用“雷雳网桥”，并保持“使用 DHCP”或自动配置。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if let error = model.status.hardwareError {
                Text(error).font(.caption).foregroundStyle(.red)
            }

            DisclosureGroup("全部雷雳端口") {
                ForEach(model.status.ports, id: \.receptacle) { port in
                    LabeledContent("端口 \(port.receptacle)") {
                        Text(port.connected ? (port.deviceName ?? "已连接设备") : "未连接")
                            .foregroundStyle(port.connected ? .primary : .secondary)
                    }
                }
                if model.status.ports.isEmpty {
                    Text("未检测到雷雳端口").foregroundStyle(.secondary)
                }
            }
        }
    }

    private var bridgeText: String {
        let b = model.status.bridge
        guard b.present else { return "不存在" }
        guard b.up else { return "未启用" }
        return b.ipv4 ?? "无 IPv4 地址"
    }
}

// MARK: - Peer

private struct PeerSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section("对方电脑") {
            if let peer = model.status.peer {
                LabeledContent("名称", value: peer.name)
                LabeledContent("型号", value: peer.model)
                LabeledContent("系统", value: peer.osVersion)
                LabeledContent("地址", value: peer.address)
                if let ms = model.status.latencyMs {
                    LabeledContent("延迟", value: String(format: "%.1f ms", ms))
                }
            } else {
                Text(model.status.link == .noCable
                     ? "用雷雳数据线连接另一台 Mac 后，在两台电脑上都运行 OpenShareMouse。"
                     : "等待对方电脑上的 OpenShareMouse 响应…")
                    .foregroundStyle(.secondary)
            }
        }
    }
}

// MARK: - Control

private struct ControlSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Section("键盘与鼠标共享") {
            Toggle("启用共享", isOn: Binding(
                get: { model.status.sharingEnabled },
                set: { model.setSharingEnabled($0) }
            ))

            Picker("对方电脑位于本机的", selection: Binding(
                get: { model.status.peerSide },
                set: { model.setPeerSide($0) }
            )) {
                Text("左侧").tag(ScreenSide.left)
                Text("右侧").tag(ScreenSide.right)
            }
            .pickerStyle(.segmented)

            LabeledContent("当前状态") {
                Text(model.roleTitle)
                    .foregroundStyle(model.status.role == .local ? .secondary : Color.accentColor)
            }

            if model.status.role != .local {
                Button("回到本机") { model.returnToLocal() }
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("把鼠标移出屏幕\(model.status.peerSide == .left ? "左" : "右")边缘即可控制对方电脑，从对方屏幕的另一侧移回来即可回到本机。")
                Text("紧急情况下按 ⌃⌘⎋（Control-Command-Esc）立即回到本机。")
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }
}

// MARK: - File transfer

private struct TransferSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section("文件拖拽") {
            if model.transfers.isEmpty {
                Text("控制对方电脑时，直接把文件从本机的访达拖过屏幕边缘，放到对方的访达窗口或桌面即可复制过去；反过来也一样。文件只经雷雳数据线传输。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            ForEach(model.transfers, id: \.id) { t in
                TransferRow(transfer: t)
            }
        }
    }
}

private struct TransferRow: View {
    @Environment(AppModel.self) private var model
    let transfer: TransferStatus

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Image(systemName: transfer.direction == .sending ? "arrow.up.doc" : "arrow.down.doc")
                    .foregroundStyle(.secondary)
                Text(transfer.name).lineLimit(1).truncationMode(.middle)
                Spacer()
                switch transfer.phase {
                case .finished:
                    if transfer.destination != nil {
                        Button("在访达中显示") { model.revealTransfer(transfer) }
                            .buttonStyle(.link)
                    }
                case .failed:
                    Button("移除") { model.dismissTransfer(transfer) }.buttonStyle(.link)
                default:
                    Button {
                        model.cancelTransfer(transfer)
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                    }
                    .buttonStyle(.borderless)
                    .help("取消")
                }
            }
            if transfer.phase != .finished && transfer.phase != .failed {
                ProgressView(value: fraction)
            }
            Text(detail)
                .font(.caption)
                .foregroundStyle(transfer.phase == .failed ? .red : .secondary)
                .lineLimit(2)
        }
    }

    private var fraction: Double {
        guard transfer.totalBytes > 0 else {
            return transfer.phase == .committing ? 1 : 0
        }
        return min(1, Double(transfer.doneBytes) / Double(transfer.totalBytes))
    }

    private var detail: String {
        let bytes = ByteCountFormatter.string(fromByteCount: Int64(clamping: transfer.doneBytes), countStyle: .file)
        let total = ByteCountFormatter.string(fromByteCount: Int64(clamping: transfer.totalBytes), countStyle: .file)
        let verb = transfer.direction == .sending ? "发送" : "接收"
        switch transfer.phase {
        case .connecting: return "正在连接…"
        case .scanning: return transfer.direction == .sending ? "正在读取文件列表…" : "等待对方读取文件列表…"
        case .transferring:
            let speed = ByteCountFormatter.string(fromByteCount: Int64(transfer.bytesPerSecond), countStyle: .file)
            var text = "正在\(verb) \(bytes) / \(total) · \(speed)/秒"
            if transfer.totalFiles > 1 {
                text += " · \(transfer.doneFiles)/\(transfer.totalFiles) 个文件"
            }
            return text
        case .committing: return "正在校验并保存…"
        case .finished: return "已\(verb) \(total)"
        case .failed: return transfer.error ?? "传输失败"
        }
    }
}

// MARK: - Permissions

private struct PermissionSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section {
            Label("需要“辅助功能”权限才能共享键盘和鼠标", systemImage: "lock.shield")
                .font(.headline)
            PermissionRow(title: "辅助功能", granted: model.permissions.accessibility)
            PermissionRow(title: "发送事件", granted: model.permissions.postEvents)
            HStack {
                Button("请求权限") { model.requestPermissions() }
                Button("打开“辅助功能”设置…") { model.openPrivacySettings(.accessibility) }
                Spacer()
                Button("重试") { model.retryCapture() }
            }
            if let error = model.status.captureError {
                Text(error).font(.caption).foregroundStyle(.red)
            }
            Text("授权后点“重试”；若仍不生效，请退出并重新打开 OpenShareMouse。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

private struct PermissionRow: View {
    let title: String
    let granted: Bool

    var body: some View {
        Label(title, systemImage: granted ? "checkmark.circle.fill" : "xmark.circle")
            .foregroundStyle(granted ? .green : .red)
    }
}
