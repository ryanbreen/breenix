import Foundation
@testable import BreenixRuns
import XCTest

final class StageCatalogStateMachineTests: XCTestCase {
    func testCommittedCatalogsLoadAndMatchTheirFileCounts() throws {
        for arch in [Arch.aarch64, .x86_64] {
            let url = try XCTUnwrap(StageCatalog.catalogURL(for: arch))
            let file = try RunStore.decoder.decode(
                StageCatalogFile.self,
                from: Data(contentsOf: url)
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

    // `String(format:)`'s `%-N@` width flag is silently ignored for NSString
    // substitutions on this Foundation, so a prior version of this table
    // rendered with zero column alignment even though every assertion here
    // (and previously, only substring `.contains` checks) still passed.
    // Proves real alignment generically -- two rows whose stage names differ
    // in length must still place their trailing line-ref token
    // (`L108` / `L114`) at the identical column offset -- rather than pinning
    // the column width, which is a rendering constant free to change.
    func testShowSubsystemsColumnsAreActuallyAligned() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let store = RunStore(root: root)
        let manifest = sampleManifest(id: "20260905T190000Z-aarch64-testing-align")
        let runDirectory = try store.createRunDirectory(id: manifest.id)
        try Data(contentsOf: fixtureURL("testing-boot1-562-panic.txt"))
            .write(to: runDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(manifest)

        let output = try RunShow.render(
            manifest: try store.readManifest(id: manifest.id),
            store: store,
            options: RunShowOptions(subsystems: true)
        )
        let lines = output.split(separator: "\n", omittingEmptySubsequences: false)

        // "ext2 root filesystem mounted" (29 chars) reached L108;
        // "TTY subsystem initialized" (26 chars) reached L114 -- different
        // name lengths, so an unaligned table places "L108"/"L114" at
        // different offsets, and an aligned one does not.
        let shorterNameLine = try XCTUnwrap(lines.first { $0.contains("ext2 root filesystem mounted") })
        let longerNameLine = try XCTUnwrap(lines.first { $0.contains("TTY subsystem initialized") })
        let shorterOffset = try XCTUnwrap(shorterNameLine.range(of: "L108")?.lowerBound.utf16Offset(in: shorterNameLine))
        let longerOffset = try XCTUnwrap(longerNameLine.range(of: "L114")?.lowerBound.utf16Offset(in: longerNameLine))

        XCTAssertEqual(shorterOffset, longerOffset)
    }

    // Closes the gap where `Verdict.isFailure` / `RunStore.latestFailureManifest()`
    // -- the mechanism behind `breenix-runs show latest-fail` -- had zero
    // coverage: stores a newer *passing* run alongside an older *failing* one
    // (a real committed panic serial) and asserts `latest-fail` selects the
    // older failure, not the newer overall run, then drives it through
    // `RunShow.render` exactly as the `show` subcommand would.
    func testShowLatestFailSelectsOlderFailureOverNewerPassEndToEnd() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let store = RunStore(root: root)

        let failing = sampleManifest(
            id: "20260905T190000Z-aarch64-testing-fail",
            startedAt: Date(timeIntervalSince1970: 1_788_633_600),
            verdict: .fail("fixture panic")
        )
        let failingDirectory = try store.createRunDirectory(id: failing.id)
        try Data(contentsOf: fixtureURL("testing-boot1-562-panic.txt"))
            .write(to: failingDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(failing)

        let passing = sampleManifest(
            id: "20260905T193000Z-aarch64-testing-pass",
            startedAt: Date(timeIntervalSince1970: 1_788_635_400),
            verdict: .gateScript(command: ["docker/qemu/run-aarch64-testing-profile-boot-test.sh"], exitCode: 0)
        )
        let passingDirectory = try store.createRunDirectory(id: passing.id)
        try Data(contentsOf: fixtureURL("testing-boot1-562-panic.txt"))
            .write(to: passingDirectory.appendingPathComponent("serial.txt"))
        try store.writeManifest(passing)

        let selected = try store.latestFailureManifest()
        XCTAssertEqual(selected.id, failing.id)

        let output = try RunShow.render(manifest: selected, store: store, options: RunShowOptions(subsystems: true))
        XCTAssertTrue(output.contains("ARM64 boot complete"))
    }

    func testLatestFailureManifestThrowsWhenStoreHasNoFailures() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let store = RunStore(root: root)
        try store.writeManifest(sampleManifest(
            id: "20260905T190000Z-aarch64-testing-onlypass",
            verdict: .gateScript(command: ["gate.sh"], exitCode: 0)
        ))

        XCTAssertThrowsError(try store.latestFailureManifest()) { error in
            XCTAssertEqual(error as? RunStoreError, .runNotFound("latest-fail"))
        }
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
