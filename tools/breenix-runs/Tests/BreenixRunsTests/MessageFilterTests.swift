import Foundation
@testable import BreenixRuns
import XCTest

final class MessageFilterTests: XCTestCase {
    func testMarkerFamiliesMapToDesignBuckets() {
        let buckets = Dictionary(grouping: MarkerFamily.allCases, by: MessageFilter.bucket(for:))

        XCTAssertEqual(Set(buckets[.boot] ?? []), [.bootStageAarch64, .bootBannerAarch64, .kernelLogX86])
        XCTAssertEqual(Set(buckets[.tests] ?? []), [.testCase, .testComplete, .testBootTests, .testKTAP, .testBTRT])
        XCTAssertEqual(Set(buckets[.oracles] ?? []), [
            .oracleGeneric,
            .oracleFutexHandoff,
            .oracleFcntlPM,
            .oracleIRQHold,
            .oraclePollTCP,
            .oracleTimerScale,
            .censusTTBR0ASID,
            .censusPinnedHome,
            .censusStrand
        ])
        XCTAssertEqual(Set(buckets[.heartbeat] ?? []), [.heartbeat])
        XCTAssertEqual(Set(buckets[.faults] ?? []), [
            .faultEL1First,
            .faultAbort,
            .faultPanic,
            .faultSoftLockup,
            .faultExt2Stall,
            .lockOrder
        ])
        XCTAssertEqual(Set(buckets[.traceNoise] ?? []), [.traceNoise])
        XCTAssertEqual(Set(buckets[.other] ?? []), [.execSmoke, .devicePCICensus])
    }

    func testLineWithoutHitMapsToOther() {
        XCTAssertEqual(MessageFilter.bucket(for: line(number: 1, family: nil, text: "plain serial")), .other)
    }

    func testFilterPredicateSelectsOnlyRequestedBuckets() {
        let lines = [
            line(number: 1, family: .bootStageAarch64, text: "[boot] Scheduler initialized"),
            line(number: 2, family: .testBootTests, text: "[BOOT_TESTS:PASS]"),
            line(number: 3, family: .oracleTimerScale, text: "[TIMER_SCALE_ORACLE:x86:PASS]"),
            line(number: 4, family: .heartbeat, text: "[heartbeat] tid=1 uptime_ms=10 kbd_nonzero=0"),
            line(number: 5, family: .faultPanic, text: "thread 'main' panicked at kernel/src/main.rs"),
            line(number: 6, family: .traceNoise, text: "T2T3"),
            line(number: 7, family: .execSmoke, text: "[EXEC_SMOKE:TARGET_OK]"),
            line(number: 8, family: nil, text: "plain serial")
        ]

        let selected = lines.filter {
            MessageFilter.includes($0, selectedBuckets: [.oracles, .faults], searchText: nil)
        }

        XCTAssertEqual(selected.map(\.lineNumber), [3, 5])
    }

    func testFilterPredicateWithEmptySelectionShowsNothing() {
        // An empty selectedBuckets means every family checkbox in the
        // Messages pane is unchecked, which must hide every line rather
        // than fall back to "no restriction" (the opposite of what an
        // all-unchecked filter bar looks like it should do).
        let lines = [
            line(number: 1, family: .bootStageAarch64, text: "[boot] Scheduler initialized"),
            line(number: 2, family: .faultPanic, text: "thread 'main' panicked at kernel/src/main.rs"),
            line(number: 3, family: nil, text: "plain serial")
        ]

        let selected = lines.filter {
            MessageFilter.includes($0, selectedBuckets: [], searchText: nil)
        }

        XCTAssertEqual(selected.map(\.lineNumber), [])
    }

    func testFilterPredicateAppliesSearchTextAfterBucketSelection() {
        let lines = [
            line(number: 1, family: .bootStageAarch64, text: "[boot] Scheduler initialized"),
            line(number: 2, family: .bootStageAarch64, text: "[boot] Timer initialized"),
            line(number: 3, family: .faultPanic, text: "Scheduler panic")
        ]

        let selected = lines.filter {
            MessageFilter.includes($0, selectedBuckets: [.boot], searchText: "scheduler")
        }

        XCTAssertEqual(selected.map(\.lineNumber), [1])
    }

    private func line(number: Int, family: MarkerFamily?, text: String) -> SerialLine {
        let range = SerialByteRange(offset: number * 100, length: text.utf8.count)
        let hits = family.map {
            [
                MarkerHit(
                    family: $0,
                    range: range,
                    fields: [:],
                    lineNumber: number
                )
            ]
        } ?? []
        return SerialLine(lineNumber: number, range: range, text: text, hits: hits)
    }
}
