import OSMCore
import SwiftUI

struct MenuContent: View {
    @Environment(AppModel.self) private var model
    let openMainWindow: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text("同桌 · TwinDesk").font(.headline)
                Spacer()
                if let appVersion {
                    Text("v\(appVersion)").font(.caption).foregroundStyle(.secondary).monospacedDigit()
                }
            }
            Text(model.linkTitle).foregroundStyle(.secondary)
            Text(model.roleTitle).foregroundStyle(.secondary)
            if !model.allPermissionsGranted || model.status.captureError != nil {
                Text("需要辅助功能权限，请打开主窗口")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if model.status.role != .local {
                Button("回到本机") { model.returnToLocal() }
            }

            if model.preparingTransfer || !model.transfers.isEmpty || model.transferError != nil {
                Divider()
                if model.preparingTransfer {
                    Text("正在准备文件…").font(.caption).foregroundStyle(.secondary)
                    ProgressView().progressViewStyle(.linear).controlSize(.small)
                }
                if !model.transfers.isEmpty {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 12) {
                            ForEach(menuTransfers, id: \.id) { transfer in
                                MenuTransferRow(transfer: transfer)
                                    .frame(height: 54, alignment: .top)
                            }
                        }
                    }
                    .frame(height: min(CGFloat(model.transfers.count) * 66 - 12, 252))
                }
                if let error = model.transferError {
                    Text(error).font(.caption).foregroundStyle(.red).lineLimit(2)
                }
            }

            Divider()
            Button("显示主窗口…", action: openMainWindow)
            Button("退出 同桌 · TwinDesk") { NSApplication.shared.terminate(nil) }
                .keyboardShortcut("q")
        }
        .buttonStyle(.plain)
        .padding(16)
        .frame(width: 320)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var appVersion: String? {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
    }

    private var menuTransfers: [TransferStatus] {
        model.activeTransfers + model.transfers.filter { $0.phase == .finished || $0.phase == .failed }
    }
}

private struct MenuTransferRow: View {
    let transfer: TransferStatus

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text("\(transfer.direction == .sending ? "发送" : "接收")：\(transfer.name)")
                    .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 4)
                Text(progress.label).monospacedDigit().foregroundStyle(.secondary).fixedSize()
            }
            .font(.caption)
            ProgressView(value: progress.fraction)
                .progressViewStyle(.linear)
                .controlSize(.small)
                .tint(transfer.phase == .failed ? .red : .accentColor)
            if let error = transfer.error, transfer.phase == .failed {
                Text(error).font(.caption2).foregroundStyle(.red).lineLimit(1)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var progress: MenuTransferProgress {
        MenuTransferProgress(phase: transfer.phase, totalBytes: transfer.totalBytes,
                             doneBytes: transfer.doneBytes, direction: transfer.direction)
    }
}

struct MenuTransferProgress {
    let phase: TransferPhase
    let totalBytes: UInt64
    let doneBytes: UInt64
    let direction: TransferDirection

    var fraction: Double? {
        if phase == .finished || phase == .committing { return 1 }
        if phase == .failed { return totalBytes > 0 ? min(1, Double(doneBytes) / Double(totalBytes)) : 0 }
        guard phase == .transferring, totalBytes > 0 else { return nil }
        return min(1, Double(doneBytes) / Double(totalBytes))
    }

    var label: String {
        switch phase {
        case .connecting: "等待连接…"
        case .scanning: "正在读取列表…"
        case .transferring: fraction.map { "\(Int($0 * 100))%" } ?? "传输中…"
        case .committing: direction == .sending ? "等待确认保存…" : "正在校验保存…"
        case .finished: "已完成"
        case .failed: "失败"
        }
    }
}
