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
            .replacingOccurrences(of: "/proc/sys/kernel/random/boot_id", with: stat.path)
            .replacingOccurrences(of: "\"/proc/%d/stat\" % pid", with: String(reflecting: stat.path))
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
            .replacingOccurrences(of: "/proc/sys/kernel/random/boot_id", with: stat.path)
            .replacingOccurrences(of: "\"/proc/%d/stat\" % pid", with: String(reflecting: stat.path))
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

    func testUnsupportedLANAliasIsRejected() throws {
        let launcher = BeastLauncher(store: RunStore(root: URL(fileURLWithPath: "/unused")),
                                     pathsTemplate: BeastPaths(host: "beast-lan", clonePath: ""))
        XCTAssertThrowsError(try launcher.plan(options: BeastLaunchOptions(sha: "abc")))
    }

    func testFollowerRejectsMissingDeadZombieReusedAndRebootedOwners() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let stat = root.appendingPathComponent("stat")
        let boot = root.appendingPathComponent("boot")
        try Data("current".utf8).write(to: boot)
        let follower = RemoteCommand.gateFollower
            .replacingOccurrences(of: "\"/proc/%d/stat\" % owner[\"pid\"]", with: String(reflecting: stat.path))
            .replacingOccurrences(of: "/proc/sys/kernel/random/boot_id", with: boot.path)
            .replacingOccurrences(of: "time.monotonic() + 60", with: "time.monotonic()")
        let request = ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", follower, root.path, "0"], timeoutSecs: 2)
        let runner = RealProcessRunner()
        XCTAssertEqual(try runner.run(request).exitCode, 76)
        for (state, birth, bootID, exists, expected) in [("S", "42", "current", false, 76), ("Z", "42", "current", true, 76), ("S", "43", "current", true, 76), ("S", "42", "old", true, 76), ("S", "42", "current", true, 75)] {
            if exists { try Data(("1 (python) \(state) " + Array(repeating: "0", count: 18).joined(separator: " ") + " \(birth)").utf8).write(to: stat) }
            try JSONSerialization.data(withJSONObject: ["state": "running", "pid": 1, "birth": "42", "boot_id": bootID]).write(to: root.appendingPathComponent("gate-owner.json"))
            XCTAssertEqual(try runner.run(request).exitCode, Int32(expected))
        }
    }

    func testCancelBeforeOwnerPublicationPreventsDelayedGateLaunch() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let proc = root.appendingPathComponent("proc")
        try FileManager.default.createDirectory(at: proc.appendingPathComponent("sys/kernel/random"), withIntermediateDirectories: true)
        try Data("boot".utf8).write(to: proc.appendingPathComponent("sys/kernel/random/boot_id"))
        let owners = root.appendingPathComponent("owners")
        let evidence = root.appendingPathComponent("evidence")
        let stopper = RemoteCommand.gateStopper.replacingOccurrences(of: "pathlib.Path(\"/proc\")", with: "pathlib.Path(\"\(proc.path)\")")
        XCTAssertEqual(try RealProcessRunner().run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", stopper, evidence.path, owners.path])).exitCode, 0)
        let stat = root.appendingPathComponent("stat")
        try Data(("1 (python) S " + Array(repeating: "0", count: 18).joined(separator: " ") + " 42").utf8).write(to: stat)
        let owner = RemoteCommand.detachedOwner.replacingOccurrences(of: "/proc/self/stat", with: stat.path)
            .replacingOccurrences(of: "/proc/sys/kernel/random/boot_id", with: proc.appendingPathComponent("sys/kernel/random/boot_id").path)
        let invocation = root.appendingPathComponent("invocation")
        let command = Data("touch '\(invocation.path)'".utf8).base64EncodedString()
        let runner = RealProcessRunner()
        XCTAssertEqual(try runner.run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", owner, evidence.path, command, Data(owner.utf8).base64EncodedString(), owners.path])).exitCode, 0)
        Thread.sleep(forTimeInterval: 0.2)
        XCTAssertFalse(FileManager.default.fileExists(atPath: invocation.path))
    }

    func testTerminalTombstonesArePrunedButRunningAndRecentClaimsRemain() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        for (name, state, old) in [("breenix-old", "finished", true), ("breenix-active", "running", true), ("breenix-recent", "finished", false)] {
            let directory = root.appendingPathComponent(name)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let record = directory.appendingPathComponent("gate-owner.json")
            try JSONSerialization.data(withJSONObject: ["state": state]).write(to: record)
            if old { try FileManager.default.setAttributes([.modificationDate: Date().addingTimeInterval(-8 * 86400)], ofItemAtPath: record.path) }
        }
        let boot = root.appendingPathComponent("boot")
        try Data("boot".utf8).write(to: boot)
        // Stop before spawning: this test isolates the production pruning body.
        let owner = RemoteCommand.detachedOwner
            .replacingOccurrences(of: "/proc/sys/kernel/random/boot_id", with: boot.path)
            .replacingOccurrences(of: "    child = subprocess.Popen([sys.executable", with: "    sys.exit(0)\n    child = subprocess.Popen([sys.executable")
        let request = ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", owner, root.appendingPathComponent("evidence").path, "unused", "unused", root.appendingPathComponent("breenix-current").path])
        XCTAssertEqual(try RealProcessRunner().run(request).exitCode, 0)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("breenix-old").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("breenix-active").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("breenix-recent").path))
    }

    func testRunnerWaitsForSlowLastCallbackBeforeReturning() throws {
        let output = CallbackCapture()
        let result = try RealProcessRunner().run(ProcessRequest(executable: "/bin/sh", arguments: ["-c", "printf chunk; exit 75"])) { data in
            Thread.sleep(forTimeInterval: 0.1)
            output.append(data)
        }
        XCTAssertEqual(result.exitCode, 75)
        XCTAssertEqual(output.data, Data("chunk".utf8))
        XCTAssertEqual(result.stdout, output.data)
    }
}

private final class CallbackCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = Data()
    var data: Data { lock.lock(); defer { lock.unlock() }; return storage }
    func append(_ bytes: Data) { lock.lock(); defer { lock.unlock() }; storage.append(bytes) }
}
