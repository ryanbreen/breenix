import Foundation
@testable import BreenixRuns
import XCTest

final class RunDiffTests: XCTestCase {
    func testFixturePairReportsSubsystemAndMarkerDeltas() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let green = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-green",
            verdict: .gateScript(command: ["gate.sh"], exitCode: 0),
            store: store
        )
        let panic = try storeFixtureRun(
            fixtureName: "testing-boot1-562-panic.txt",
            id: "20260905T191000Z-aarch64-testing-panic",
            verdict: .fail("fixture panic"),
            store: store
        )

        let result = try RunDiff.compare(lhs: green, rhs: panic, store: store)

        XCTAssertEqual(result.subsystemDelta, [
            SubsystemDeltaRow(stageName: "ARM64 boot complete", lhs: .reached(line: 571), rhs: .notReached)
        ])
        XCTAssertFalse(result.markerCountDelta.isEmpty)

        let bootTests = try XCTUnwrap(result.markerCountDelta.first { $0.family == .testBootTests })
        XCTAssertEqual(bootTests.lhsCount, 7)
        XCTAssertEqual(bootTests.rhsCount, 0)

        let heartbeats = try XCTUnwrap(result.markerCountDelta.first { $0.family == .heartbeat })
        XCTAssertEqual(heartbeats.lhsCount, 16)
        XCTAssertEqual(heartbeats.rhsCount, 0)
    }

    func testRunComparedWithItselfHasNoSubsystemOrMarkerDeltas() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let manifest = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-self",
            verdict: .unknown,
            store: store
        )

        let result = try RunDiff.compare(lhs: manifest, rhs: manifest, store: store)

        XCTAssertEqual(result.subsystemDelta, [])
        XCTAssertEqual(result.markerCountDelta, [])
    }

    func testCrossArchCompareThrowsArchMismatch() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let lhs = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-lhs",
            arch: .aarch64,
            verdict: .unknown,
            store: store
        )
        let rhs = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T191000Z-x86_64-testing-rhs",
            arch: .x86_64,
            verdict: .unknown,
            store: store
        )

        XCTAssertThrowsError(try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)) { error in
            guard case RunDiffError.archMismatch(let lhsArch, let rhsArch) = error else {
                return XCTFail("expected RunDiffError.archMismatch, got \(error)")
            }
            XCTAssertEqual(lhsArch, .aarch64)
            XCTAssertEqual(rhsArch, .x86_64)
        }
    }

    func testHostFactsDeltaIsExplicitNotSampledWhenEitherSideIsNil() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let lhs = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-no-host-lhs",
            verdict: .unknown,
            store: store
        )
        var rhs = try storeFixtureRun(
            fixtureName: "testing-boot1-562-panic.txt",
            id: "20260905T191000Z-aarch64-testing-host-rhs",
            verdict: .unknown,
            store: store
        )
        rhs.host = sampleHostTrace(startWall: 2_000, qemuPeersAarch64: 2, qemuPeersX86_64: 1, loadavg1: 3.0)
        try store.writeManifest(rhs)

        let result = try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)

        XCTAssertEqual(result.hostFactsDelta, .notSampled(lhsSampled: false, rhsSampled: true))
    }

    func testHostFactsDeltaUsesStartSamplesAcrossRuns() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        var lhs = try storeFixtureRun(
            fixtureName: "05-runtime-anti-vacuity-strict-serial.txt",
            id: "20260905T190000Z-aarch64-testing-host-lhs",
            verdict: .unknown,
            store: store
        )
        lhs.host = sampleHostTrace(startWall: 1_000, qemuPeersAarch64: 1, qemuPeersX86_64: 4, loadavg1: 1.25)
        try store.writeManifest(lhs)

        var rhs = try storeFixtureRun(
            fixtureName: "testing-boot1-562-panic.txt",
            id: "20260905T191000Z-aarch64-testing-host-rhs",
            verdict: .unknown,
            store: store
        )
        rhs.host = sampleHostTrace(startWall: 1_012.5, qemuPeersAarch64: 4, qemuPeersX86_64: 2, loadavg1: 2.0)
        try store.writeManifest(rhs)

        let result = try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)

        XCTAssertEqual(result.hostFactsDelta, .sampled(HostFactsComparison(
            startWallDeltaSeconds: 12.5,
            qemuPeersAarch64Delta: 3,
            qemuPeersX86_64Delta: -2,
            loadavg1Delta: 0.75
        )))
    }

    private func storeFixtureRun(
        fixtureName: String,
        id: String,
        arch: Arch = .aarch64,
        verdict: Verdict,
        store: RunStore
    ) throws -> RunManifest {
        let data = try Data(contentsOf: fixtureURL(fixtureName))
        var manifest = sampleManifest(id: id, arch: arch, verdict: verdict)
        manifest.serials = [SerialRef(name: "serial.txt", path: "serial.txt", bytes: data.count, stream: .single)]
        let runDirectory = try store.createRunDirectory(id: id)
        try data.write(to: runDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(manifest)
        return try store.readManifest(id: id)
    }

    private func sampleHostTrace(
        startWall: TimeInterval,
        qemuPeersAarch64: Int,
        qemuPeersX86_64: Int,
        loadavg1: Double
    ) -> HostFactsTrace {
        let start = HostFactsSample(
            wallTime: Date(timeIntervalSince1970: startWall),
            qemuPeersAarch64: qemuPeersAarch64,
            qemuPeersX86_64: qemuPeersX86_64,
            loadavg1: loadavg1
        )
        var end = start
        end.wallTime = start.wallTime.addingTimeInterval(60)
        return HostFactsTrace(start: start, end: end)
    }

    private func fixtureURL(_ name: String) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name)")
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-diff-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func sampleManifest(
        id: String,
        startedAt: Date = Date(timeIntervalSince1970: 1_788_633_600),
        arch: Arch = .aarch64,
        verdict: Verdict
    ) -> RunManifest {
        RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: startedAt.addingTimeInterval(60),
            arch: arch,
            profile: "testing",
            launcher: .imported,
            kernel: KernelIdentity(buildID: "006a9bb0022747", gitSHA: "7a19f550", gitDirty: true),
            host: nil,
            verdict: verdict,
            verdictSource: .imported,
            serials: [],
            captures: [],
            command: [],
            env: [:],
            tags: ["fixture"],
            notes: nil
        )
    }
}
