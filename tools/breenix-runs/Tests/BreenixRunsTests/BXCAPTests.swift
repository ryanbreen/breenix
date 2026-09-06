import Foundation
@testable import BreenixRuns
import XCTest

final class BXCAPTests: XCTestCase {
    func testSynthesizedV1FixturePreservesRowsVerdictSectionsSkippedAndQuality() throws {
        let result = BXCAPDecoder.decode(text: try fixtureText("bxcap-v1-synthesized.txt"))

        XCTAssertTrue(result.refusals.isEmpty)
        let capture = try XCTUnwrap(result.captures.first)
        XCTAssertEqual(capture.seq, 1)
        XCTAssertEqual(capture.version, 1)
        XCTAssertEqual(capture.edge, "soft_lockup")
        XCTAssertEqual(capture.verdict, "partial")
        XCTAssertEqual(capture.sectionsSkipped, "0x02")
        XCTAssertFalse(capture.truncated)
        XCTAssertEqual(capture.rows.count, 8)
        XCTAssertEqual(capture.rows.first { $0.section == .cpu }?.fields["q"], "exact")
        XCTAssertEqual(capture.rows.first { $0.section == .thread }?.fields["q"], "derived")
        XCTAssertEqual(capture.rows.first { $0.section == .note }?.note, "scheduler snapshot used try_lock fallback")
    }

    func testBeginWithoutMatchingEndDecodesAsTruncated() {
        let text = """
        [BXCAP:BEGIN v=1 seq=7 edge=panic cpu=1 ts=10 tsfreq=1 uptime_ms=99 arch=aarch64 smp=4 build=0xabc]
        [BXCAP:CPU cpu=1 cur=3 prev=2 pend=0 idle=0 rq=1 nr=0 preempt=0 hardirq=0 last_sched_tick=90 silence_ms=9 sched_lock=free q=racy]
        """

        let result = BXCAPDecoder.decode(text: text)

        XCTAssertTrue(result.refusals.isEmpty)
        let capture = try! XCTUnwrap(result.captures.first)
        XCTAssertEqual(capture.seq, 7)
        XCTAssertTrue(capture.truncated)
        XCTAssertNil(capture.endLine)
        XCTAssertEqual(capture.rows.first?.fields["q"], "racy")
    }

    func testUnknownMajorVersionIsRefused() {
        let text = """
        [BXCAP:BEGIN v=2 seq=8 edge=future cpu=0 ts=1 tsfreq=1 uptime_ms=1 arch=aarch64 smp=4 build=0xabc]
        [BXCAP:CPU cpu=0 cur=1 prev=0 pend=0 idle=0 rq=0 nr=0 preempt=0 hardirq=0 last_sched_tick=1 silence_ms=0 sched_lock=free q=exact]
        [BXCAP:END v=2 seq=8 edge=future verdict=complete records=1 bytes=1 truncated=0 sections_skipped=0x0]
        """

        let result = BXCAPDecoder.decode(text: text)

        XCTAssertTrue(result.captures.isEmpty)
        let refusal = try! XCTUnwrap(result.refusals.first)
        XCTAssertEqual(refusal.seq, 8)
        XCTAssertEqual(refusal.version, 2)
        XCTAssertEqual(refusal.reason, "unsupported version")
    }

    func testInterleavedSequencesDecodeAsSeparateCaptures() {
        let text = """
        [BXCAP:BEGIN v=1 seq=1 edge=outer cpu=0 ts=1 tsfreq=1 uptime_ms=1 arch=aarch64 smp=4 build=0xabc]
        [BXCAP:CPU cpu=0 cur=1 prev=0 pend=0 idle=0 rq=1 nr=0 preempt=0 hardirq=0 last_sched_tick=1 silence_ms=0 sched_lock=free q=exact]
        [BXCAP:BEGIN v=1 seq=2 edge=inner cpu=1 ts=2 tsfreq=1 uptime_ms=2 arch=aarch64 smp=4 build=0xabc]
        [BXCAP:THR tid=9 pid=1 st=R onq=1 elr=0x1 x30=0x2 sp=0x3 wake_site=inner block_site=none dwell_ms=1 q=unavail]
        [BXCAP:END v=1 seq=2 edge=inner verdict=complete records=1 bytes=1 truncated=0 sections_skipped=0x0]
        [BXCAP:RING cpu=0 writes=1 dropped=0 span_us=1 kept=1]
        [BXCAP:END v=1 seq=1 edge=outer verdict=complete records=2 bytes=2 truncated=0 sections_skipped=0x0]
        """

        let result = BXCAPDecoder.decode(text: text)

        XCTAssertEqual(result.captures.map(\.seq), [1, 2])
        let outer = try! XCTUnwrap(result.captures.first { $0.seq == 1 })
        let inner = try! XCTUnwrap(result.captures.first { $0.seq == 2 })
        XCTAssertEqual(outer.rows.map(\.section), [.cpu, .ring])
        XCTAssertEqual(inner.rows.map(\.section), [.thread])
    }

    func testNoCapturePresentIsEmptyResult() {
        let result = BXCAPDecoder.decode(text: "ordinary serial line\n[heartbeat] tid=1 uptime_ms=2 kbd_nonzero=0\n")
        XCTAssertTrue(result.isEmpty)
    }

    private func fixtureText(_ name: String) throws -> String {
        String(decoding: try Data(contentsOf: fixtureURL(name)), as: UTF8.self)
    }

    private func fixtureURL(_ name: String) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name)")
    }
}
