import OSMCore
import Testing
@testable import OpenShareMouse

@Suite("Transfer menu")
struct TransferMenuTests {
    @Test func opensOnceForAnOutgoingBatchAndAgainForTheNextBatch() {
        var menu = TransferMenuPresentation()
        #expect({ !menu.update(preparing: false, transfers: []) }())
        #expect({ menu.update(preparing: true, transfers: []) }())
        #expect({ !menu.update(preparing: false, transfers: [transfer(1, .connecting)]) }())
        // Dismissing the panel must not cause progress or another batch member to reopen it.
        #expect({ !menu.update(preparing: false, transfers: [transfer(1, .transferring), transfer(2, .scanning)]) }())
        #expect({ !menu.update(preparing: false, transfers: [transfer(1, .finished), transfer(2, .failed)]) }())
        #expect({ menu.update(preparing: true, transfers: [transfer(1, .finished), transfer(2, .failed)]) }())
    }

    @Test func incomingAndInstantlyCompletedTransfersOpenWithoutLocalPreparation() {
        var menu = TransferMenuPresentation()
        #expect({ menu.update(preparing: false, transfers: [transfer(1, .transferring)]) }())
        #expect({ !menu.update(preparing: false, transfers: [transfer(1, .finished)]) }())
        #expect({ menu.update(preparing: false, transfers: [transfer(1, .finished), transfer(2, .finished)]) }())
        #expect({ !menu.update(preparing: false, transfers: [transfer(2, .finished)]) }())
        #expect({ !menu.update(preparing: false, transfers: []) }())
    }

    @Test func preparationFailureDoesNotPreventTheNextAttemptOpening() {
        var menu = TransferMenuPresentation()
        #expect({ menu.update(preparing: true, transfers: []) }())
        #expect({ !menu.update(preparing: false, transfers: []) }())
        #expect({ menu.update(preparing: true, transfers: []) }())
    }

    @Test func progressHandlesUnknownSizesOverflowAndSaveConfirmation() {
        let unknown = MenuTransferProgress(phase: .scanning, totalBytes: 0, doneBytes: 0, direction: .sending)
        #expect(unknown.fraction == nil)
        let empty = MenuTransferProgress(phase: .committing, totalBytes: 0, doneBytes: 0, direction: .receiving)
        #expect(empty.fraction == 1)
        #expect(empty.label == "正在校验保存…")
        let awaitingReceipt = MenuTransferProgress(phase: .committing, totalBytes: 100, doneBytes: 100, direction: .sending)
        #expect(awaitingReceipt.label == "等待确认保存…")
        let partial = MenuTransferProgress(phase: .transferring, totalBytes: 100, doneBytes: 25, direction: .sending)
        #expect(partial.fraction == 0.25)
        #expect(partial.label == "25%")
        let oversized = MenuTransferProgress(phase: .transferring, totalBytes: 1, doneBytes: UInt64.max, direction: .sending)
        #expect(oversized.fraction == 1)
        let failed = MenuTransferProgress(phase: .failed, totalBytes: 0, doneBytes: 0, direction: .sending)
        #expect(failed.fraction == 0)
        #expect(failed.label == "失败")
    }

    private func transfer(_ id: UInt64, _ phase: TransferPhase) -> TransferStatus {
        TransferStatus(id: id, direction: .receiving, name: "文件.txt", phase: phase,
                       totalBytes: 100, doneBytes: phase == .finished ? 100 : 0,
                       totalFiles: 1, doneFiles: phase == .finished ? 1 : 0,
                       bytesPerSecond: 0, error: nil, destination: nil)
    }
}
