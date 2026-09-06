import Foundation
import XCTest

final class CLIHelpTests: XCTestCase {
    func testHelpExitsSuccessfullyWithoutStoreOrRepositoryAccess() throws {
        let package = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let unavailableStore = root.appendingPathComponent("a-file")
        try Data("not a directory".utf8).write(to: unavailableStore)
        for arguments in [["--help"], ["run", "--help"], ["show", "--help"]] {
            let process = Process()
            process.executableURL = package.appendingPathComponent(".build/debug/breenix-runs")
            process.arguments = arguments
            process.currentDirectoryURL = root
            process.environment = ["BREENIX_RUNS_STORE": unavailableStore.path, "PATH": "/nonexistent"]
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = pipe
            try process.run()
            process.waitUntilExit()
            let output = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            XCTAssertEqual(process.terminationReason, .exit, output)
            XCTAssertEqual(process.terminationStatus, 0, output)
            for expected in ["Usage:", "Application Support/BreenixRuns", "BREENIX_RUNS_STORE",
                             "breenix-runs import <dir>", "breenix-runs show latest --messages"] {
                XCTAssertTrue(output.contains(expected), output)
            }
            XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: root.path), ["a-file"])
        }
    }
}
