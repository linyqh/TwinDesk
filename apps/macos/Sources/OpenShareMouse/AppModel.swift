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
    let build: BuildInfo = buildInfo()

    private let agent: Agent
    private var refreshTask: Task<Void, Never>?

    init() {
        let name = Host.current().localizedName ?? "Mac"
        agent = Agent(deviceName: name)
        status = agent.status()
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
    }

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
    func retryCapture() { agent.retryCapture() }

    // MARK: Presentation

    var menuBarSymbol: String {
        if !allPermissionsGranted { return "exclamationmark.triangle" }
        switch status.role {
        case .controlling, .controlled: return "computermouse.fill"
        case .local: return status.link == .connected ? "cable.connector" : "cable.connector.slash"
        }
    }

    var linkTitle: String {
        switch status.link {
        case .noCable: "未连接雷雳数据线"
        case .cableNoNetwork: "已连接数据线，网络未就绪"
        case .searching: "已连接数据线，等待对方启动 OpenShareMouse"
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
