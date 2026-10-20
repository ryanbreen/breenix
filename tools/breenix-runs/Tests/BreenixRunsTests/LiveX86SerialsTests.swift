import Foundation
import Darwin
@testable import BreenixRuns
import XCTest

final class LiveX86SerialsTests: XCTestCase {
    func testVigilTimeoutReapsHungScriptAndItsChildWithoutSSH() throws {
        let pidFile = FileManager.default.temporaryDirectory.appendingPathComponent("breenix-timeout-\(UUID().uuidString).pid")
        defer { try? FileManager.default.removeItem(at: pidFile) }
        let command = "sleep 30 & echo $! > '\(pidFile.path)'; wait"
        let result = try RealProcessRunner().run(ProcessRequest(executable: "/bin/bash", arguments: ["-c", command], timeoutSecs: 1))
        XCTAssertEqual(result.exitCode, 124, String(decoding: result.stderr, as: UTF8.self))
        let pid = try XCTUnwrap(Int32(String(contentsOf: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)))
        XCTAssertEqual(Darwin.kill(pid, 0), -1, "the timed-out script left its sleep child running")
        XCTAssertEqual(errno, ESRCH)
    }

    func testDisconnectCleanupFindsDetachedWorkerAndExcludesReusedPIDAndPeer() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        for supervisorMatches in [false, true] {
        let code = """
        import pathlib, json, shutil
        proc = pathlib.Path(\(String(reflecting: root.path))) / "proc"
        record = proc.parent / "gate-tmp" / "launcher-gate.json"
        if proc.exists(): shutil.rmtree(proc)
        record.parent.mkdir(exist_ok=True)
        record.write_text(json.dumps([100, "original-birth"]))
        for pid, birth, helper in [(100, \(String(reflecting: supervisorMatches ? "original-birth" : "reused-birth")), "/peer/host-slots.py"),
                                   (200, "worker-birth", str(record.parent / "host-slots.py")),
                                   (300, "peer-birth", "/another/host-slots.py")]:
            directory = proc / str(pid)
            directory.mkdir(parents=True)
            fields = ["S"] + ["0"] * 18 + [birth]
            (directory / "stat").write_text(str(pid) + " (python3) " + " ".join(fields))
            (directory / "cmdline").write_bytes(b"python3\\0" + helper.encode() + b"\\0supervise\\0")
        namespace = dict(__name__="fake")
        exec(\(String(reflecting: RemoteCommand.gateStopper)), namespace)
        killed = []
        def fake_kill(pid, number):
            killed.append(pid)
            shutil.rmtree(proc / str(pid))
        namespace["os"].kill = fake_kill
        status = namespace["stop_gate"](record, proc)
        assert status == 0, status
        assert killed == \(supervisorMatches ? "[100, 200]" : "[200]"), killed
        assert (proc / "300").exists()
        assert (proc / "100").exists() == \(supervisorMatches ? "False" : "True")
        print("detached worker quiesced; reused PID and peer untouched")
        """
        let result = try RealProcessRunner().run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", code]))
        XCTAssertEqual(result.exitCode, 0, result.stderrString)
        XCTAssertTrue(result.stdoutString.contains("detached worker quiesced"))
        }
    }

}
