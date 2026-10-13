import OSMCore
import Testing

@Suite("Rust core bindings")
struct CoreBindingTests {
    @Test func buildInfoComesFromRust() {
        let info = buildInfo()
        #expect(info.protocolMajor == 1)
        #expect(info.serviceType == "_opensharemouse._udp")
    }

    @Test func newAgentIsLocal() throws {
        #expect(try Agent().status() == .local)
    }

    @Test func permissionReportListsCaptureAndInjection() {
        let names = Set(probePermissions().items.map(\.capability))
        #expect(names.contains("input.capture"))
        #expect(names.contains("input.keyboard.physical"))
    }
}
