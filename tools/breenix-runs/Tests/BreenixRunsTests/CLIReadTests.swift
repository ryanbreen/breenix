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

    func testFactsDecodesSidecarInTextAndJSON() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let source = root.appendingPathComponent("original-evidence")
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
        let archive = package.appendingPathComponent("Tests/Fixtures/confirm/2026-09-06")
        // The transcript archive renamed serial.txt; reconstruct its original sidecar layout.
        for (archived, original) in [("strict-serial.txt", "serial.txt"),
                                      ("gate_boot_facts.txt", "gate_boot_facts.txt"),
                                      ("run-inspector.json", "run-inspector.json")] {
            try FileManager.default.copyItem(at: archive.appendingPathComponent(archived), to: source.appendingPathComponent(original))
        }
        let result = try Importer(store: store).importPath(source)
        XCTAssertEqual(result.imported.count, 1)
        let text = try invoke(["facts", "latest"], store: store)
        XCTAssertEqual(text.0, 0, text.1)
        XCTAssertTrue(text.1.contains("gate_boot_facts.txt:L"), text.1)
        XCTAssertTrue(text.1.contains("qemu_cpu_s="), text.1)
        XCTAssertFalse(text.1.contains("lands in"), text.1)
        let json = try invoke(["facts", "latest", "--json"], store: store)
        XCTAssertEqual(json.0, 0, json.1)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.1.utf8)) as? [String: Any])
        let records = try XCTUnwrap(object["gateRecords"] as? [[String: Any]])
        XCTAssertTrue(records.contains { $0["sourceFile"] as? String == "gate_boot_facts.txt" })
        let fields = try XCTUnwrap(records.first?["fields"] as? [String: String])
        XCTAssertNotNil(fields["qemu_cpu_s"])
        XCTAssertNotNil(object["kernel"])
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
