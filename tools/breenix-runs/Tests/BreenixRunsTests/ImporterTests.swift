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

    func testTestingProfilePreservedFailuresImportAsAarch64TestingFailures() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let failures = root.appendingPathComponent("breenix_testing_profile_failures", isDirectory: true)
        try FileManager.default.createDirectory(at: failures, withIntermediateDirectories: true)
        let serial = failures.appendingPathComponent("20260101T000000Z-boot1.txt")
        try Data("Breenix ARM64 Kernel Starting\ntesting profile failed boot\n".utf8).write(to: serial)

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(failures)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        XCTAssertEqual(manifest.arch, .aarch64)
        XCTAssertEqual(manifest.profile, "testing")
        assertFailureVerdict(manifest.verdict)
    }

    // review finding
    // breenix-runs-importer-missing-new-x86-boot-tests-failure-dir:
    // run-x86-boot-tests.sh's own failure_dir names each preserved failure
    // run `<timestamp>_<pid>` (a trailing pid suffix breenix_prod_profile_
    // failures's bare-timestamp directories, covered by
    // testProdProfileGateTmpTreeMergesFailureWithoutDuplicatingFactsCapture
    // below, do not carry) and preserves both serial_kernel.txt/
    // serial_user.txt plus capture_drain.txt, not a single serial.txt.
    func testX86BootTestsPreservedFailureImportsPidSuffixedRunWithBothSerialsAndCaptureDrain() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let failures = root.appendingPathComponent("breenix_x86_boot_tests_failures", isDirectory: true)
        let run = failures.appendingPathComponent("20260101T000000Z_54321", isDirectory: true)
        try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
        try Data("kernel log\n".utf8).write(to: run.appendingPathComponent("serial_kernel.txt"))
        try Data("user log\n".utf8).write(to: run.appendingPathComponent("serial_user.txt"))
        try Data("[CAPTURE_DRAIN:capture=partial:seq=1:edge=FAULT:cpu=0:records=-:drain_ms=300]\n".utf8)
            .write(to: run.appendingPathComponent("capture_drain.txt"))

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(failures)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        assertFailureVerdict(manifest.verdict)
        XCTAssertEqual(manifest.arch, .x86_64)
        XCTAssertEqual(manifest.profile, "boot-tests")
        XCTAssertEqual(manifest.startedAt, Date(timeIntervalSince1970: 1_767_225_600))
        XCTAssertEqual(Set(manifest.serials.map(\.name)), ["serial_kernel.txt", "serial_user.txt"])
        XCTAssertEqual(
            Set(manifest.serials.map(\.stream)),
            [SerialStream.com1, SerialStream.com2]
        )
        XCTAssertEqual(manifest.captures.map(\.name), ["capture_drain.txt"])
    }

    // Same review finding: a directory name with no numeric pid suffix (or
    // no `_` at all) must not be misread as a bare timestamp --
    // timestampFromPidSuffixedName's own guard, not just an accident of
    // Foundation's DateFormatter rejecting the extra characters. Matches
    // isProdFailureRunDirectory's own existing behavior for a directory
    // name it does not recognize: silently not a run directory (no
    // diagnostic recorded here either -- that only happens once
    // isX86BootTestsFailureRunDirectory/isProdFailureRunDirectory has
    // already accepted the name and the CONTENT inside it turns out to be
    // missing).
    func testX86BootTestsPreservedFailureSkipsRunDirectoryWithoutNumericPidSuffix() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let failures = root.appendingPathComponent("breenix_x86_boot_tests_failures", isDirectory: true)
        let run = failures.appendingPathComponent("20260101T000000Z_notapid", isDirectory: true)
        try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
        try Data("kernel log\n".utf8).write(to: run.appendingPathComponent("serial_kernel.txt"))

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(failures)

        XCTAssertEqual(result.imported.count, 0)
        XCTAssertEqual(result.skipped.count, 0)
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

    func testStrictGateTmpTreeMergesMatchingPreservedFailure() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = try makeStrictTreeWithBoot2Failure(root: root)
        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))

        let result = try Importer(store: store).importPath(fixture.gateTmp)
        let importedManifests = try manifests(for: result.imported, store: store)
        let failures = importedManifests.filter(\.verdict.isFailure)
        let unknowns = importedManifests.filter { $0.verdict == .unknown }

        XCTAssertEqual(result.imported.count, 3)
        XCTAssertEqual(result.skipped, [])
        XCTAssertEqual(unknowns.count, 2)
        let failure = try XCTUnwrap(failures.first)
        assertFailureVerdict(failure.verdict)
        XCTAssertEqual(failure.serials.map(\.name), ["20260101T000000Z-boot2.txt"])
    }

    func testProdProfileGateTmpTreeMergesFailureWithoutDuplicatingFactsCapture() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        let iteration = gateTmp.appendingPathComponent("breenix_aarch64_prod_profile", isDirectory: true)
        try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
        let serialData = Data("Breenix ARM64 Kernel Starting\nprod profile failed boot\n".utf8)
        try serialData.write(to: iteration.appendingPathComponent("serial.txt"))
        let factsData = Data("[GATE_BOOT_FACTS:boot=1:ended_by=scored_fail]\n".utf8)
        try factsData.write(to: iteration.appendingPathComponent("gate_boot_facts.txt"))

        let failureRun = gateTmp
            .appendingPathComponent("breenix_prod_profile_failures", isDirectory: true)
            .appendingPathComponent("20260101T000000Z", isDirectory: true)
        try FileManager.default.createDirectory(at: failureRun, withIntermediateDirectories: true)
        try serialData.write(to: failureRun.appendingPathComponent("serial.txt"))
        try factsData.write(to: failureRun.appendingPathComponent("gate_boot_facts.txt"))

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(gateTmp)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        assertFailureVerdict(manifest.verdict)
        XCTAssertEqual(manifest.captures.map(\.name), ["gate_boot_facts.txt"])
    }

    func testTestingProfileGateTmpTreeMergesFailureAndKeepsIterationCapture() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        let iteration = gateTmp
            .appendingPathComponent("breenix_aarch64_testing_profile", isDirectory: true)
            .appendingPathComponent("1", isDirectory: true)
        try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
        let serialData = Data("Breenix ARM64 Kernel Starting\ntesting profile failed boot\n".utf8)
        try serialData.write(to: iteration.appendingPathComponent("serial.txt"))
        try Data("qemu exited with status 1\n".utf8).write(to: iteration.appendingPathComponent("qemu-stdout.log"))

        let failures = gateTmp.appendingPathComponent("breenix_testing_profile_failures", isDirectory: true)
        try FileManager.default.createDirectory(at: failures, withIntermediateDirectories: true)
        try serialData.write(to: failures.appendingPathComponent("20260101T000000Z-boot1.txt"))

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(gateTmp)
        let manifest = try XCTUnwrap(manifests(for: result.imported, store: store).first)

        XCTAssertEqual(result.imported.count, 1)
        assertFailureVerdict(manifest.verdict)
        XCTAssertEqual(manifest.profile, "testing")
        XCTAssertTrue(manifest.captures.map(\.name).contains("qemu-stdout.log"))
    }

    func testGateTmpTreeImportsOrphanedPreservedFailureWhenSiblingIterationDoesNotMatch() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        let iteration = gateTmp.appendingPathComponent("breenix_aarch64_strict_1", isDirectory: true)
        try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
        try Data("Breenix ARM64 Kernel Starting\nnonmatching boot 1\n".utf8)
            .write(to: iteration.appendingPathComponent("serial.txt"))

        let failures = gateTmp.appendingPathComponent("breenix_aarch64_strict_failures", isDirectory: true)
        try FileManager.default.createDirectory(at: failures, withIntermediateDirectories: true)
        let orphanedFailure = failures.appendingPathComponent("20260101T000000Z-boot2.txt")
        try Data("Breenix ARM64 Kernel Starting\norphaned failed boot 2\n".utf8).write(to: orphanedFailure)

        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let result = try Importer(store: store).importPath(gateTmp)
        let importedManifests = try manifests(for: result.imported, store: store)
        let failure = try XCTUnwrap(importedManifests.first(where: \.verdict.isFailure))

        XCTAssertEqual(result.imported.count, 2)
        XCTAssertEqual(importedManifests.filter { $0.verdict == .unknown }.count, 1)
        assertFailureVerdict(failure.verdict)
        XCTAssertEqual(failure.serials.map(\.name), ["20260101T000000Z-boot2.txt"])
    }

    func testMergedFailureIDMatchesStandaloneFailureDirectoryImport() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = try makeStrictTreeWithBoot2Failure(root: root)
        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let importer = Importer(store: store)

        let treeResult = try importer.importPath(fixture.gateTmp)
        let treeManifests = try manifests(for: treeResult.imported, store: store)
        let mergedFailureID = try XCTUnwrap(treeManifests.first(where: \.verdict.isFailure)?.id)
        let failureOnlyResult = try importer.importPath(fixture.failures)
        let standaloneFailureID = try XCTUnwrap(failureOnlyResult.imported.first?.id)

        XCTAssertEqual(standaloneFailureID, mergedFailureID)
        XCTAssertEqual(try store.readIndex().runs.count, 3)
    }

    private func manifests(for imported: [ImportedRun], store: RunStore) throws -> [RunManifest] {
        try imported.map { try store.readManifest(id: $0.id) }
    }

    private func assertFailureVerdict(_ verdict: Verdict, file: StaticString = #filePath, line: UInt = #line) {
        if case .fail(let reason) = verdict {
            XCTAssertFalse(reason.isEmpty, file: file, line: line)
        } else {
            XCTFail("expected .fail verdict, got \(verdict)", file: file, line: line)
        }
    }

    private struct StrictTreeFixture {
        var gateTmp: URL
        var failures: URL
        var boot2FailureSerial: URL
        var iterations: [Int: URL]
    }

    private func makeStrictTreeWithBoot2Failure(root: URL) throws -> StrictTreeFixture {
        let gateTmp = root.appendingPathComponent("gate-tmp", isDirectory: true)
        var iterations: [Int: URL] = [:]
        var boot2SerialData = Data()

        for boot in 1...3 {
            let iteration = gateTmp.appendingPathComponent("breenix_aarch64_strict_\(boot)", isDirectory: true)
            try FileManager.default.createDirectory(at: iteration, withIntermediateDirectories: true)
            let serialData = Data("Breenix ARM64 Kernel Starting\nstrict boot \(boot)\n".utf8)
            try serialData.write(to: iteration.appendingPathComponent("serial.txt"))
            iterations[boot] = iteration
            if boot == 2 {
                boot2SerialData = serialData
            }
        }

        let failures = gateTmp.appendingPathComponent("breenix_aarch64_strict_failures", isDirectory: true)
        try FileManager.default.createDirectory(at: failures, withIntermediateDirectories: true)
        let boot2FailureSerial = failures.appendingPathComponent("20260101T000000Z-boot2.txt")
        try boot2SerialData.write(to: boot2FailureSerial)
        try Data("[GATE_BOOT_FACTS:boot=2:ended_by=scored_fail]\n".utf8)
            .write(to: failures.appendingPathComponent("20260101T000000Z-boot2.facts.txt"))

        return StrictTreeFixture(
            gateTmp: gateTmp,
            failures: failures,
            boot2FailureSerial: boot2FailureSerial,
            iterations: iterations
        )
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
