import Foundation
@testable import BreenixRuns
import XCTest

final class DetachedGateTests: XCTestCase {
    func testOwnerSurvivesLaunchExitAndDuplicateAcknowledgementsNeverRerunGate() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        // The owner runs on Linux; supply its process-birth input on this macOS host.
        let stat = root.appendingPathComponent("stat")
        try Data(("1 (python) S " + Array(repeating: "0", count: 18).joined(separator: " ") + " 42").utf8).write(to: stat)
        let owner = RemoteCommand.detachedOwner.replacingOccurrences(of: "/proc/self/stat", with: stat.path)
        let encoded = Data(owner.utf8).base64EncodedString()
        let owners = root.appendingPathComponent("owners")
        let evidence = root.appendingPathComponent("evidence")
        let script = "echo invocation >> '\(root.path)/invocations'; sleep 1; echo final-output; exit 7"
        let command = Data(script.utf8).base64EncodedString()
        let runner = RealProcessRunner()
        let launch = ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", owner, evidence.path, command, encoded, owners.path], timeoutSecs: 5)
        XCTAssertEqual(try runner.run(launch).exitCode, 0)
        let record = owners.appendingPathComponent("gate-owner.json")
        func state() throws -> String {
            let json = try JSONSerialization.jsonObject(with: Data(contentsOf: record)) as! [String: Any]
            return json["state"] as! String
        }
        XCTAssertEqual(try state(), "running", "the remote owner must outlive the launching SSH process")
        XCTAssertEqual(try runner.run(launch).exitCode, 0, "an uncertain launch acknowledgement is idempotent")
        let deadline = Date().addingTimeInterval(5)
        while try state() != "finished" && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        XCTAssertEqual(try state(), "finished")
        XCTAssertEqual(try runner.run(launch).exitCode, 0)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("invocations"), encoding: .utf8), "invocation\n")
        let follow = try runner.run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", RemoteCommand.gateFollower, evidence.path, "0", owners.path], timeoutSecs: 5))
        XCTAssertEqual(follow.exitCode, 0)
        XCTAssertEqual(follow.stderrString, "BREENIX_GATE_FINISHED=7\n")
        XCTAssertEqual(follow.stdoutString, "final-output\n")
        let resumed = try runner.run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", RemoteCommand.gateFollower, evidence.path, "6", owners.path], timeoutSecs: 5))
        XCTAssertEqual(resumed.exitCode, 0)
        XCTAssertEqual(resumed.stderrString, "BREENIX_GATE_FINISHED=7\n")
        XCTAssertEqual(resumed.stdoutString, "output\n")
        try FileManager.default.removeItem(at: evidence)
        XCTAssertEqual(try runner.run(launch).exitCode, 0)
        // Wait for the redundant bootstrap to observe the retained tombstone.
        Thread.sleep(forTimeInterval: 0.2)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("invocations"), encoding: .utf8), "invocation\n")
        XCTAssertFalse(FileManager.default.fileExists(atPath: evidence.path))

    }

    func testLostLaunchBeforeSpawnLeavesNoClaimAndRetryRunsOnce() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let stat = root.appendingPathComponent("stat")
        try Data(("1 (python) S " + Array(repeating: "0", count: 18).joined(separator: " ") + " 42").utf8).write(to: stat)
        let owner = RemoteCommand.detachedOwner.replacingOccurrences(of: "/proc/self/stat", with: stat.path)
        let delayed = owner.replacingOccurrences(of: "    child = subprocess.Popen([sys.executable", with: "    time.sleep(1)\n    child = subprocess.Popen([sys.executable")
        let command = Data("echo invocation >> '\(root.path)/invocations'".utf8).base64EncodedString()
        let runner = RealProcessRunner()
        let interrupted = try runner.run(ProcessRequest(executable: "/usr/bin/python3",
            arguments: ["-c", delayed, root.path, command, Data(owner.utf8).base64EncodedString()], timeoutSecs: 0.2))
        XCTAssertEqual(interrupted.exitCode, 124)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("gate-owner.json").path))
        let retry = try runner.run(ProcessRequest(executable: "/usr/bin/python3",
            arguments: ["-c", owner, root.path, command, Data(owner.utf8).base64EncodedString()], timeoutSecs: 5))
        XCTAssertEqual(retry.exitCode, 0)
        let deadline = Date().addingTimeInterval(5)
        var finished = false
        while Date() < deadline {
            let record = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("gate-owner.json"))) as! [String: Any]
            if record["state"] as? String == "finished" { finished = true; break }
            Thread.sleep(forTimeInterval: 0.05)
        }
        XCTAssertTrue(finished)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("invocations"), encoding: .utf8), "invocation\n")
    }

    func testSerialReconnectDiscardsPartialFramesAndDeduplicatesReplayAcrossBoots() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let live = try LiveX86Serials(directory: root, id: "replay", options: BeastLaunchOptions(sha: "abc"), script: nil, runner: RealProcessRunner())
        func frame(_ boot: Int, _ offset: Int, _ text: String) throws -> Data {
            try JSONSerialization.data(withJSONObject: ["boot": boot, "stream": "user", "offset": offset, "data": Data(text.utf8).base64EncodedString()]) + Data([10])
        }
        live.receive(try frame(1, 0, "first"))
        live.receive(Data("{partial".utf8))
        live.resetConnection()
        live.receive(try frame(1, 0, "first-second"))
        live.receive(try frame(2, 0, "next"))
        live.resetConnection()
        live.receive(try frame(1, 0, "first-second"))
        live.receive(try frame(2, 0, "next-last"))
        live.finish(status: 0)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("serial_user.txt"), encoding: .utf8),
            "==== breenix-runs boot 1: breenix_gate_1/serial_user.log ====\nfirst-second\n==== breenix-runs boot 2: breenix_gate_2/serial_user.log ====\nnext-last")
    }

    func testLANChangesOnlySSHTransport() throws {
        var lan = BeastPaths(clonePath: "/root/run")
        lan.host = "beast-lan"
        let tunnel = BeastPaths(clonePath: "/root/run")
        let lhs = RemoteCommand.plan(sha: "abc", boots: 1, mode: .full, timeoutSecs: 180, paths: tunnel, suite: "smoke")
        let rhs = RemoteCommand.plan(sha: "abc", boots: 1, mode: .full, timeoutSecs: 180, paths: lan, suite: "smoke")
        for (a, b) in [(lhs.prepareClone, rhs.prepareClone), (lhs.runGate, rhs.runGate), (lhs.pullEvidence, rhs.pullEvidence), (lhs.removeClone, rhs.removeClone),
                       (RemoteCommand.startGateRequest(gate: lhs.runGate, paths: tunnel), RemoteCommand.startGateRequest(gate: rhs.runGate, paths: lan)),
                       (RemoteCommand.followGateRequest(paths: tunnel, offset: 12), RemoteCommand.followGateRequest(paths: lan, offset: 12)),
                       (RemoteCommand.streamSerialsRequest(paths: tunnel, boots: 1), RemoteCommand.streamSerialsRequest(paths: lan, boots: 1))] {
            var expected = a
            expected.arguments = a.arguments.map { $0 == "beast" ? "beast-lan" : $0 }
            XCTAssertEqual(expected, b)
        }
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")), pathsTemplate: lan)
        XCTAssertNoThrow(try launcher.plan(options: BeastLaunchOptions(sha: "abc", suite: "smoke")))
    }
}
