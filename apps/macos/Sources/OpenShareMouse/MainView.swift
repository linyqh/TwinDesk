import OSMCore
import SwiftUI

struct MainView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            StatusHeader()
            if !model.allPermissionsGranted || model.status.captureError != nil {
                PermissionSection()
            }
            ControlSection()
            TransferSection()
            if let error = model.status.lastError {
                Section {
                    Label(error, systemImage: "exclamationmark.circle")
                        .foregroundStyle(.secondary)
                        .wrapping()
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: - Header

private struct StatusHeader: View {
    @Environment(AppModel.self) private var model
    @State private var showingDetails = false

    var body: some View {
        Section {
            HStack(spacing: 12) {
                BrandIcon(size: 44)
                VStack(alignment: .leading, spacing: 3) {
                    Text("同桌 · TwinDesk").font(.headline)
                    HStack(spacing: 6) {
                        Image(systemName: "circle.fill")
                            .font(.system(size: 7))
                            .foregroundStyle(Color(nsColor: model.linkColor))
                        Text(summary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .layoutPriority(-1)
                    }
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if model.status.link == .cableNoNetwork {
                    Button("网络设置…") { model.openNetworkSettings() }
                }
                Button {
                    showingDetails.toggle()
                } label: {
                    Image(systemName: "info.circle").font(.title3)
                }
                .buttonStyle(.borderless)
                .help("连接详情")
                .popover(isPresented: $showingDetails, arrowEdge: .bottom) {
                    Form {
                        CableSection()
                        PeerSection()
                    }
                    .formStyle(.grouped)
                    .frame(width: 360)
                    .fixedSize(horizontal: false, vertical: true)
                    .environment(model)
                }
            }
        }
    }

    private var summary: String {
        guard model.status.link == .connected, let peer = model.status.peer else { return model.linkTitle }
        guard let ms = model.status.latencyMs else { return "已连接 \(peer.name)" }
        return "已连接 \(peer.name) · " + String(format: "%.1f ms", ms)
    }
}

// MARK: - Cable

private struct CableSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section("雷雳数据线") {
            Label {
                Text(model.linkTitle).font(.headline)
            } icon: {
                Image(systemName: "circle.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(Color(nsColor: model.linkColor))
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
                Text(error).font(.caption).foregroundStyle(.red).wrapping()
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
                     ? "用雷雳数据线连接另一台 Mac 后，在两台电脑上都运行 同桌 · TwinDesk。"
                     : "等待对方电脑上的 同桌 · TwinDesk 响应…")
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
        Section {
            Toggle("启用共享", isOn: Binding(
                get: { model.status.sharingEnabled },
                set: { model.setSharingEnabled($0) }
            ))

            Picker("对方电脑位置", selection: Binding(
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
        } header: {
            Text("键盘与鼠标共享")
        } footer: {
            Text("鼠标移出屏幕\(model.status.peerSide == .left ? "左" : "右")边缘即控制对方；按 ⌃⌘⎋ 立即回到本机。")
                .formFooterStyle()
        }
    }
}

// MARK: - File transfer

private struct TransferSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section {
            LabeledContent("接收位置") {
                Button("下载目录", systemImage: "folder") { model.openDownloads() }
            }
            if model.status.link == .connected && !model.status.portalAvailable {
                Label("传送通道尚未就绪，请确认两端均已更新且共享已开启。", systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .wrapping()
            }
            if let error = model.transferError {
                Label(error, systemImage: "xmark.octagon.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .wrapping()
            }
            ForEach(model.transfers, id: \.id) { t in
                TransferRow(transfer: t)
            }
        } header: {
            Text("文件传送")
        } footer: {
            Text("拖动文件时，屏幕\(model.status.peerSide == .left ? "左" : "右")侧会出现传送通道，松手即发送。每次最多 16 项。")
                .formFooterStyle()
        }
    }
}

private struct TransferRow: View {
    @Environment(AppModel.self) private var model
    let transfer: TransferStatus

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Image(systemName: icon)
                    .foregroundStyle(transfer.phase == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                    .contentTransition(.symbolEffect(.replace))
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
                    .controlSize(.small)
            }
            Text(detail)
                .font(.caption)
                .foregroundStyle(transfer.phase == .failed ? .red : .secondary)
                .lineLimit(2)
        }
    }

    private var icon: String {
        switch transfer.phase {
        case .finished: "checkmark.circle.fill"
        case .failed: "exclamationmark.circle.fill"
        default: transfer.direction == .sending ? "arrow.up.doc" : "arrow.down.doc"
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
        case .committing: return transfer.direction == .sending ? "等待对方确认保存…" : "正在校验并保存…"
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
            VStack(alignment: .leading, spacing: 6) {
                Label("需要“辅助功能”权限", systemImage: "exclamationmark.triangle.fill")
                    .font(.headline)
                    .foregroundStyle(.orange)
                Text(model.status.captureError ?? "授权后点“重试”；若仍不生效，请重新打开 同桌 · TwinDesk。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .wrapping()
                HStack {
                    Button("打开设置…") {
                        model.requestPermissions()
                        model.openPrivacySettings(.accessibility)
                    }
                    .buttonStyle(.borderedProminent)
                    Button("重试") { model.retryCapture() }
                }
                .controlSize(.small)
                .padding(.top, 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private extension View {
    func formFooterStyle() -> some View {
        font(.caption)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.leading)
            .wrapping()
            .padding(.horizontal, 10)
    }

    /// A grouped Form sizes itself to its widest row's ideal width, so long
    /// text must report a modest ideal width to wrap instead of widening it.
    func wrapping() -> some View {
        fixedSize(horizontal: false, vertical: true)
            .frame(idealWidth: 280, maxWidth: .infinity, alignment: .leading)
    }
}
