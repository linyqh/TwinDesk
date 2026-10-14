import AppKit
import OSMCore
import Testing
@testable import OpenShareMouse

@Suite("File portal", .serialized)
@MainActor
struct TransferPortalTests {
    @Test func portalUsesFacingOuterEdgeWithOffsetDisplays() {
        let main = CGRect(x: 0, y: 0, width: 1440, height: 900)
        let left = CGRect(x: -1920, y: -200, width: 1920, height: 1080)
        let right = CGRect(x: 1440, y: 300, width: 1920, height: 1080)
        let leftPortal = PortalLayout.frame(screens: [main, left, right], side: .left)!
        let rightPortal = PortalLayout.frame(screens: [main, left, right], side: .right)!
        #expect(leftPortal.minX == left.minX)
        #expect(leftPortal.midY == left.midY)
        #expect(rightPortal.maxX == right.maxX)
        #expect(rightPortal.midY == right.midY)
        #expect(left.contains(leftPortal))
        #expect(right.contains(rightPortal))
        #expect(PortalLayout.frame(screens: [], side: .left) == nil)
    }

    @Test func nativePasteboardAcceptsMultipleFileURLsButNotTextOrWebURLs() {
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        let urls = [URL(fileURLWithPath: "/tmp/文件 1.txt"), URL(fileURLWithPath: "/tmp/folder")]
        pasteboard.writeObjects(urls as [NSURL])
        #expect(PortalDropView.fileURLs(from: pasteboard) == urls)
        pasteboard.clearContents()
        pasteboard.writeObjects([URL(string: "https://example.com/file")! as NSURL])
        #expect(PortalDropView.fileURLs(from: pasteboard).isEmpty)
        pasteboard.clearContents()
        pasteboard.setString("/tmp/not-an-actual-file-url", forType: .string)
        #expect(PortalDropView.fileURLs(from: pasteboard).isEmpty)
    }
}
