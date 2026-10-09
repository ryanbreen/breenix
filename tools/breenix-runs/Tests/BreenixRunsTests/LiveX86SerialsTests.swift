import Foundation
import Darwin
@testable import BreenixRuns
import XCTest

final class LiveX86SerialsTests: XCTestCase {
    func testLiveRegistrationAndAuthoritativeReplacementForSuccessAndFailure() throws {
        for status: Int32 in [0, 1, 255] {
            let fixture = try Fixture(status: status)
            defer { fixture.remove() }
            let result = try fixture.launcher.runX86(options: fixture.options)
            XCTAssertEqual(result.manifest.verdict.isFailure, status != 0)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "start" }.count, 1)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "finish" }.count, 1)
            XCTAssertFalse(fixture.runner.events.contains("record"))
            XCTAssertEqual(fixture.stream.stops, 1)
            let events = fixture.runner.events
            XCTAssertLessThan(events.firstIndex(of: "start")!, events.firstIndex(of: "gate.end")!)
            XCTAssertLessThan(events.firstIndex(of: "stream.stop")!, events.firstIndex(of: "harvest")!)
            XCTAssertLessThan(events.firstIndex(of: "harvest")!, events.firstIndex(of: "finish")!)
            if status == 255 {
                XCTAssertLessThan(events.firstIndex(of: "quiesce")!, events.firstIndex(of: "harvest")!)
            }
            let directory = try XCTUnwrap(result.runDirectory)
            for stream in ["user", "kernel"] {
                let expected = "==== breenix-runs boot 1: breenix_gate_1/serial_\(stream).log ====\nfinal \(stream)\n\n"
                XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("serial_\(stream).txt")), Data(expected.utf8))
            }
            XCTAssertEqual(fixture.runner.startArguments?[2], "suite")
            XCTAssertEqual(fixture.runner.startArguments?[3], "processes")
            XCTAssertEqual(fixture.runner.startArguments?[6], "q35")
            XCTAssertEqual(fixture.runner.startArguments?[7], fixture.options.runID)
            XCTAssertEqual(fixture.runner.startArguments?[8], fixture.options.sha)
            XCTAssertEqual(fixture.runner.finishStatus, String(status))
        }
    }

    func testDisconnectedLiveGateIsNeverHarvestedOrRemovedWithoutQuiescence() throws {
        let fixture = try Fixture(status: 255)
        defer { fixture.remove() }
        fixture.runner.quiesceStatus = 1
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertFalse(fixture.runner.events.contains("harvest"))
        XCTAssertFalse(fixture.runner.events.contains("remove"))
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertEqual(fixture.runner.finishStatus, "255")
        let serial = try String(contentsOf: XCTUnwrap(result.runDirectory).appendingPathComponent("serial_user.txt"), encoding: .utf8)
        XCTAssertTrue(serial.contains("live user"))
    }

    func testFailedExtractionPreservesLiveSerialsAndFinishesWithFailure() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.extractionStatus = 1
        XCTAssertThrowsError(try fixture.launcher.runX86(options: fixture.options))
        XCTAssertEqual(fixture.runner.finishStatus, "1")
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertFalse(fixture.runner.events.contains("remove"))
        let file = fixture.root.appendingPathComponent("runs/live-test/serial_user.txt")
        XCTAssertTrue(try String(contentsOf: file, encoding: .utf8).contains("live user"))
    }

    func testThrownGateStillStopsStreamQuiescesAndFinishesOnce() throws {
        let fixture = try Fixture(status: 1)
        defer { fixture.remove() }
        fixture.runner.throwGate = true
        XCTAssertThrowsError(try fixture.launcher.runX86(options: fixture.options))
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertEqual(fixture.runner.events.filter { $0 == "finish" }.count, 1)
        XCTAssertTrue(fixture.runner.events.contains("quiesce"))
        XCTAssertFalse(fixture.runner.events.contains("remove"))
    }

    func testInterruptUnwindsOwnedStreamAndRetainsFailureStatus() throws {
        let fixture = try Fixture(status: 9)
        defer { fixture.remove() }
        fixture.runner.interruptGate = true
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertEqual(fixture.runner.finishStatus, "130")
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertTrue(fixture.runner.events.contains("quiesce"))
    }

    func testFramesPreserveBinaryPartialLinesAndIterationSeparators() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        let live = try LiveX86Serials(directory: fixture.root, id: "id", options: fixture.options, script: nil, runner: fixture.runner)
        let binary = Data([0, 255, 13, 10, 65])
        let frames = frame(boot: 1, stream: "user", data: binary) + frame(boot: 1, stream: "user", data: Data([66])) + frame(boot: 2, stream: "user", data: Data([67]))
        for byte in frames { live.receive(Data([byte])) }
        live.finish(status: 0)
        live.receive(frame(boot: 3, stream: "user", data: Data([68])))
        let expected = Data("==== breenix-runs boot 1: breenix_gate_1/serial_user.log ====\n".utf8) + binary + Data("B\n==== breenix-runs boot 2: breenix_gate_2/serial_user.log ====\nC".utf8)
        XCTAssertEqual(try Data(contentsOf: fixture.root.appendingPathComponent("serial_user.txt")), expected)
    }

    func testUnavailableRegistrationDoesNotFinishOrClaimSuccess() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.registers = false
        let live = try LiveX86Serials(directory: fixture.root, id: "id", options: fixture.options, script: URL(fileURLWithPath: "/fake-vigil"), runner: fixture.runner)
        live.receive(frame(boot: 1, stream: "user", data: Data()))
        live.finish(status: 0)
        XCTAssertFalse(live.registered)
        XCTAssertFalse(fixture.runner.events.contains("finish"))
    }

    func testOwnedProcessExitsOnInputEOFAndStartFailureIsSafeWithoutSSH() throws {
        let received = expectation(description: "local fake process receives heartbeat")
        received.assertForOverFulfill = false
        let stream = try SSHSerialStreaming().start(ProcessRequest(executable: "/bin/cat")) { data in
            if !data.isEmpty { received.fulfill() }
        }
        wait(for: [received], timeout: 3)
        stream.stop()
        stream.stop()
        XCTAssertThrowsError(try SSHSerialStreaming().start(ProcessRequest(executable: "/does-not-exist")) { _ in })
    }

    func testDisconnectCleanupFindsDetachedWorkerAndExcludesReusedPIDAndPeer() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        let code = """
        import pathlib, json, shutil
        proc = pathlib.Path(\(String(reflecting: fixture.root.path))) / "proc"
        record = proc.parent / "gate-tmp" / "launcher-gate.json"
        record.parent.mkdir()
        record.write_text(json.dumps([100, "original-birth"]))
        for pid, birth, helper in [(100, "reused-birth", "/peer/host-slots.py"),
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
        assert killed == [200], killed
        assert (proc / "100").exists() and (proc / "300").exists()
        print("detached worker quiesced; reused PID and peer untouched")
        """
        let result = try RealProcessRunner().run(ProcessRequest(executable: "/usr/bin/python3", arguments: ["-c", code]))
        XCTAssertEqual(result.exitCode, 0, result.stderrString)
        XCTAssertTrue(result.stdoutString.contains("detached worker quiesced"))
    }

    func testRemoteReaderHandlesIterationGrowthAndStopsOnEOFWithoutSSH() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        let directory = fixture.root.appendingPathComponent("breenix_gate_1")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data("user".utf8).write(to: directory.appendingPathComponent("serial_user.log"))
        try Data("kernel".utf8).write(to: directory.appendingPathComponent("serial_kernel.log"))
        let runner = RealProcessRunner()
        // The fake transport feeds one heartbeat, then EOF. The exact remote reader runs locally.
        let code = "printf x | python3 -u -c " + shellQuote(RemoteCommand.serialReader) + " " + shellQuote(fixture.root.path) + " 1"
        let result = try runner.run(ProcessRequest(executable: "/bin/bash", arguments: ["-c", code]))
        XCTAssertEqual(result.exitCode, 0)
        XCTAssertTrue(result.stdoutString.contains("dXNlcg=="))
        XCTAssertTrue(result.stdoutString.contains("a2VybmVs"))
        let request = RemoteCommand.streamSerialsRequest(paths: BeastPaths(clonePath: "/root/owned"), boots: 2)
        XCTAssertFalse(request.combineOutput)
        XCTAssertTrue(request.arguments.last!.contains("incus exec breenix-x86"))
    }
}

private func shellQuote(_ value: String) -> String { "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'" }
private func frame(boot: Int, stream: String, data: Data) -> Data {
    try! JSONSerialization.data(withJSONObject: ["boot": boot, "stream": stream, "data": data.base64EncodedString()]) + Data([10])
}

private final class FakeStream: X86SerialStreaming, X86SerialStream {
    var receive: (@Sendable (Data) -> Void)?
    var stops = 0
    var onStop: (() -> Void)?
    func start(_ request: ProcessRequest, receive: @escaping @Sendable (Data) -> Void) throws -> X86SerialStream {
        self.receive = receive
        return self
    }
    func stop() { if stops == 0 { stops = 1; onStop?() }; receive = nil }
}

private final class LaunchRunner: ProcessRunner {
    var events: [String] = []
    var status: Int32
    var quiesceStatus: Int32 = 0
    var extractionStatus: Int32 = 0
    var throwGate = false
    var interruptGate = false
    var registers = true
    var startArguments: [String]?
    var finishStatus: String?
    var stream: FakeStream
    var interrupted = false
    let lock = NSLock()
    init(status: Int32, stream: FakeStream) { self.status = status; self.stream = stream }
    func interrupt() { lock.lock(); interrupted = true; lock.unlock() }
    func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)?) throws -> ProcessResult {
        if request.executable == "/fake-vigil" {
            let action = request.arguments[0]
            events.append(action)
            if action == "start" {
                startArguments = request.arguments
                return ProcessResult(stdout: registers ? Data((request.arguments[7] + "\n").utf8) : Data(), exitCode: 0)
            }
            if action == "finish" { finishStatus = request.arguments[2] }
            return ProcessResult(exitCode: 0)
        }
        if request.executable == "/usr/bin/tar" {
            if extractionStatus != 0 { return ProcessResult(exitCode: extractionStatus) }
            let directory = URL(fileURLWithPath: request.arguments.last!).appendingPathComponent("gate-tmp/breenix_gate_1")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            for serial in ["user", "kernel"] {
                try Data("final \(serial)\n".utf8).write(to: directory.appendingPathComponent("serial_\(serial).log"))
            }
            return ProcessResult(exitCode: 0)
        }
        let remote = request.arguments.last ?? ""
        if remote.contains("run-x86-gate.sh") {
            for serial in ["user", "kernel"] {
                stream.receive?(frame(boot: 1, stream: serial, data: Data("live \(serial)".utf8)))
            }
            if let args = startArguments {
                XCTAssertTrue(try String(contentsOfFile: args[5], encoding: .utf8).contains("live user"))
            }
            if interruptGate {
                kill(getpid(), SIGINT)
                let deadline = Date().addingTimeInterval(3)
                while Date() < deadline {
                    lock.lock(); let done = interrupted; lock.unlock()
                    if done { break }
                    Thread.sleep(forTimeInterval: 0.01)
                }
                lock.lock(); XCTAssertTrue(interrupted); lock.unlock()
            }
            events.append("gate.end")
            if throwGate { throw NSError(domain: "fake gate", code: 1) }
            return ProcessResult(stdout: Data("gate ended\n".utf8), exitCode: status)
        }
        if remote.contains("python3 -c 'import base64") { events.append("quiesce"); return ProcessResult(exitCode: quiesceStatus) }
        if remote.contains("tar -czf -") { events.append("harvest"); return ProcessResult(stdout: Data([1]), exitCode: 0) }
        if remote.contains("rm -rf") && !remote.contains("git clone") { events.append("remove") }
        return ProcessResult(exitCode: 0)
    }
}

private struct Fixture {
    let root: URL
    let stream: FakeStream
    let runner: LaunchRunner
    let launcher: BeastLauncher
    let options: BeastLaunchOptions
    init(status: Int32) throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("live-x86-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        stream = FakeStream()
        runner = LaunchRunner(status: status, stream: stream)
        stream.onStop = { [runner] in runner.events.append("stream.stop") }
        launcher = BeastLauncher(store: RunStore(root: root), runner: runner, vigilScript: URL(fileURLWithPath: "/fake-vigil"), serialStreaming: stream)
        options = BeastLaunchOptions(sha: String(repeating: "a", count: 40), runID: "live-test", qemuProfile: .q35, suite: "processes")
    }
    func remove() { try? FileManager.default.removeItem(at: root) }
}
