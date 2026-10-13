import AppKit
import Observation
import OSMCore

/// UI-facing state. Owns the Rust agent and polls cheap probes; it never
/// decides control state itself; the Rust core does (report §4.1: the UI
/// quitting or stalling must not leave the user stuck in remote control).
@MainActor
@Observable
final class AppModel {
    private(set) var status: ControlStatus = .local
    private(set) var permissions: PermissionReport = probePermissions()
    let build: BuildInfo = buildInfo()

    private let agent = Agent()
    private var refreshTask: Task<Void, Never>?

    init() {
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                self?.refresh()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    var allPermissionsGranted: Bool {
        permissions.items.allSatisfy { $0.state == .granted }
    }

    var menuBarSymbol: String {
        if !allPermissionsGranted { return "exclamationmark.triangle" }
        switch status {
        case .local: return "computermouse"
        case .paused: return "pause.circle"
        case .switching, .recovering: return "arrow.left.arrow.right"
        case .controlling, .controlled: return "computermouse.fill"
        }
    }

    var statusText: String {
        switch status {
        case .local: "本机"
        case .paused: "已暂停"
        case .switching: "正在切换…"
        case .controlling: "正在控制其他设备"
        case .controlled: "正被其他设备控制"
        case .recovering: "正在恢复本机控制…"
        }
    }

    func refresh() {
        permissions = probePermissions()
        if let s = try? agent.status() { status = s }
    }

    func requestPermissions() {
        OSMCore.requestPermissions()
        refresh()
    }

    func openPrivacySettings(_ pane: PrivacyPane) {
        NSWorkspace.shared.open(pane.url)
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
