import OSMCore
import Testing

@Suite("Rust core bindings")
struct CoreBindingTests {
    @Test func buildInfoComesFromRust() {
        let info = buildInfo()
        #expect(info.protocolMajor == 1)
        #expect(info.protocolMinor >= 2)
        #expect(info.defaultPort == 49001)
    }

    @Test func agentStartsLocal() {
        let agent = Agent(deviceName: "test", downloadsDirectory: "/tmp/osm-binding-downloads")
        let status = agent.status()
        #expect(status.role == .local)
        #expect(status.sharingEnabled)
        #expect(agent.transfers().isEmpty)
    }

    @Test func permissionReportListsCaptureAndInjection() {
        let names = Set(probePermissions().items.map(\.capability))
        #expect(names.contains("input.capture"))
        #expect(names.contains("input.keyboard.physical"))
    }
}
