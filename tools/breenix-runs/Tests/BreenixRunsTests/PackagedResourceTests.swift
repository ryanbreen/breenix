import Foundation
@testable import BreenixRuns
import XCTest

final class PackagedResourceTests: XCTestCase {
    func testSwiftPMCatalogResolution() throws {
        XCTAssertEqual(try StageCatalog.load(for: .aarch64).first?.name, "ARM64 kernel starting")
        XCTAssertFalse(try StageCatalog.load(for: .x86_64).isEmpty)
    }

    func testPackagedExecutableResolvesItsOwnCatalogs() throws {
        let package = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let app = package.appendingPathComponent("Breenix Run Inspector.app").standardizedFileURL
        guard FileManager.default.fileExists(atPath: app.path) else {
            throw XCTSkip("Packaged app absent; run make app to exercise shipped resource resolution")
        }
        let process = Process()
        process.executableURL = app.appendingPathComponent("Contents/MacOS/BreenixRunInspector")
        process.arguments = ["--resource-probe"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        let deadline = Date().addingTimeInterval(15)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning {
            process.terminate()
            XCTFail("Packaged resource probe timed out")
        }
        process.waitUntilExit()
        let output = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        print(output)
        XCTAssertEqual(process.terminationReason, .exit, output)
        XCTAssertEqual(process.terminationStatus, 0, output)
        XCTAssertTrue(output.contains("Bundle.main=\(app.path)"), output)
        for arch in [Arch.aarch64, .x86_64] {
            XCTAssertTrue(output.contains("catalog=\(arch.rawValue) stages="), output)
            let line = try XCTUnwrap(output.split(separator: "\n").first { $0.hasPrefix("catalog=\(arch.rawValue)") })
            XCTAssertTrue(line.contains("path=\(app.path)/Contents/Resources/breenix-runs_BreenixRuns.bundle/"), output)
        }
    }
}
