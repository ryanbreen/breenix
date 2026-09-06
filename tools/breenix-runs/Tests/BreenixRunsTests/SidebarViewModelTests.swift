import Foundation
@testable import BreenixRuns
import XCTest

final class SidebarViewModelTests: XCTestCase {
    func testRowsSortNewestFirstAndKeepAttributedAmber() {
        let oldPass = manifest(
            id: "20260905T110000Z-aarch64-strict-pass",
            startedAt: Date(timeIntervalSince1970: 1_788_604_400),
            profile: "strict",
            verdict: .pass,
            gitSHA: "1111111122222222"
        )
        let newestFail = manifest(
            id: "20260905T130000Z-aarch64-strict-fail",
            startedAt: Date(timeIntervalSince1970: 1_788_611_600),
            profile: "strict",
            verdict: .gateScript(command: ["gate.sh"], exitCode: 2),
            gitSHA: "3333333344444444"
        )
        let amber = manifest(
            id: "20260905T120000Z-aarch64-testing-attributed",
            startedAt: Date(timeIntervalSince1970: 1_788_608_000),
            profile: "testing",
            verdict: .attributed("#728 lockup"),
            gitSHA: "f96ea36cabcdef00"
        )

        let rows = SidebarViewModel.rows(for: [oldPass, amber, newestFail])

        XCTAssertEqual(rows.map(\.id), [
            newestFail.id,
            amber.id,
            oldPass.id
        ])
        XCTAssertEqual(rows[1].verdictState, .attributed)
        XCTAssertEqual(rows[1].verdictText, "PASS+#728")
        XCTAssertEqual(rows[1].shortSHA, "f96ea36c")
        XCTAssertEqual(rows[0].verdictState, .failure)
        XCTAssertEqual(rows[2].verdictState, .success)
    }

    func testRunningUnknownAndGateScriptDisplayStatesRemainDistinct() {
        XCTAssertEqual(SidebarViewModel.displayState(for: .running), .inFlight)
        XCTAssertEqual(SidebarViewModel.displayState(for: .unknown), .unknown)
        XCTAssertEqual(
            SidebarViewModel.displayState(for: .gateScript(command: ["gate.sh"], exitCode: 0)),
            .success
        )
        XCTAssertEqual(
            SidebarViewModel.displayState(for: .gateScript(command: ["gate.sh"], exitCode: 1)),
            .failure
        )
    }

    private func manifest(
        id: String,
        startedAt: Date,
        profile: String,
        verdict: Verdict,
        gitSHA: String?
    ) -> RunManifest {
        RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: startedAt.addingTimeInterval(60),
            arch: .aarch64,
            profile: profile,
            launcher: .imported,
            kernel: KernelIdentity(gitSHA: gitSHA),
            host: nil,
            verdict: verdict,
            verdictSource: .imported,
            serials: [],
            captures: [],
            command: [],
            env: [:],
            tags: [],
            notes: nil
        )
    }
}
