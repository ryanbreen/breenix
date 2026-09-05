import Foundation
@testable import BreenixRuns
import XCTest

final class ImporterTests: XCTestCase {
    func testGateTmpTreeImportsOneRunPerIterationDirectory() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let importer = Importer(store: store)

        for boot in 1...3 {
            let iteration = gateTmp.appendingPathComponent("breenix_aarch64_strict_\(boot)", isDirectory: true)
            try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
            try Data("Breenix ARM64 Kernel Starting\nboot \(boot)\n".utf8)
                .write(to: iteration.appendingPathComponent("serial.txt"))
        }

        let result = try importer.importPath(gateTmp)
        let manifests = try manifests(for: result.imported, store: store)

        XCTAssertEqual(result.imported.count, 3)
        XCTAssertEqual(result.skipped, [])
        XCTAssertEqual(Set(manifests.map(\.arch)), [.aarch64])
        XCTAssertEqual(Set(manifests.map(\.profile)), ["strict"])
        XCTAssertTrue(manifests.allSatisfy { $0.verdict == .unknown })
        XCTAssertEqual(Set(manifests.map(\.launcher)), [.imported])
    }

    func testPreservedFailuresImportRecordsFailureVerdictAndParsedTimestamp() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let failures = root.appendingPathComponent("breenix_aarch64_strict_failures", isDirectory: true)
        try FileManager.default.createDirectory(at: failures, withIntermediateDirectories: true)
        let serial = failures.appendingPathComponent("20260101T000000Z-boot1.txt")
        try Data("Breenix ARM64 Kernel Starting\nfailed boot\n".utf8).write(to: serial)
        try Data("[GATE_BOOT_FACTS:boot=1:ended_by=scored_fail]\n".utf8)
            .write(to: failures.appendingPathComponent("20260101T000000Z-boot1.facts.txt"))

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(failures)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        if case .fail(let reason) = manifest.verdict {
            XCTAssertFalse(reason.isEmpty)
        } else {
            XCTFail("preserved failure should import as .fail")
        }
        XCTAssertEqual(manifest.startedAt, Date(timeIntervalSince1970: 1_767_225_600))
        XCTAssertEqual(manifest.arch, .aarch64)
        XCTAssertEqual(manifest.profile, "strict")
        XCTAssertEqual(manifest.captures.map(\.name), ["20260101T000000Z-boot1.facts.txt"])
    }

    func testLooseSerialsDirectoryInfersArchAndStrictProfileWithoutInventingVerdict() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let serials = root.appendingPathComponent("serials", isDirectory: true)
        try FileManager.default.createDirectory(at: serials, withIntermediateDirectories: true)
        let serial = serials.appendingPathComponent("05-runtime-anti-vacuity-strict-serial.txt")
        try Data(contentsOf: fixtureURL(named: "05-runtime-anti-vacuity-strict-serial.txt")).write(to: serial)

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(serials)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        XCTAssertEqual(result.skipped, [])
        XCTAssertEqual(manifest.arch, .aarch64)
        XCTAssertEqual(manifest.profile, "strict")
        // This fixture contains passing gate markers; loose serial import must
        // still preserve the absence of a recorded source verdict as `.unknown`.
        XCTAssertEqual(manifest.verdict, .unknown)
        XCTAssertEqual(manifest.launcher, .imported)
        XCTAssertEqual(manifest.verdictSource, .imported)
    }

    func testGateTmpReimportIsIdempotent() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        for boot in 1...3 {
            let iteration = gateTmp.appendingPathComponent("breenix_aarch64_strict_\(boot)", isDirectory: true)
            try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
            try Data("Breenix ARM64 Kernel Starting\nboot \(boot)\n".utf8)
                .write(to: iteration.appendingPathComponent("serial.txt"))
        }

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let importer = Importer(store: store)
        let first = try importer.importPath(gateTmp)
        let countAfterFirst = try store.readIndex().runs.count
        let second = try importer.importPath(gateTmp)
        let countAfterSecond = try store.readIndex().runs.count

        XCTAssertEqual(first.imported.map(\.id), second.imported.map(\.id))
        XCTAssertEqual(countAfterFirst, 3)
        XCTAssertEqual(countAfterSecond, countAfterFirst)
    }

    private func manifests(for imported: [ImportedRun], store: RunStore) throws -> [RunManifest] {
        try imported.map { try store.readManifest(id: $0.id) }
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-importer-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func fixtureURL(named name: String) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name)")
    }
}
