import Foundation
@testable import BreenixRuns
import XCTest

final class RunStoreTests: XCTestCase {
    func testManifestWriteRoundTripsAndIndexRebuildsFromScratch() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let manifest = sampleManifest(id: "20260905T185233Z-aarch64-strict-a3f1")

        try store.writeManifest(manifest)
        let indexFromCache = try store.readIndex()
        try FileManager.default.removeItem(at: store.indexURL)
        let rebuilt = try store.rebuildIndex()

        XCTAssertEqual(try store.readManifest(id: manifest.id), manifest)
        XCTAssertEqual(rebuilt, indexFromCache)
        XCTAssertEqual(rebuilt.runs.map(\.id), [manifest.id])
    }

    func testStrayManifestTmpIsNeverSurfacedOverValidManifest() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let manifest = sampleManifest(id: "20260905T185233Z-aarch64-strict-a3f1")

        try store.writeManifest(manifest)
        let tmpURL = store.runDirectory(id: manifest.id).appendingPathComponent("manifest.json.tmp")
        try Data("{ truncated".utf8).write(to: tmpURL)

        XCTAssertEqual(try store.readManifest(id: manifest.id), manifest)
        let rebuilt = try store.rebuildIndex()
        XCTAssertEqual(rebuilt.runs.map(\.id), [manifest.id])
    }

    func testStrayManifestTmpWithoutManifestDoesNotCreateRun() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let runDir = try store.createRunDirectory(id: "20260905T185233Z-aarch64-strict-bad0")
        try Data("{ truncated".utf8).write(to: runDir.appendingPathComponent("manifest.json.tmp"))

        XCTAssertThrowsError(try store.readManifest(id: "20260905T185233Z-aarch64-strict-bad0")) { error in
            XCTAssertEqual(error as? RunStoreError, .runNotFound("20260905T185233Z-aarch64-strict-bad0"))
        }
        XCTAssertEqual(try store.rebuildIndex().runs, [])
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func sampleManifest(id: String) -> RunManifest {
        let started = Date(timeIntervalSince1970: 1_788_632_000)
        let ended = Date(timeIntervalSince1970: 1_788_632_060)
        let start = HostFactsSample(
            wallTime: started,
            qemuPeersAarch64: 1,
            qemuPeersX86_64: 0,
            loadavg1: 1.0,
            loadavg5: 2.0,
            loadavg15: 3.0,
            qemuCPUSeconds: nil,
            thermalPressure: nil,
            hostModel: "Mac16,1",
            physMem: 34_359_738_368,
            qemuVersion: "QEMU emulator version 10.0.2",
            gitSHA: "7a19f550",
            gitDirty: true
        )
        let end = HostFactsSample(
            wallTime: ended,
            qemuPeersAarch64: 1,
            qemuPeersX86_64: 0,
            loadavg1: 1.5,
            loadavg5: 2.5,
            loadavg15: 3.5,
            qemuCPUSeconds: nil,
            thermalPressure: nil,
            hostModel: "Mac16,1",
            physMem: 34_359_738_368,
            qemuVersion: "QEMU emulator version 10.0.2",
            gitSHA: "7a19f550",
            gitDirty: true
        )
        let command = ["docker/qemu/run-aarch64-boot-test-strict.sh", "1"]

        return RunManifest(
            id: id,
            startedAt: started,
            endedAt: ended,
            arch: .aarch64,
            profile: "strict",
            launcher: .localQEMU,
            kernel: KernelIdentity(buildID: "20260905185233", gitSHA: "7a19f550", gitDirty: true),
            host: HostFactsTrace(start: start, end: end),
            verdict: .gateScript(command: command, exitCode: 0),
            verdictSource: .gateScript(command: command, exitCode: 0),
            serials: [SerialRef(name: "serial.txt", path: "serial.txt", bytes: 42, stream: .single)],
            captures: [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: 128)],
            command: command,
            env: ["BREENIX_GATE_TMP": "/tmp/example"],
            tags: ["fixture"],
            notes: nil
        )
    }
}
