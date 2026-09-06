import Foundation
@testable import BreenixRuns
import XCTest

final class RunInspectorLoaderTests: XCTestCase {
    func testLoadRunsMatchesDirectStoreAndSidebarProjection() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let older = sampleManifest(
            id: "20260905T190000Z-aarch64-testing-old",
            startedAt: Date(timeIntervalSince1970: 1_788_633_600),
            verdict: .pass
        )
        let newer = sampleManifest(
            id: "20260905T193000Z-aarch64-testing-new",
            startedAt: Date(timeIntervalSince1970: 1_788_635_400),
            verdict: .fail("fixture panic")
        )
        try store.writeManifest(older)
        try store.writeManifest(newer)

        let index = try store.readIndex()
        var manifests: [RunManifest] = []
        for entry in index.runs {
            manifests.append(try store.readManifest(id: entry.id))
        }
        let expectedRows = SidebarViewModel.rows(for: manifests)
        let manifestsByID = Dictionary(uniqueKeysWithValues: manifests.map { ($0.id, $0) })
        var expectedManifests: [RunManifest] = []
        for row in expectedRows {
            expectedManifests.append(try XCTUnwrap(manifestsByID[row.id]))
        }

        let loaded = try await RunInspectorLoader.loadRuns(store: store)

        XCTAssertEqual(loaded.map(\.row), expectedRows)
        XCTAssertEqual(loaded.map(\.manifest), expectedManifests)
    }

    func testLoadDetailMatchesDirectViewModelLoadForFixtureSerial() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let serialData = try Data(contentsOf: fixtureURL("testing-boot1-562-panic.txt"))
        var manifest = sampleManifest(id: "20260905T190000Z-aarch64-testing-detail")
        manifest.serials = [SerialRef(name: "serial.txt", path: "serial.txt", bytes: serialData.count, stream: .single)]

        let runDirectory = try store.createRunDirectory(id: manifest.id)
        try serialData.write(to: runDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(manifest)
        let storedManifest = try store.readManifest(id: manifest.id)

        let expected = try RunDetailViewModel.load(manifest: storedManifest, store: store)
        let loaded = try await RunInspectorLoader.loadDetail(manifest: storedManifest, store: store)

        XCTAssertEqual(loaded, expected)
    }

    func testLoadDetailKeepsGateStdoutBootFactsLineNumbers() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let serialData = try Data(contentsOf: fixtureURL("boot2-hard-timeout-serial-no-gate-boot-facts.txt"))
        let gateStdoutData = try Data(contentsOf: fixtureURL("gate-boot-facts-positive.txt"))
        var manifest = sampleManifest(id: "20260905T200000Z-aarch64-testing-gate-stdout")
        manifest.serials = [SerialRef(name: "serial.txt", path: "serial.txt", bytes: serialData.count, stream: .single)]
        manifest.captures = [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: gateStdoutData.count)]

        let runDirectory = try store.createRunDirectory(id: manifest.id)
        try serialData.write(to: runDirectory.appendingPathComponent("serial.txt"))
        try gateStdoutData.write(to: runDirectory.appendingPathComponent("gate-stdout.txt"))
        try store.writeManifest(manifest)
        let storedManifest = try store.readManifest(id: manifest.id)

        let loaded = try await RunInspectorLoader.loadDetail(manifest: storedManifest, store: store)

        let firstHostFact = try XCTUnwrap(loaded.traces.hostFacts.first)
        XCTAssertEqual(firstHostFact.boot, 1)
        XCTAssertEqual(firstHostFact.lineNumber, 2)
        XCTAssertEqual(firstHostFact.sourceFile, "gate-stdout.txt")
    }

    func testLoadDiffMatchesDirectRunDiffCompareForFixtureSerials() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let lhs = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-lhs",
            verdict: .gateScript(command: ["gate.sh"], exitCode: 0),
            store: store
        )
        let rhs = try storeFixtureRun(
            fixtureName: "testing-boot1-562-panic.txt",
            id: "20260905T191000Z-aarch64-testing-rhs",
            verdict: .fail("fixture panic"),
            store: store
        )

        let expected = try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)
        let loaded = try await RunInspectorLoader.loadDiff(lhs: lhs, rhs: rhs, store: store)

        XCTAssertEqual(loaded, expected)
    }

    private func storeFixtureRun(
        fixtureName: String,
        id: String,
        verdict: Verdict,
        store: RunStore
    ) throws -> RunManifest {
        let data = try Data(contentsOf: fixtureURL(fixtureName))
        var manifest = sampleManifest(id: id, verdict: verdict)
        manifest.serials = [SerialRef(name: "serial.txt", path: "serial.txt", bytes: data.count, stream: .single)]
        let runDirectory = try store.createRunDirectory(id: id)
        try data.write(to: runDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(manifest)
        return try store.readManifest(id: id)
    }

    private func fixtureURL(_ name: String) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name)")
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-loader-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func sampleManifest(
        id: String,
        startedAt: Date = Date(timeIntervalSince1970: 1_788_633_600),
        verdict: Verdict = .fail("fixture panic")
    ) -> RunManifest {
        RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: startedAt.addingTimeInterval(60),
            arch: .aarch64,
            profile: "testing",
            launcher: .imported,
            kernel: KernelIdentity(buildID: "006a9bb0022747", gitSHA: "7a19f550", gitDirty: true),
            host: nil,
            verdict: verdict,
            verdictSource: .imported,
            serials: [SerialRef(name: "serial.txt", path: "serial.txt", bytes: 0, stream: .single)],
            captures: [],
            command: [],
            env: [:],
            tags: ["fixture"],
            notes: nil
        )
    }
}
