import Foundation
@testable import BreenixRuns
import XCTest

final class StageCatalogStateMachineTests: XCTestCase {
    func testCommittedCatalogsLoadAndMatchTheirFileCounts() throws {
        for arch in [Arch.aarch64, .x86_64] {
            let file = try RunStore.decoder.decode(
                StageCatalogFile.self,
                from: Data(contentsOf: StageCatalog.catalogURL(for: arch))
            )
            let stages = try StageCatalog.load(for: arch)

            XCTAssertEqual(file.arch, arch)
            XCTAssertFalse(stages.isEmpty)
            XCTAssertEqual(stages.count, file.stages.count)
        }
    }

    func testGreenStrictFixtureReachesAarch64KernelBootPrefix() throws {
        let catalog = try StageCatalog.load(for: .aarch64)
        let bootCompleteIndex = try XCTUnwrap(catalog.firstIndex { $0.name == "ARM64 boot complete" })
        let kernelBootPrefix = Array(catalog[...bootCompleteIndex])
        let states = try StateMachine.evaluate(
            catalog: kernelBootPrefix,
            index: MarkerScanner().scanFile(at: fixtureURL("05-runtime-anti-vacuity-strict-serial.txt"))
        )

        XCTAssertEqual(states.map(\.stage.name).last, "ARM64 boot complete")
        XCTAssertTrue(states.allSatisfy(\.isReached))
        XCTAssertEqual(states.filter(\.isStoppedHere).count, 0)
    }

    func testPartialAarch64BootReportsExactlyOneStoppedHereStage() throws {
        let catalog = try StageCatalog.load(for: .aarch64)
        let states = try StateMachine.evaluate(
            catalog: catalog,
            index: MarkerScanner().scanFile(at: fixtureURL("testing-boot1-562-panic.txt"))
        )

        let stopped = states.filter(\.isStoppedHere)
        XCTAssertEqual(stopped.count, 1)
        XCTAssertEqual(stopped.first?.stage.name, "ARM64 boot complete")
        XCTAssertEqual(states.first { $0.stage.name == "SMP CPUs online" }?.reachedLine, 158)
        XCTAssertNil(states.first { $0.stage.name == "ARM64 boot complete" }?.reachedLine)
    }

    func testShowSubsystemsRendersStoredRunHighWaterMark() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let store = RunStore(root: root)
        let manifest = sampleManifest(id: "20260905T190000Z-aarch64-testing-show")
        let runDirectory = try store.createRunDirectory(id: manifest.id)
        try Data(contentsOf: fixtureURL("testing-boot1-562-panic.txt"))
            .write(to: runDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(manifest)

        let output = try RunShow.render(
            manifest: try store.readManifest(id: manifest.id),
            store: store,
            options: RunShowOptions(subsystems: true)
        )

        let catalog = try StageCatalog.load(for: .aarch64)
        let index = try MarkerScanner().scanFile(at: fixtureURL("testing-boot1-562-panic.txt"))
        let states = StateMachine.evaluate(catalog: catalog, index: index)
        let reached = states.filter(\.isReached).count
        let stopped = try XCTUnwrap(states.first { $0.isStoppedHere })

        XCTAssertTrue(output.contains("reached \(reached) / \(states.count)"))
        XCTAssertTrue(output.contains("stopped at #\(stopped.index)"))
        XCTAssertTrue(output.contains("ARM64 boot complete"))
        XCTAssertTrue(output.contains("SMP CPUs online"))
        XCTAssertTrue(output.contains("L158"))
        XCTAssertTrue(output.contains(stopped.stage.failureMeaning))
        XCTAssertTrue(output.contains(stopped.stage.checkHint))
    }

    private func fixtureURL(_ name: String) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name)")
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-stage-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func sampleManifest(id: String) -> RunManifest {
        RunManifest(
            id: id,
            startedAt: Date(timeIntervalSince1970: 1_788_633_600),
            endedAt: Date(timeIntervalSince1970: 1_788_633_660),
            arch: .aarch64,
            profile: "testing",
            launcher: .imported,
            kernel: KernelIdentity(buildID: "006a9bb0022747", gitSHA: "7a19f550", gitDirty: true),
            host: nil,
            verdict: .fail("fixture panic"),
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
