import Foundation
@testable import BreenixRuns
import XCTest

final class BootFactsParserTests: XCTestCase {
    func testParsesRealQuotedLinesAndPreservesUnknownExtraKey() throws {
        var text = try fixtureText("gate-boot-facts-positive.txt")
        text += "\n[GATE_BOOT_FACTS:boot=21:host_ms=1-2:qemu_cpu_s=NA:guest_uptime_ms=NA:ended_by=hard_timeout:some_new_field=7]\n"

        let records = BootFactsParser.parse(text: text)

        XCTAssertEqual(records.map(\.boot), [1, 20, 21])
        XCTAssertEqual(records[0].fields["ended_by"], "scored_pass")
        XCTAssertEqual(records[0].hostMilliseconds, BootFactsHostMilliseconds(start: 1_788_642_135_971, end: 1_788_642_146_985))
        XCTAssertEqual(records[2].fields["qemu_cpu_s"], "NA")
        XCTAssertEqual(records[2].fields["guest_uptime_ms"], "NA")
        XCTAssertEqual(records[2].fields["some_new_field"], "7")
    }

    func testAbsentFixtureReturnsNoRecords() throws {
        let text = try fixtureText("boot2-hard-timeout-serial-no-gate-boot-facts.txt")
        XCTAssertTrue(BootFactsParser.parse(text: text).isEmpty)
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
