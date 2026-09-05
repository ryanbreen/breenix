import Foundation
@testable import BreenixRuns
import XCTest

final class LocalGateLauncherTests: XCTestCase {
    func testProdProfileFailureCopyIsExcludedFromMergedSerial() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let serialContent = "MAIN SERIAL CONTENT\n"
        let runner = ScriptedProcessRunner(hostFactsOutputs: Self.hostFactsFixture())
        runner.gateScriptSuffix = "run-aarch64-prod-profile-boot-test.sh"
        runner.gateExitCode = 1
        runner.gateStdout = "boot 1 failed\n"
        runner.populateGateTmp = { gateTmp in
            let mainDir = gateTmp.appendingPathComponent("breenix_aarch64_prod_profile", isDirectory: true)
            try? FileManager.default.createDirectory(at: mainDir, withIntermediateDirectories: true)
            try? Data(serialContent.utf8).write(to: mainDir.appendingPathComponent("serial.txt"))

            // The gate's own failure-preservation copy: byte-identical, named
            // literally serial.txt (run-aarch64-prod-profile-boot-test.sh:225-236).
            let failureDir = gateTmp.appendingPathComponent("breenix_prod_profile_failures/20260101T000000Z", isDirectory: true)
            try? FileManager.default.createDirectory(at: failureDir, withIntermediateDirectories: true)
            try? Data(serialContent.utf8).write(to: failureDir.appendingPathComponent("serial.txt"))
        }

        let launcher = LocalGateLauncher(store: store, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true), runner: runner, hostLock: AlwaysAvailableHostLock())
        let result = try launcher.runArm(options: LocalGateLaunchOptions(profile: .prod, boots: 1, persist: true))

        let merged = String(decoding: try Data(contentsOf: result.serialURL), as: UTF8.self)
        let bootHeaderCount = merged.components(separatedBy: "==== breenix-runs boot").count - 1
        XCTAssertEqual(bootHeaderCount, 1, "the failure-preservation copy duplicated the single failing boot under a second boot header:\n\(merged)")
        XCTAssertEqual(merged.components(separatedBy: serialContent.trimmingCharacters(in: .newlines)).count - 1, 1)
    }

    func testGateTmpIsRemovedAfterHarvest() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let runner = ScriptedProcessRunner(hostFactsOutputs: Self.hostFactsFixture())
        runner.gateScriptSuffix = "run-aarch64-boot-test-strict.sh"
        runner.gateExitCode = 0
        runner.gateStdout = "boot 1 passed\n"
        runner.populateGateTmp = { gateTmp in
            let iterDir = gateTmp.appendingPathComponent("breenix_aarch64_strict_1", isDirectory: true)
            try? FileManager.default.createDirectory(at: iterDir, withIntermediateDirectories: true)
            try? Data("serial bytes\n".utf8).write(to: iterDir.appendingPathComponent("serial.txt"))
        }

        let launcher = LocalGateLauncher(store: store, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true), runner: runner, hostLock: AlwaysAvailableHostLock())
        let result = try launcher.runArm(options: LocalGateLaunchOptions(profile: .strict, boots: 1, persist: true))

        let gateTmpURL = result.runDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        XCTAssertFalse(FileManager.default.fileExists(atPath: gateTmpURL.path), "gate-tmp/ must not survive past harvest")
        XCTAssertTrue(FileManager.default.fileExists(atPath: result.serialURL.path), "the merged serial.txt must still exist")
    }

    func testPreflightRefusalSurfacesAsRefusedVerdict() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let runner = ScriptedProcessRunner(hostFactsOutputs: Self.hostFactsFixture())
        runner.gateScriptSuffix = "run-aarch64-boot-test-strict.sh"
        runner.gateExitCode = 1
        runner.gateStdout = "Error: /path/to/kernel-aarch64 was not built with --features boot_tests.\n  Missing boot_tests-only marker literal(s): [BOOT_TESTS:\n"
        runner.populateGateTmp = { _ in }

        let launcher = LocalGateLauncher(store: store, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true), runner: runner, hostLock: AlwaysAvailableHostLock())
        let result = try launcher.runArm(options: LocalGateLaunchOptions(profile: .strict, boots: 1, persist: true))

        guard case .refused = result.manifest.verdict else {
            return XCTFail("expected .refused verdict for a boot_tests preflight refusal, got \(result.manifest.verdict)")
        }
    }

    func testHostLockRefusalLeavesNoOrphanRunDirectory() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let runner = ScriptedProcessRunner(hostFactsOutputs: Self.hostFactsFixture())
        let launcher = LocalGateLauncher(store: store, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true), runner: runner, hostLock: AlwaysRefusingHostLock())

        XCTAssertThrowsError(try launcher.runArm(options: LocalGateLaunchOptions(profile: .strict, boots: 1, persist: true)))

        let entries = (try? FileManager.default.contentsOfDirectory(atPath: store.runsDirectory.path)) ?? []
        XCTAssertTrue(entries.isEmpty, "a refused launch must not leave an orphaned run directory behind: \(entries)")
    }

    func testNoStoreCleansUpScratchDirectoryAfterReturn() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)

        let runner = ScriptedProcessRunner(hostFactsOutputs: Self.hostFactsFixture())
        runner.gateScriptSuffix = "run-aarch64-boot-test-strict.sh"
        runner.gateExitCode = 0
        runner.gateStdout = "boot 1 passed\n"
        runner.populateGateTmp = { gateTmp in
            let iterDir = gateTmp.appendingPathComponent("breenix_aarch64_strict_1", isDirectory: true)
            try? FileManager.default.createDirectory(at: iterDir, withIntermediateDirectories: true)
            try? Data("serial bytes\n".utf8).write(to: iterDir.appendingPathComponent("serial.txt"))
        }

        let launcher = LocalGateLauncher(store: store, repoRoot: URL(fileURLWithPath: "/repo", isDirectory: true), runner: runner, hostLock: AlwaysAvailableHostLock())
        let result = try launcher.runArm(options: LocalGateLaunchOptions(profile: .strict, boots: 1, persist: false))

        XCTAssertFalse(result.stored)
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: result.runDirectory.path),
            "--no-store must leave no scratch directory behind"
        )
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-launcher-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private static func hostFactsFixture() -> [String: ProcessResult] {
        [
            "/usr/bin/pgrep -c qemu-system-aarch64": ProcessResult(stdout: Data("0\n".utf8), exitCode: 1),
            "/usr/bin/pgrep -c qemu-system-x86_64": ProcessResult(stdout: Data("0\n".utf8), exitCode: 1),
            "/usr/sbin/sysctl -n vm.loadavg": ProcessResult(stdout: Data("{ 1.00 1.00 1.00 }\n".utf8), exitCode: 0),
            "/usr/bin/pmset -g therm": ProcessResult(stdout: Data(), stderr: Data("unsupported\n".utf8), exitCode: 1),
            "/usr/sbin/sysctl hw.model hw.memsize": ProcessResult(stdout: Data("hw.model: Mac16,1\nhw.memsize: 34359738368\n".utf8), exitCode: 0),
            "/usr/bin/env qemu-system-aarch64 --version": ProcessResult(stdout: Data("QEMU emulator version 10.0.2\n".utf8), exitCode: 0),
            "/usr/bin/git rev-parse HEAD @ /repo": ProcessResult(stdout: Data("7a19f550abcdef\n".utf8), exitCode: 0),
            "/usr/bin/git status --porcelain @ /repo": ProcessResult(stdout: Data(), exitCode: 0)
        ]
    }
}

private struct AlwaysAvailableHostLock: HostLock {
    func acquire(runner: ProcessRunner) throws {}
}

private struct AlwaysRefusingHostLock: HostLock {
    func acquire(runner: ProcessRunner) throws {
        throw LocalGateLauncherError.qemuAlreadyRunning("test refusal")
    }
}

/// Routes host-fact subprocess calls through a fixed fixture table (same shape
/// as HostFactsTests' fixture), and treats any executable whose path ends in
/// `gateScriptSuffix` as the gate script itself: it calls `populateGateTmp`
/// with the run's BREENIX_GATE_TMP directory (simulating the gate writing
/// per-boot serials there) before returning the scripted exit code/stdout.
private final class ScriptedProcessRunner: ProcessRunner {
    var hostFactsOutputs: [String: ProcessResult]
    var gateScriptSuffix = ""
    var gateExitCode: Int32 = 0
    var gateStdout = ""
    var populateGateTmp: (URL) -> Void = { _ in }

    init(hostFactsOutputs: [String: ProcessResult]) {
        self.hostFactsOutputs = hostFactsOutputs
    }

    func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)?) throws -> ProcessResult {
        if !gateScriptSuffix.isEmpty, request.executable.hasSuffix(gateScriptSuffix) {
            if let gateTmpPath = request.environment["BREENIX_GATE_TMP"] {
                populateGateTmp(URL(fileURLWithPath: gateTmpPath, isDirectory: true))
            }
            let data = Data(gateStdout.utf8)
            if request.combineOutput {
                outputHandler?(data)
            }
            return ProcessResult(stdout: data, exitCode: gateExitCode)
        }

        let key = Self.key(for: request)
        guard let result = hostFactsOutputs[key] else {
            XCTFail("No fixture output for \(key)")
            return ProcessResult(exitCode: 127)
        }
        return result
    }

    private static func key(for request: ProcessRequest) -> String {
        var key = ([request.executable] + request.arguments).joined(separator: " ")
        if let path = request.workingDirectory?.path {
            key += " @ \(path)"
        }
        return key
    }
}
