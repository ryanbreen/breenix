import Foundation
@testable import BreenixRuns
import XCTest

final class BeastLauncherTests: XCTestCase {
    func testPrepareCloneRequestArgv() {
        let request = RemoteCommand.prepareCloneRequest(
            sha: "abc123def",
            paths: BeastPaths(clonePath: "/root/breenix-testclone")
        )

        XCTAssertEqual(request.executable, "/usr/bin/ssh")
        XCTAssertEqual(request.arguments, [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "beast",
            "sudo -n incus exec breenix-x86 -- bash -lc 'git -C /root/breenix fetch origin && rm -rf /root/breenix-testclone && git clone --shared /root/breenix /root/breenix-testclone && git -C /root/breenix-testclone checkout --detach abc123def'"
        ])
        XCTAssertTrue(request.combineOutput)
    }

    func testRunGateRequestArgvEncodesBootsModeAndGateTmpInsideClone() {
        let paths = BeastPaths(clonePath: "/root/breenix-testclone")
        let request = RemoteCommand.runGateRequest(boots: 3, mode: .kthread, timeoutSecs: 900, paths: paths)

        XCTAssertEqual(request.executable, "/usr/bin/ssh")
        XCTAssertEqual(request.arguments, [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "beast",
            "sudo -n incus exec breenix-x86 -- bash -lc 'mkdir -p /root/breenix-testclone/gate-tmp && source /root/.cargo/env && env BREENIX_GATE_TMP=/root/breenix-testclone/gate-tmp BREENIX_REPO_DIR=/root/breenix-testclone BREENIX_RUST_FORK=/root/breenix/rust-fork-real BREENIX_GATE_TIMEOUT=900 /root/breenix-testclone/docker/qemu/run-x86-gate.sh 3 kthread'"
        ])
        XCTAssertTrue(request.combineOutput)
        XCTAssertTrue(paths.gateTmpPath.hasPrefix(paths.clonePath + "/"))
    }

    func testPullEvidenceRequestArgvAndDoesNotCombineOutput() {
        let request = RemoteCommand.pullEvidenceRequest(paths: BeastPaths(clonePath: "/root/breenix-testclone"))

        XCTAssertEqual(request.arguments.last, "sudo -n incus exec breenix-x86 -- tar -czf - -C /root/breenix-testclone gate-tmp")
        XCTAssertFalse(request.combineOutput)
    }

    func testRemoveCloneRequestArgvTargetsClonePath() {
        let request = RemoteCommand.removeCloneRequest(paths: BeastPaths(clonePath: "/root/breenix-testclone"))

        XCTAssertEqual(request.arguments.last, "sudo -n incus exec breenix-x86 -- rm -rf /root/breenix-testclone")
    }

    func testPlanIsPureFunctionOfInputs() {
        let paths = BeastPaths(clonePath: "/root/breenix-testclone")
        let lhs = RemoteCommand.plan(sha: "abc123def", boots: 3, mode: .kthread, timeoutSecs: 900, paths: paths)
        let rhs = RemoteCommand.plan(sha: "abc123def", boots: 3, mode: .kthread, timeoutSecs: 900, paths: paths)
        let changedSHA = RemoteCommand.plan(sha: "fed321cba", boots: 3, mode: .kthread, timeoutSecs: 900, paths: paths)

        XCTAssertEqual(lhs, rhs)
        XCTAssertNotEqual(lhs.prepareClone, changedSHA.prepareClone)
        XCTAssertEqual(lhs.pullEvidence, changedSHA.pullEvidence)
        XCTAssertEqual(lhs.removeClone, changedSHA.removeClone)
    }

    func testParseHostFactsFromFixtureString() {
        let wallTime = Date(timeIntervalSince1970: 1_788_700_000)
        let fixture = """
        loadavg=0.35 0.85 1.15
        qemu_peers_x86=2
        qemu_peers_aarch64=2
        mem_total_kb=8097132
        qemu_version=QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1.18)
        cpu_model=Intel(R) Xeon(R) CPU E5-2640 v4 @ 2.40GHz
        \u{1B}[?1000l\u{1B}[?1002l\u{1B}[?25h
        """

        let sample = RemoteCommand.parseHostFacts(fixture, wallTime: wallTime)
        XCTAssertEqual(sample.wallTime, wallTime)
        XCTAssertEqual(sample.loadavg1, 0.35)
        XCTAssertEqual(sample.loadavg5, 0.85)
        XCTAssertEqual(sample.loadavg15, 1.15)
        XCTAssertEqual(sample.qemuPeersX86_64, 2)
        XCTAssertEqual(sample.qemuPeersAarch64, 2)
        XCTAssertEqual(sample.physMem, 8_097_132 * 1024)
        XCTAssertTrue(sample.qemuVersion?.contains("8.2.2") == true)
        XCTAssertTrue(sample.hostModel?.contains("Xeon") == true)

        let empty = RemoteCommand.parseHostFacts("", wallTime: wallTime)
        XCTAssertEqual(empty.qemuPeersX86_64, 0)
        XCTAssertEqual(empty.qemuPeersAarch64, 0)
        XCTAssertNil(empty.loadavg1)
        XCTAssertNil(empty.loadavg5)
        XCTAssertNil(empty.loadavg15)
        XCTAssertNil(empty.physMem)
        XCTAssertNil(empty.qemuVersion)
        XCTAssertNil(empty.hostModel)
    }

    func testTeardownRemovesCloneOnGateSuccess() throws {
        try assertTeardownRemovesClone(gateExitCode: 0)
    }

    func testTeardownRemovesCloneOnGateNonZeroExit() throws {
        try assertTeardownRemovesClone(gateExitCode: 1)
    }

    func testPrepareFailureCallsRemoveCloneAndPersistsNothing() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let runner = BeastScriptedProcessRunner()
        runner.prepareResult = ProcessResult(stdout: Data("fetch failed\n".utf8), exitCode: 1)
        let launcher = BeastLauncher(store: store, runner: runner)

        XCTAssertThrowsError(try launcher.runX86(options: options(runID: "prepare-fails"))) { error in
            guard case BeastLauncherError.prepareCloneFailed(let exitCode, _) = error else {
                return XCTFail("expected prepareCloneFailed, got \(error)")
            }
            XCTAssertEqual(exitCode, 1)
        }
        XCTAssertTrue(runner.calls.contains(RemoteCommand.removeCloneRequest(paths: BeastPaths(clonePath: "/root/breenix-prepare-fails"))))
        let entries = (try? FileManager.default.contentsOfDirectory(atPath: store.runsDirectory.path)) ?? []
        XCTAssertTrue(entries.isEmpty, "prepare failure must not leave an orphaned run directory: \(entries)")
    }

    func testGateTmpAndTarballRemovedAfterHarvest() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = try makeEvidenceTarball(root: root, entries: [
            1: ("user boot one\n", "kernel boot one\n")
        ])
        let runner = BeastScriptedProcessRunner()
        runner.pullResult = ProcessResult(stdout: fixture.tarball, exitCode: 0)
        runner.extractTarball = { request in
            try Self.copyGateTmpFixture(fixture.gateTmpSource, tarRequest: request)
            return ProcessResult(exitCode: 0)
        }
        let result = try runSuccessfulX86(root: root, runner: runner, runID: "harvest-one")
        let runDirectory = try XCTUnwrap(result.runDirectory)

        XCTAssertFalse(FileManager.default.fileExists(atPath: runDirectory.appendingPathComponent("gate-tmp", isDirectory: true).path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: runDirectory.appendingPathComponent("gate-tmp.tar.gz").path))

        let user = String(decoding: try Data(contentsOf: runDirectory.appendingPathComponent("serial_user.txt")), as: UTF8.self)
        let kernel = String(decoding: try Data(contentsOf: runDirectory.appendingPathComponent("serial_kernel.txt")), as: UTF8.self)
        XCTAssertEqual(user, "==== breenix-runs boot 1: breenix_gate_1/serial_user.log ====\nuser boot one\n\n")
        XCTAssertEqual(kernel, "==== breenix-runs boot 1: breenix_gate_1/serial_kernel.log ====\nkernel boot one\n\n")
    }

    func testTwoBootHarvestOrdersNaturally() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = try makeEvidenceTarball(root: root, entries: [
            10: ("user ten\n", "kernel ten\n"),
            9: ("user nine\n", "kernel nine\n")
        ])
        let runner = BeastScriptedProcessRunner()
        runner.pullResult = ProcessResult(stdout: fixture.tarball, exitCode: 0)
        runner.extractTarball = { request in
            try Self.copyGateTmpFixture(fixture.gateTmpSource, tarRequest: request)
            return ProcessResult(exitCode: 0)
        }
        let result = try runSuccessfulX86(root: root, runner: runner, runID: "harvest-natural")
        let runDirectory = try XCTUnwrap(result.runDirectory)
        let user = String(decoding: try Data(contentsOf: runDirectory.appendingPathComponent("serial_user.txt")), as: UTF8.self)

        let boot9 = try XCTUnwrap(user.range(of: "breenix_gate_9/serial_user.log")?.lowerBound)
        let boot10 = try XCTUnwrap(user.range(of: "breenix_gate_10/serial_user.log")?.lowerBound)
        XCTAssertLessThan(boot9, boot10)
    }

    func testFailedEvidencePullStillProducesTwoEmptySerialRefs() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let runner = BeastScriptedProcessRunner()
        runner.pullResult = ProcessResult(stdout: Data(), stderr: Data("tar failed\n".utf8), exitCode: 1)
        let result = try runSuccessfulX86(root: root, runner: runner, runID: "pull-fails")

        XCTAssertEqual(result.manifest.serials, [
            SerialRef(name: "serial_user.txt", path: "serial_user.txt", bytes: 0, stream: .com1),
            SerialRef(name: "serial_kernel.txt", path: "serial_kernel.txt", bytes: 0, stream: .com2)
        ])
    }

    func testHostFactsSamplingFailureDoesNotAbortRun() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let runner = BeastScriptedProcessRunner()
        runner.hostFactsResult = ProcessResult(stdout: Data("no facts\n".utf8), exitCode: 1)
        let result = try runSuccessfulX86(root: root, runner: runner, runID: "facts-fail")

        XCTAssertNil(result.manifest.host)
    }

    func testUnsupportedHostIsRejected() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let runner = BeastScriptedProcessRunner()
        let launcher = BeastLauncher(
            store: RunStore(root: root.appendingPathComponent("store", isDirectory: true)),
            runner: runner,
            pathsTemplate: BeastPaths(host: "localhost", clonePath: "")
        )

        XCTAssertThrowsError(try launcher.runX86(options: options(runID: "bad-host"))) { error in
            XCTAssertEqual(error as? BeastLauncherError, .unsupportedHost("localhost"))
        }
        XCTAssertTrue(runner.calls.isEmpty)
    }

    func testPlanMatchesWhatRunX86WouldExecute() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let runID = "plan-match"
        let runner = BeastScriptedProcessRunner()
        let launcher = BeastLauncher(
            store: RunStore(root: root.appendingPathComponent("store", isDirectory: true)),
            runner: runner
        )
        let launchOptions = options(runID: runID)
        let plan = try launcher.plan(options: launchOptions)

        _ = try launcher.runX86(options: launchOptions)

        XCTAssertTrue(runner.calls.contains(plan.prepareClone))
        XCTAssertTrue(runner.calls.contains(plan.runGate))
        XCTAssertTrue(runner.calls.contains(plan.pullEvidence))
        XCTAssertTrue(runner.calls.contains(plan.removeClone))
    }

    private func assertTeardownRemovesClone(gateExitCode: Int32) throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let runID = "teardown-\(gateExitCode)"
        let runner = BeastScriptedProcessRunner()
        runner.gateResult = ProcessResult(stdout: Data("gate exited \(gateExitCode)\n".utf8), exitCode: gateExitCode)
        _ = try runSuccessfulX86(root: root, runner: runner, runID: runID)

        let remove = RemoteCommand.removeCloneRequest(paths: BeastPaths(clonePath: "/root/breenix-\(runID)"))
        XCTAssertTrue(runner.calls.contains(remove))
    }

    private func runSuccessfulX86(root: URL, runner: BeastScriptedProcessRunner, runID: String) throws -> BeastLaunchResult {
        let store = RunStore(root: root.appendingPathComponent("store", isDirectory: true))
        let launcher = BeastLauncher(store: store, runner: runner)
        return try launcher.runX86(options: options(runID: runID))
    }

    private func options(runID: String) -> BeastLaunchOptions {
        BeastLaunchOptions(
            boots: 1,
            mode: .full,
            sha: "abc123def",
            gitDirty: true,
            tags: ["test"],
            persist: true,
            runID: runID
        )
    }

    private struct EvidenceTarball {
        var gateTmpSource: URL
        var tarball: Data
    }

    private func makeEvidenceTarball(root: URL, entries: [Int: (user: String, kernel: String)]) throws -> EvidenceTarball {
        let sourceRoot = root.appendingPathComponent("tar-source-\(UUID().uuidString)", isDirectory: true)
        let gateTmp = sourceRoot.appendingPathComponent("gate-tmp", isDirectory: true)
        try FileManager.default.createDirectory(at: gateTmp, withIntermediateDirectories: true)

        for (boot, content) in entries {
            let directory = gateTmp.appendingPathComponent("breenix_gate_\(boot)", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try Data(content.user.utf8).write(to: directory.appendingPathComponent("serial_user.log"))
            try Data(content.kernel.utf8).write(to: directory.appendingPathComponent("serial_kernel.log"))
        }

        let tarballURL = root.appendingPathComponent("evidence-\(UUID().uuidString).tar.gz")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
        process.arguments = ["-czf", tarballURL.path, "-C", sourceRoot.path, "gate-tmp"]
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)

        return EvidenceTarball(gateTmpSource: gateTmp, tarball: try Data(contentsOf: tarballURL))
    }

    private static func copyGateTmpFixture(_ sourceGateTmp: URL, tarRequest: ProcessRequest) throws {
        guard let cIndex = tarRequest.arguments.firstIndex(of: "-C"),
              tarRequest.arguments.indices.contains(cIndex + 1) else {
            XCTFail("tar request missing -C destination: \(tarRequest)")
            return
        }
        let destinationRunDirectory = URL(fileURLWithPath: tarRequest.arguments[cIndex + 1], isDirectory: true)
        let destinationGateTmp = destinationRunDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        if FileManager.default.fileExists(atPath: destinationGateTmp.path) {
            try FileManager.default.removeItem(at: destinationGateTmp)
        }
        try FileManager.default.copyItem(at: sourceGateTmp, to: destinationGateTmp)
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-beast-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

private final class BeastScriptedProcessRunner: ProcessRunner {
    var calls: [ProcessRequest] = []
    var hostFactsResult = ProcessResult(stdout: Data("""
    loadavg=0.35 0.85 1.15
    qemu_peers_x86=0
    qemu_peers_aarch64=0
    mem_total_kb=8097132
    qemu_version=QEMU emulator version 8.2.2
    cpu_model=Intel(R) Xeon(R) CPU E5-2640 v4 @ 2.40GHz
    """.utf8), exitCode: 0)
    var prepareResult = ProcessResult(stdout: Data("prepared\n".utf8), exitCode: 0)
    var gateResult = ProcessResult(stdout: Data("gate ok\n".utf8), exitCode: 0)
    var pullResult = ProcessResult(stdout: Data(), exitCode: 0)
    var removeResult = ProcessResult(exitCode: 0)
    var extractTarball: (ProcessRequest) throws -> ProcessResult = { _ in ProcessResult(exitCode: 0) }

    func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)?) throws -> ProcessResult {
        calls.append(request)

        if request.executable == "/usr/bin/tar" {
            return try extractTarball(request)
        }

        guard request.executable == "/usr/bin/ssh", let remote = request.arguments.last else {
            XCTFail("unexpected request: \(request)")
            return ProcessResult(exitCode: 127)
        }

        if remote.contains("read la1 la2 la3") {
            return hostFactsResult
        }
        if remote.contains("git clone --shared") {
            return prepareResult
        }
        if remote.contains("run-x86-gate.sh") {
            if request.combineOutput {
                outputHandler?(gateResult.stdout)
            }
            return gateResult
        }
        if remote.contains("tar -czf -") {
            return pullResult
        }
        if remote.contains("rm -rf") {
            return removeResult
        }

        XCTFail("unexpected ssh remote command: \(remote)")
        return ProcessResult(exitCode: 127)
    }
}
