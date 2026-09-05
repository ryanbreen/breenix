import Foundation
@testable import BreenixRuns
import XCTest

final class MarkerScannerTests: XCTestCase {
    func testBootTestsPassOccurrencesAndPrefixTolerance() throws {
        let index = try scanFixture()
        let passHits = index.hits.filter {
            $0.family == .testBootTests && $0.fields["state"]?.stringValue == "PASS"
        }

        XCTAssertEqual(passHits.map(\.lineNumber), [602, 610])

        let prefixLine = try XCTUnwrap(index.lines.first { $0.lineNumber == 175 })
        XCTAssertEqual(prefixLine.text, "T2T3[BOOT_TESTS:START]")

        let bootTestsStart = try XCTUnwrap(prefixLine.hits.first {
            $0.family == .testBootTests && $0.fields["state"]?.stringValue == "START"
        })
        XCTAssertEqual(bootTestsStart.lineNumber, 175)
        // A line-anchored BOOT_TESTS regex would fail this assertion because
        // scheduler trace bytes precede the bracketed marker on line 175.
        XCTAssertEqual(bootTestsStart.range.offset, prefixLine.range.offset + 4)
    }

    func testTTBR0ASIDCensusCountAndCounters() throws {
        let hits = try scanFixture().hits.filter { $0.family == .censusTTBR0ASID }

        XCTAssertEqual(hits.count, 19)

        let first = try XCTUnwrap(hits.first)
        XCTAssertEqual(first.lineNumber, 259)
        XCTAssertEqual(first.fields["untagged"]?.intValue, 0)
        XCTAssertEqual(first.fields["tagged"]?.intValue, 0)
        XCTAssertEqual(first.fields["kernel"]?.intValue, 21)
        XCTAssertEqual(first.fields["cleared"]?.intValue, 21)

        let last = try XCTUnwrap(hits.last)
        XCTAssertEqual(last.lineNumber, 914)
        XCTAssertEqual(last.fields["untagged"]?.intValue, 0)
        XCTAssertEqual(last.fields["tagged"]?.intValue, 31_351)
        XCTAssertEqual(last.fields["kernel"]?.intValue, 37_622)
        XCTAssertEqual(last.fields["cleared"]?.intValue, 68_110)
    }

    func testExecSmokeDistinctStates() throws {
        let states = Set(try scanFixture().hits.compactMap { hit -> String? in
            guard hit.family == .execSmoke else {
                return nil
            }
            return hit.fields["state"]?.stringValue
        })

        XCTAssertEqual(states, [
            "LAUNCH",
            "LAUNCHER_EXIT",
            "TARGET_ENTER",
            "TARGET_OK"
        ])
    }

    func testHeartbeatUptimeIsMonotonic() throws {
        let uptimes = try scanFixture().hits.compactMap { hit -> Int? in
            guard hit.family == .heartbeat else {
                return nil
            }
            return hit.fields["uptimeMs"]?.intValue
        }

        XCTAssertEqual(uptimes.count, 16)
        XCTAssertFalse(uptimes.isEmpty)
        XCTAssertEqual(uptimes, uptimes.sorted())
    }

    func testByteRangesSliceOriginalBuffer() throws {
        let data = try fixtureData()
        let index = try MarkerScanner().scan(data: data)

        for (lineNumber, expected) in [
            (175, "T2T3[BOOT_TESTS:START]"),
            (602, "[BOOT_TESTS:PASS]")
        ] {
            let line = try XCTUnwrap(index.lines.first { $0.lineNumber == lineNumber })
            let sliced = data.subdata(in: line.range.offset..<line.range.endOffset)
            XCTAssertEqual(String(decoding: sliced, as: UTF8.self), expected)
        }
    }

    func testBannerPinnedHomeAndStrandFamilies() throws {
        let hits = try scanFixture().hits

        let build = try XCTUnwrap(hits.first {
            $0.family == .bootBannerAarch64 && $0.fields["buildID"]?.stringValue != nil
        })
        XCTAssertEqual(build.lineNumber, 4)
        XCTAssertEqual(build.fields["buildID"]?.stringValue, "006a9c06861e6f")

        let pinned = hits.filter { $0.family == .censusPinnedHome }
        XCTAssertEqual(pinned.map(\.lineNumber), [260, 567, 568, 651, 776, 866])
        let firstCounter = try XCTUnwrap(pinned.first)
        XCTAssertEqual(firstCounter.fields["count"]?.intValue, 0)
        XCTAssertEqual(firstCounter.fields["publish_discarded"]?.intValue, 0)
        XCTAssertEqual(firstCounter.fields["hold_pen_migrated"]?.intValue, 0)
        XCTAssertEqual(firstCounter.fields["delivered"]?.intValue, 0)

        let strandNames = Set(hits.compactMap { hit -> String? in
            guard hit.family == .censusStrand else {
                return nil
            }
            return hit.fields["name"]?.stringValue
        })
        XCTAssertEqual(strandNames, [
            "SCHED_STRAND_ORACLE",
            "STRAND_INJECT_ORACLE",
            "CENSUS_WIDEN_ORACLE"
        ])
    }

    func testAnchoredKTAPPatternDoesNotMatchWithPrefixNoise() throws {
        let data = Data("T1ok 7 prefixed\nok 8 clean # SKIP\n".utf8)
        let hits = try MarkerScanner().scan(data: data).hits.filter { $0.family == .testKTAP }

        XCTAssertEqual(hits.count, 1)
        XCTAssertEqual(hits.first?.lineNumber, 2)
        XCTAssertEqual(hits.first?.fields["num"]?.intValue, 8)
        XCTAssertEqual(hits.first?.fields["disposition"]?.stringValue, "SKIP")
    }

    private func scanFixture() throws -> SerialIndex {
        try MarkerScanner().scan(data: fixtureData())
    }

    private func fixtureData() throws -> Data {
        try Data(contentsOf: fixtureURL())
    }

    private func fixtureURL() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/05-runtime-anti-vacuity-strict-serial.txt")
    }
}
