import Foundation
@testable import BreenixRuns
import XCTest

final class TracesViewModelTests: XCTestCase {
    func testGateStdoutBootFactsKeepGateStdoutLineNumbers() throws {
        let serialText = try fixtureText("boot2-hard-timeout-serial-no-gate-boot-facts.txt")
        let gateStdoutText = try fixtureText("gate-boot-facts-positive.txt")
        let serialIndex = try MarkerScanner().scan(data: Data(serialText.utf8))

        let viewModel = TracesViewModel.build(serialIndex: serialIndex, gateStdoutText: gateStdoutText)

        XCTAssertEqual(serialIndex.lines.count, 805)
        XCTAssertEqual(viewModel.hostFacts.map(\.boot), [1, 20])
        XCTAssertEqual(viewModel.hostFacts.map(\.lineNumber), [2, 3])
        XCTAssertEqual(viewModel.hostFacts.map(\.sourceFile), ["gate-stdout.txt", "gate-stdout.txt"])
    }

    func testSerialBootFactsKeepSerialLineNumbers() throws {
        let serialText = """
        boot prelude
        [GATE_BOOT_FACTS:boot=7:host_ms=10-20:qemu_at_start=0:ended_by=scored_pass]
        boot epilogue
        """
        let serialIndex = try MarkerScanner().scan(data: Data(serialText.utf8))

        let viewModel = TracesViewModel.build(serialIndex: serialIndex, gateStdoutText: "")

        XCTAssertEqual(viewModel.hostFacts.map(\.boot), [7])
        XCTAssertEqual(viewModel.hostFacts[0].lineNumber, 2)
        XCTAssertEqual(viewModel.hostFacts[0].sourceFile, "serial.txt")
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
