import Foundation
@testable import BreenixRuns
import XCTest

final class CLIReadTests: XCTestCase {
    var package: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
    }

    func invoke(_ arguments: [String], store: RunStore) throws -> (Int32, String) {
        let process = Process()
        process.executableURL = package.appendingPathComponent(".build/debug/breenix-runs")
        process.arguments = arguments
        process.environment = ProcessInfo.processInfo.environment.merging(["BREENIX_RUNS_STORE": store.root.path]) { _, new in new }
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        let output = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        process.waitUntilExit()
        XCTAssertEqual(process.terminationReason, .exit, output)
        return (process.terminationStatus, output)
    }

    func testListShowsImportedIdentityAndAppliesFilters() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let result = try Importer(store: store).importPath(package.appendingPathComponent("Tests/Fixtures/05-runtime-anti-vacuity-strict-serial.txt"))
        let id = try XCTUnwrap(result.imported.first?.id)
        let listed = try invoke(["list"], store: store)
        XCTAssertEqual(listed.0, 0, listed.1)
        XCTAssertTrue(listed.1.contains(id), listed.1)
        XCTAssertTrue(listed.1.contains("unknown\timported\tfiles-present:"), listed.1)
        XCTAssertTrue(listed.1.contains("aarch64/"), listed.1)
        let filtered = try invoke(["list", "--arch", "x86_64"], store: store)
        XCTAssertEqual(filtered.0, 0)
        XCTAssertFalse(filtered.1.contains(id), filtered.1)
        let invalid = try invoke(["list", "--verdict", "imaginary"], store: store)
        XCTAssertEqual(invalid.0, 1)
    }
}
