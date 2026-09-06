import Foundation
@testable import BreenixRuns
import XCTest

final class FatalRegsTests: XCTestCase {
    func testLabelledFatalRegsFixtureHasCompleteGridAndDispatchHeader() throws {
        let records = FatalRegsDecoder.decode(text: try fixtureText("fatal-regs-labelled-excerpt.txt"))

        let record = try XCTUnwrap(records.first)
        XCTAssertEqual(record.label, "INSTRUCTION_ABORT")
        XCTAssertEqual(record.cpu, 0)
        XCTAssertTrue(record.hasCompleteRegisterGrid)
        XCTAssertFalse(record.truncated)
        XCTAssertEqual(Set(record.registers.keys), Set(0...30))
        XCTAssertEqual(record.registers[0], "0xffff0000412022c0")
        XCTAssertEqual(record.registers[30], "0xffff000054243f00")
        XCTAssertEqual(record.dispatchTraceCPU, 0)
        XCTAssertEqual(record.dispatchEntries.count, 8)
        XCTAssertEqual(record.dispatchEntries.first?.path, "I")
    }

    func testUnlabelledFatalRegsFixtureDoesNotRequireDispatchHeader() throws {
        let records = FatalRegsDecoder.decode(text: try fixtureText("fatal-regs-unlabelled-excerpt.txt"))

        let record = try XCTUnwrap(records.first)
        XCTAssertNil(record.label)
        XCTAssertEqual(record.cpu, 2)
        XCTAssertTrue(record.hasCompleteRegisterGrid)
        XCTAssertFalse(record.truncated)
        XCTAssertEqual(Set(record.registers.keys), Set(0...30))
        XCTAssertEqual(record.registers[0], "0x0")
        XCTAssertEqual(record.registers[30], "0xffff000040576d5c")
        XCTAssertNil(record.dispatchTraceCPU)
        XCTAssertEqual(record.dispatchEntries.count, 8)
        XCTAssertEqual(record.dispatchEntries.first?.oldTID, 4)
        XCTAssertEqual(record.dispatchEntries.first?.newTID, 7)
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
