import Foundation
@testable import BreenixRuns
import XCTest

final class BeastLauncherTests: XCTestCase {
    func testHardwareProfilesMatchCatalogInOrder() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("docs/x86-profiles.json"))
        let rows = try JSONDecoder().decode([[String: String]].self, from: data)
        XCTAssertEqual(X86HardwareProfile.allCases.map(\.rawValue), rows.map { $0["name"]! })
    }

    func testSuiteListsPreserveOrderAndRejectDuplicatesOrShellSyntax() {
        XCTAssertTrue(RemoteCommand.isSuiteList("files-io,directories,processes"))
        XCTAssertTrue(RemoteCommand.isSuiteList("files-io"))
        for value in ["", ",files-io", "files-io,", "files-io,files-io", "files-io, directories", "files-io;exit", "files-io\n"] {
            XCTAssertFalse(RemoteCommand.isSuiteList(value), value)
        }
        let request = RemoteCommand.runGateRequest(boots: 1, mode: .full, timeoutSecs: 900,
            paths: BeastPaths(clonePath: "/root/run"), suite: "files-io,directories,processes")
        XCTAssertTrue(request.arguments.last!.contains("BREENIX_BOOT_SUITE=files-io,directories,processes"))
    }

    func testProgressBundleWorksWithoutTheTestedCheckout() throws {
        var repo = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { repo.deleteLastPathComponent() }
        let bundle = try BeastLauncher.progressBundle(repoRoot: repo)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let helper = try Data(contentsOf: repo.appendingPathComponent("scripts/host-slots.py")).base64EncodedString()
        let request = RemoteCommand.runGateRequest(boots: 1, mode: .full, timeoutSecs: 300, paths: BeastPaths(clonePath: "/root/run"), slotHelperBase64: helper, progressBundleBase64: bundle)
        XCTAssertLessThan(try XCTUnwrap(request.arguments.last).utf8.count, 65_536)
        let detached = RemoteCommand.startGateRequest(gate: request, paths: BeastPaths(clonePath: "/root/run"))
        XCTAssertLessThan(try XCTUnwrap(detached.arguments.last).utf8.count, 120_000,
                          "leave headroom below Linux's 128 KiB single-argument limit")
        let syntax = try RealProcessRunner().run(ProcessRequest(executable: "/bin/bash", arguments: ["-n", "-c", try XCTUnwrap(request.arguments.last)]))
        XCTAssertEqual(syntax.exitCode, 0)
        let code = """
        import pathlib,runpy,shlex,subprocess,sys
        script=shlex.split(sys.argv[1])[-1]
        subprocess.run(['/bin/bash','-n','-c',script],check=True)
        installer=script.split(' && python3 -c ')[2]
        command=[sys.executable,'-c']+shlex.split(installer)
        command[3]=sys.argv[2]
        subprocess.run(command,check=True)
        root=pathlib.Path(sys.argv[2])
        p=runpy.run_path(str(root/'scripts/boot_progress.py'))['BootProgress'](root/'serial')
        assert 'EXEC_ARGV_TEST_PASSED' in p.markers
        assert 'Kernel initialization complete!' in p.markers
        assert 'ntp' in p.user_tags
        assert 'http_fetch_test' in p.user_tags
        assert 'timer' not in p.user_tags
        """
        let result = try RealProcessRunner().run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", code, try XCTUnwrap(request.arguments.last), root.path]))
        XCTAssertEqual(result.exitCode, 0, String(decoding: result.stderr, as: UTF8.self))
    }

    func testPersistentTreePreparationAndCleanupKeepRunEvidencePrivate() {
        var paths = BeastPaths(clonePath: "/root/run")
        paths.laneKey = String(repeating: "a", count: 64)
        paths.requestedSHA = String(repeating: "b", count: 40)
        let prepare = RemoteCommand.prepareCloneRequest(sha: paths.requestedSHA!, paths: paths, treeHelperBase64: "aGVscGVy")
        XCTAssertTrue(prepare.arguments.last!.contains("/root/run/gate-tmp/gate-tree.py"))
        XCTAssertFalse(prepare.arguments.last!.contains("git clone"))
        let gate = RemoteCommand.runGateRequest(boots: 1, mode: .full, timeoutSecs: 300, paths: paths, slotHelperBase64: "c2xvdHM=", progressBundleBase64: "e30=")
        XCTAssertTrue(gate.arguments.last!.contains("gate-tree.py /root/breenix " + paths.laneKey!))
        XCTAssertTrue(gate.arguments.last!.contains("BREENIX_SLOT_WORKTREE=\"${BREENIX_GATE_CACHE_DIR:-/root/breenix-gate-cache}/trees/" + paths.laneKey!))
        XCTAssertTrue(gate.arguments.last!.contains("BREENIX_SLOT_COMMIT=" + paths.requestedSHA!))
        XCTAssertTrue(gate.arguments.last!.contains("BREENIX_SLOT_PROGRESS_HELPER=/root/run/gate-tmp/watchdog/scripts/boot_progress.py"))
        XCTAssertTrue(gate.arguments.last!.contains("json.loads(zlib.decompress(base64.b64decode(sys.argv[2]),-15))"))
        XCTAssertFalse(gate.arguments.last!.contains("acquire x86-boot"), "current gates acquire and release their own distinct build/boot leases")
        XCTAssertEqual(RemoteCommand.removeCloneRequest(paths: paths).arguments.last,
            "sudo -n incus exec breenix-x86 -- python3 /root/run/gate-tmp/gate-tree.py remove-evidence /root/run")
    }

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
            "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2",
            "beast",
            "sudo -n incus exec breenix-x86 -- bash -lc 'git -C /root/breenix fetch --no-tags --no-write-fetch-head --no-auto-gc origin abc123def && rm -rf /root/breenix-testclone && git clone --shared /root/breenix /root/breenix-testclone && git -C /root/breenix-testclone checkout --detach abc123def'"
        ])
        XCTAssertTrue(request.combineOutput)
    }

    func testConcurrentPrepareFetchesSHAWithoutChangingSharedRefs() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let source = root.appendingPathComponent("source")
        let canonical = root.appendingPathComponent("canonical")
        let runner = RealProcessRunner()
        func git(_ arguments: [String]) throws -> String {
            let result = try runner.run(ProcessRequest(executable: "/usr/bin/git", arguments: arguments))
            XCTAssertEqual(result.exitCode, 0, result.stderrString)
            return result.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        _ = try git(["init", source.path])
        let commitArguments = ["-C", source.path, "-c", "core.hooksPath=/dev/null",
                               "-c", "user.name=Slot Test", "-c", "user.email=test@example.invalid",
                               "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m"]
        _ = try git(commitArguments + ["initial"])
        _ = try git(["clone", source.path, canonical.path])
        _ = try git(commitArguments + ["requested"])
        let sha = try git(["-C", source.path, "rev-parse", "HEAD"])
        _ = try git(["-C", source.path, "tag", "new-tag"])
        let refs = try git(["-C", canonical.path, "for-each-ref", "--format=%(refname) %(objectname)"])
        let fetchHead = canonical.appendingPathComponent(".git/FETCH_HEAD")
        try Data("existing fetch record\n".utf8).write(to: fetchHead)

        var processes: [(Process, Pipe, URL)] = []
        defer {
            for (process, _, _) in processes where process.isRunning {
                process.terminate()
                process.waitUntilExit()
            }
        }
        for name in ["first", "second"] {
            let clone = root.appendingPathComponent(name)
            let request = RemoteCommand.prepareCloneRequest(
                sha: sha, paths: BeastPaths(canonicalRepoDir: canonical.path, clonePath: clone.path))
            // Execute the generated preparation body locally; no SSH, Incus or VM.
            let remote = try XCTUnwrap(request.arguments.last)
            let body = try XCTUnwrap(remote.split(separator: "'", maxSplits: 2).dropFirst().first)
            let process = Process()
            let output = Pipe()
            process.executableURL = URL(fileURLWithPath: "/bin/bash")
            process.arguments = ["-c", String(body)]
            process.standardOutput = output
            process.standardError = output
            try process.run()
            processes.append((process, output, clone))
        }
        for (process, output, clone) in processes {
            process.waitUntilExit()
            let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            XCTAssertEqual(process.terminationStatus, 0, text)
            XCTAssertEqual(try git(["-C", clone.path, "rev-parse", "HEAD"]), sha)
        }
        XCTAssertEqual(try git(["-C", canonical.path, "for-each-ref", "--format=%(refname) %(objectname)"]), refs)
        XCTAssertEqual(try String(contentsOf: fetchHead, encoding: .utf8), "existing fetch record\n")
    }

    func testRunGateRequestArgvEncodesBootsModeAndGateTmpInsideClone() {
        let paths = BeastPaths(clonePath: "/root/breenix-testclone")
        let request = RemoteCommand.runGateRequest(boots: 3, mode: .kthread, timeoutSecs: 900, paths: paths)

        XCTAssertEqual(request.executable, "/usr/bin/ssh")
        XCTAssertEqual(request.arguments, [
            "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2", "beast",
            #"sudo -n incus exec breenix-x86 -- bash -lc 'mkdir -p /root/breenix-testclone/gate-tmp && python3 -c "import json,pathlib,sys;pid=int(sys.argv[1]);birth=pathlib.Path(\"/proc/%d/stat\"%pid).read_text().rsplit(\")\",1)[1].split()[19];pathlib.Path(sys.argv[2]).write_text(json.dumps([pid,birth,sys.argv[3]]))" "$$" /root/breenix-testclone/gate-tmp/launcher-gate.json /root/breenix/scripts/host-slots.py && source /root/.cargo/env && exec env BREENIX_GATE_TMP=/root/breenix-testclone/gate-tmp BREENIX_REPO_DIR=/root/breenix-testclone BREENIX_RUST_FORK=/root/breenix/rust-fork-real BREENIX_GATE_TIMEOUT=900 BREENIX_FULL_BACKSTOP=1800 CARGO_BUILD_JOBS=4 python3 /root/breenix/scripts/host-slots.py supervise -- bash -c "if [ ! -f /root/breenix-testclone/scripts/host-slots.py ]; then python3 /root/breenix/scripts/host-slots.py acquire x86-build && python3 /root/breenix/scripts/host-slots.py acquire x86-boot || exit 1; fi; exec /root/breenix-testclone/docker/qemu/run-x86-gate.sh 3 kthread"'"#
        ])
        XCTAssertTrue(request.combineOutput)
        XCTAssertTrue(paths.gateTmpPath.hasPrefix(paths.clonePath + "/"))
    }

    func testQuietOverrideForwardingAndRejection() throws {
        let paths = BeastPaths(clonePath: "/root/run")
        for value in [nil, "", "0", "1"] as [String?] {
            let plan = RemoteCommand.plan(sha: "abc123", boots: 1, mode: .full,
                timeoutSecs: 300, paths: paths, quietOverride: value)
            let command = try XCTUnwrap(plan.runGate.arguments.last)
            switch value {
            case "0": XCTAssertTrue(command.contains(" BREENIX_QUIET=0 "))
            case "1": XCTAssertTrue(command.contains(" BREENIX_QUIET=1 "))
            default: XCTAssertFalse(command.contains("BREENIX_QUIET="))
            }
        }
        for value in ["2", "0;exit 0", "1\n"] {
            let request = RemoteCommand.runGateRequest(boots: 1, mode: .full,
                timeoutSecs: 300, paths: paths, quietOverride: value)
            XCTAssertEqual(request.arguments.last, "exit 2")
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/sh")
            process.arguments = ["-c", try XCTUnwrap(request.arguments.last)]
            try process.run()
            process.waitUntilExit()
            XCTAssertEqual(process.terminationStatus, 2)
        }
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

    func testRecoverStopsHarvestsAndRemovesPreviouslyUnreachableRun() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root.appendingPathComponent("store"))
        let id = "unreachable-run"
        let endedAt = Date(timeIntervalSince1970: 1_788_700_000)
        let manifest = RunManifest(id: id, startedAt: endedAt.addingTimeInterval(-60), endedAt: endedAt, arch: .x86_64,
            profile: "gate", launcher: .beastSSH, kernel: KernelIdentity(), host: nil,
            verdict: .fail("remote unreachable"), verdictSource: .none, serials: [],
            captures: [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: 0)],
            command: ["historical-gate"], env: ["BREENIX_LANE_KEY": String(repeating: "a", count: 64)],
            tags: [], notes: nil)
        try store.writeManifest(manifest)
        let source = root.appendingPathComponent("source")
        let boot = source.appendingPathComponent("gate-tmp/breenix_gate_1")
        try FileManager.default.createDirectory(at: boot, withIntermediateDirectories: true)
        try Data("user evidence\n".utf8).write(to: boot.appendingPathComponent("serial_user.log"))
        try Data("kernel evidence\n".utf8).write(to: boot.appendingPathComponent("serial_kernel.log"))
        try Data("gate evidence\n".utf8).write(to: source.appendingPathComponent("gate-tmp/gate-output.log"))
        let archive = root.appendingPathComponent("evidence.tar.gz")
        let packed = try RealProcessRunner().run(ProcessRequest(executable: "/usr/bin/tar",
            arguments: ["-czf", archive.path, "-C", source.path, "gate-tmp"]))
        XCTAssertEqual(packed.exitCode, 0, packed.stderrString)
        var paths = BeastPaths(clonePath: "/root/breenix-" + id)
        paths.laneKey = manifest.env["BREENIX_LANE_KEY"]
        let stop = RemoteCommand.stopGateRequest(paths: paths)
        let pull = RemoteCommand.pullEvidenceRequest(paths: paths)
        let remove = RemoteCommand.removeCloneRequest(paths: paths)
        let runner = ReviewProcessRunner()
        let bytes = try Data(contentsOf: archive)
        runner.respond = { request in
            if request == stop { return ProcessResult(exitCode: 0) }
            if request == pull { return ProcessResult(stdout: bytes, exitCode: 0) }
            if request.executable == "/usr/bin/tar" { return try RealProcessRunner().run(request) }
            if request == remove {
                XCTAssertEqual(try store.readManifest(id: id).env["BREENIX_REMOTE_QUIESCED"], "1")
                return ProcessResult(exitCode: 0)
            }
            XCTFail("unexpected request: \(request)")
            return ProcessResult(exitCode: 1)
        }
        let launcher = BeastLauncher(store: store, runner: runner, targetGuard: nil)
        try launcher.recoverX86(id: id)
        XCTAssertEqual(runner.calls.map(\.executable), ["/usr/bin/ssh", "/usr/bin/ssh", "/usr/bin/tar", "/usr/bin/ssh"])
        XCTAssertEqual(runner.calls.first, stop)
        XCTAssertEqual(runner.calls[1], pull)
        XCTAssertEqual(runner.calls.last, remove)
        let recovered = try store.readManifest(id: id)
        XCTAssertEqual(recovered.verdict, manifest.verdict)
        XCTAssertEqual(recovered.endedAt, manifest.endedAt)
        XCTAssertEqual(recovered.env["BREENIX_REMOTE_QUIESCED"], "1")
        XCTAssertEqual(recovered.captures.first?.bytes, Data("gate evidence\n".utf8).count)
        for (name, evidence) in [("serial_user.txt", "user evidence\n"), ("serial_kernel.txt", "kernel evidence\n")] {
            let text = try String(contentsOf: store.runDirectory(id: id).appendingPathComponent(name), encoding: .utf8)
            XCTAssertTrue(text.contains(evidence), text)
            XCTAssertEqual(recovered.serials.first { $0.name == name }?.bytes, text.utf8.count)
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.runDirectory(id: id).appendingPathComponent("gate-tmp").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.runDirectory(id: id).appendingPathComponent("gate-tmp.tar.gz").path))
    }

    func testTargetPolicyRefusalStopsBeforeBeast() throws {
        let runner = ReviewProcessRunner()
        runner.respond = { _ in ProcessResult(stderr: Data("effort lane refused\n".utf8), exitCode: 1) }
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), runner: runner,
            targetGuard: URL(fileURLWithPath: "/repo/scripts/effort-target-guard.py"))
        XCTAssertThrowsError(try launcher.checkTargetPolicy(options: BeastLaunchOptions(sha: "abc123def"))) {
            XCTAssertEqual($0 as? BeastLauncherError, .targetPolicyRefused("effort lane refused"))
        }
        XCTAssertEqual(runner.calls.map(\.executable), ["/usr/bin/env"])
        XCTAssertEqual(runner.calls.first?.arguments,
            ["python3", "/repo/scripts/effort-target-guard.py", "--repo", "/repo", "--commit", "abc123def", "--target", "x86"])
    }

    func testAcceptedTargetPolicyStampsTheRemoteGate() throws {
        let runner = ReviewProcessRunner()
        let sha = String(repeating: "a", count: 40)
        runner.respond = { _ in ProcessResult(stdout: Data((sha + "\n").utf8), exitCode: 0) }
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), runner: runner,
            targetGuard: URL(fileURLWithPath: "/repo/scripts/effort-target-guard.py"))
        var options = BeastLaunchOptions(sha: "abc123def", catchup: true)
        options.targetPolicySHA = try launcher.checkTargetPolicy(options: options)
        XCTAssertEqual(options.targetPolicySHA, sha)
        XCTAssertEqual(runner.calls.first?.arguments.last, "--catchup")
        let plan = try launcher.plan(options: options)
        XCTAssertTrue(try XCTUnwrap(plan.runGate.arguments.last).contains("BREENIX_TARGET_POLICY_CHECKED=x86:" + sha))
        XCTAssertEqual(runner.calls.count, 1, "planning must not execute the remote gate")
    }

    func testUnsupportedHostIsRejected() throws {
        let runner = ReviewProcessRunner()
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), runner: runner,
            pathsTemplate: BeastPaths(host: "localhost", clonePath: ""), targetGuard: nil)
        XCTAssertThrowsError(try launcher.plan(options: BeastLaunchOptions(sha: "abc123def"))) {
            XCTAssertEqual($0 as? BeastLauncherError, .unsupportedHost("localhost"))
        }
        XCTAssertTrue(runner.calls.isEmpty)
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-beast-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

private final class ReviewProcessRunner: ProcessRunner {
    var calls: [ProcessRequest] = []
    var respond: ((ProcessRequest) throws -> ProcessResult)?
    func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)?) throws -> ProcessResult {
        calls.append(request)
        return try XCTUnwrap(respond)(request)
    }
}
