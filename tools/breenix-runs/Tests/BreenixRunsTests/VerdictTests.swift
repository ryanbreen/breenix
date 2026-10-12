import Foundation
@testable import BreenixRuns
import XCTest

/// Exercises `Verdict.isFailure` against every case -- the predicate behind
/// `show latest-fail` (RunStore.latestFailureManifest()) -- so a future case
/// added to `Verdict` without updating `isFailure` reddens here rather than
/// silently mis-classifying `latest-fail` selection.
final class VerdictTests: XCTestCase {
    func testSharedGateProjectionMatchesProvenance() {
        let verdict = Verdict.projectGateVerdict("PASS-WITH-ATTRIBUTED-LOCKUP", exitCode: 0, command: [])
        let provenance = GateProvenance(schemaVersion: 1, id: UUID().uuidString,
            arch: .aarch64, profile: "testing", verdict: "PASS-WITH-ATTRIBUTED-LOCKUP",
            exitCode: 0, startedAt: Date(), endedAt: Date(), command: [], serials: [], captures: [])
        XCTAssertEqual(verdict, .attributed("PASS-WITH-ATTRIBUTED-LOCKUP"))
        XCTAssertEqual(verdict, provenance.projectedVerdict)
        XCTAssertEqual(Verdict.projectGateVerdict("PASS", exitCode: 1, command: []), .fail("PASS"))
    }

    func testFailAttributedAndRefusedAreFailures() {
        XCTAssertTrue(Verdict.fail("boom").isFailure)
        XCTAssertTrue(Verdict.stalled("STALLED: stopped_at=2026-10-12T12:00:00Z; last progress: FAIL").isFailure)
        XCTAssertTrue(Verdict.attributed("known flake").isFailure)
        XCTAssertTrue(Verdict.refused("preflight refused").isFailure)
    }

    func testGateOutputPersistsStalledVerdictAndLastProgress() throws {
        let line = "STALLED: stopped_at=2026-10-12T12:00:00Z no test progress for 120.0s; last progress: SIGKILL_TEARDOWN_TEST: FAIL"
        let verdict = Verdict.projectGateOutput("earlier output\n" + line + "\n", verdict: "FAIL", exitCode: 124, command: [])
        XCTAssertEqual(verdict, .stalled(line))
        XCTAssertEqual(try JSONDecoder().decode(Verdict.self, from: JSONEncoder().encode(verdict)), verdict)
        XCTAssertEqual(SidebarViewModel.displayText(for: verdict), "STALLED")
    }

    func testPassRunningAndUnknownAreNotFailures() {
        XCTAssertFalse(Verdict.pass.isFailure)
        XCTAssertFalse(Verdict.running.isFailure)
        XCTAssertFalse(Verdict.unknown.isFailure)
    }

    func testGateScriptIsFailureExactlyWhenExitCodeIsNonZero() {
        XCTAssertFalse(Verdict.gateScript(command: ["gate.sh"], exitCode: 0).isFailure)
        XCTAssertTrue(Verdict.gateScript(command: ["gate.sh"], exitCode: 1).isFailure)
        // Negative exit codes (e.g. a signal-terminated process reported as
        // -N) must still read as a failure -- only exactly zero is success.
        XCTAssertTrue(Verdict.gateScript(command: ["gate.sh"], exitCode: -1).isFailure)
    }
}
