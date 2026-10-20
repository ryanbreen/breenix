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


















    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-beast-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}
