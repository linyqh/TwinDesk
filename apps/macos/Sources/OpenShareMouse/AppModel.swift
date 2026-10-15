import AppKit
import Observation
import OSMCore

/// UI-facing state. Owns the Rust agent and polls its status snapshot; it
/// never makes control decisions itself, so a stalled UI cannot leave the
/// user stuck controlling the other Mac.
@MainActor
@Observable
final class AppModel {
    private(set) var status: AgentStatus
    private(set) var permissions: PermissionReport = probePermissions()
    private(set) var transfers: [TransferStatus] = []
    let build: BuildInfo = buildInfo()

    private let agent: Agent
    private var portal: TransferPortal?
    private(set) var preparingTransfer = false
    private(set) var transferError: String?
    private(set) var isRestarting = false
    private(set) var restartError: String?
    let downloadsDirectory: URL
    private var refreshTask: Task<Void, Never>?

    init() {
        let name = Host.current().localizedName ?? "Mac"
        downloadsDirectory = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads", isDirectory: true)
        agent = Agent(deviceName: name, downloadsDirectory: downloadsDirectory.path)
        status = agent.status()
        portal = TransferPortal { [weak self] urls in self?.sendFiles(urls) }
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                self?.refresh()
                try? await Task.sleep(for: .milliseconds(300))
            }
        }
    }

    var allPermissionsGranted: Bool {
        permissions.accessibility && permissions.postEvents
    }

    func refresh() {
        let wasMissing = !allPermissionsGranted
        permissions = probePermissions()
        if wasMissing && allPermissionsGranted && status.captureError != nil {
            agent.retryCapture()
        }
        status = agent.status()
        let previouslyDone = Set(transfers.filter { $0.phase == .finished || $0.phase == .failed }.map(\.id))
        transfers = agent.transfers()
        if let recent = transfers.last(where: {
            ($0.phase == .finished || $0.phase == .failed) && !previouslyDone.contains($0.id)
        }) {
            let failed = recent.phase == .failed
            portal?.showResult(failed ? "传输失败，查看主窗口" : recent.direction == .sending ? "对方已保存" : "已存入下载目录", failed: failed)
        }
        portal?.update(status: status, preparing: preparingTransfer, activeCount: activeTransfers.count)
    }

    func sendFiles(_ urls: [URL]) {
        guard !preparingTransfer else { return }
        preparingTransfer = true
        transferError = nil
        let agent = agent
        let paths = urls.map(\.path)
        Task { [weak self] in
            let error: String? = await Task.detached {
                do { try agent.sendFiles(paths: paths); return nil }
                catch let error as SendFilesError {
                    switch error {
                    case .Failed(let message): return message
                    case .Cancelled: return "已取消"
                    }
                } catch { return error.localizedDescription }
            }.value
            guard let self else { return }
            preparingTransfer = false
            transferError = error
            portal?.showResult(error == nil ? "已提交传输" : "发送失败，查看主窗口", failed: error != nil)
            refresh()
        }
    }

    func openDownloads() { NSWorkspace.shared.open(downloadsDirectory) }

    var connectedPort: PortInfo? {
        status.ports.first { $0.connected && $0.ipService }
            ?? status.ports.first { $0.connected }
    }

    // MARK: Commands

    func requestPermissions() {
        OSMCore.requestPermissions()
        refresh()
    }

    func openPrivacySettings(_ pane: PrivacyPane) {
        NSWorkspace.shared.open(pane.url)
    }

    func openNetworkSettings() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Network-Settings.extension")!)
    }

    func setPeerSide(_ side: ScreenSide) { agent.setPeerSide(side: side) }
    func setSharingEnabled(_ on: Bool) { agent.setSharingEnabled(enabled: on) }
    func returnToLocal() { agent.returnToLocal() }
    func restartApplication() {
        guard !isRestarting else { return }
        restartError = nil
        let bundleURL = Bundle.main.bundleURL
        guard bundleURL.pathExtension == "app" else {
            restartError = "请从已安装的 同桌 · TwinDesk 应用中重启。"
            return
        }

        // Wait for this process to exit before reopening the same signed bundle.
        // A fixed delay can race shutdown and merely activate the old process.
        // Pass paths as arguments so spaces and shell characters stay literal.
        let relauncher = Process()
        relauncher.executableURL = URL(fileURLWithPath: "/bin/sh")
        relauncher.arguments = [
            "-c",
            "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open \"$2\"",
            "twindesk-relaunch",
            String(ProcessInfo.processInfo.processIdentifier),
            bundleURL.path,
        ]
        relauncher.standardInput = FileHandle.nullDevice
        relauncher.standardOutput = FileHandle.nullDevice
        relauncher.standardError = FileHandle.nullDevice
        do {
            try relauncher.run()
            isRestarting = true
            NSApplication.shared.terminate(nil)
        } catch {
            restartError = "无法自动重启，请退出后重新打开 同桌 · TwinDesk。"
        }
    }
    func cancelTransfer(_ t: TransferStatus) { agent.cancelTransfer(id: t.id) }
    func dismissTransfer(_ t: TransferStatus) { agent.dismissTransfer(id: t.id) }

    func revealTransfer(_ t: TransferStatus) {
        guard let path = t.destination else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    var activeTransfers: [TransferStatus] {
        transfers.filter { $0.phase != .finished && $0.phase != .failed }
    }

    // MARK: Presentation

    var linkTitle: String {
        switch status.link {
        case .noCable: "未连接雷雳数据线"
        case .cableNoNetwork: "已连接数据线，网络未就绪"
        case .searching: "已连接数据线，等待对方启动 同桌 · TwinDesk"
        case .connecting: "正在连接…"
        case .connected: "已连接"
        }
    }

    var linkColor: NSColor {
        switch status.link {
        case .noCable: .systemGray
        case .cableNoNetwork: .systemOrange
        case .searching, .connecting: .systemYellow
        case .connected: .systemGreen
        }
    }

    var roleTitle: String {
        switch status.role {
        case .local: "正在使用本机"
        case .controlling: "正在控制 \(peerName)"
        case .controlled: "正被 \(peerName) 控制"
        }
    }

    var peerName: String {
        status.peer?.name ?? connectedPort?.deviceName ?? "另一台电脑"
    }
}

enum PrivacyPane {
    case accessibility, inputMonitoring

    var url: URL {
        let anchor = switch self {
        case .accessibility: "Privacy_Accessibility"
        case .inputMonitoring: "Privacy_ListenEvent"
        }
        return URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)")!
    }
}
