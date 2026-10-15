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
            if status != 255 {
                XCTAssertLessThan(events.firstIndex(of: "stream.stop")!, events.firstIndex(of: "harvest")!)
                XCTAssertLessThan(events.firstIndex(of: "harvest")!, events.firstIndex(of: "finish")!)
            }
            if status == 255 {
                XCTAssertTrue(events.contains("quiesce"))
                XCTAssertTrue(events.contains("harvest"))
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

    func testDroppedFollowResumesWithTheRemoteVerdictAndOnlyOneLaunch() throws {
        for finalStatus: Int32 in [0, 1] {
            let fixture = try Fixture(status: finalStatus)
            defer { fixture.remove() }
            fixture.runner.followStatuses = [9, 75, finalStatus]
            fixture.runner.followChunks = ["GATE: ", "PASS ", "(1/1 boot tests passed)\n"].map { Data($0.utf8) }
            let result = try fixture.launcher.runX86(options: fixture.options)
            XCTAssertEqual(result.manifest.verdict.isFailure, finalStatus != 0)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "launch" }.count, 1)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "gate.end" }.count, 3)
            XCTAssertFalse(fixture.runner.events.contains("quiesce"))
            XCTAssertTrue(fixture.runner.events.contains("harvest"))
            let directory = try XCTUnwrap(result.runDirectory)
            let records = try String(contentsOf: directory.appendingPathComponent("reconnects.jsonl"), encoding: .utf8)
            XCTAssertTrue(records.contains("\"operation\":\"gate\""))
            XCTAssertTrue(records.contains("\"exit_code\":9"))
            let output = try String(contentsOf: directory.appendingPathComponent("gate-stdout.txt"), encoding: .utf8)
            XCTAssertEqual(output, "GATE: PASS (1/1 boot tests passed)\n")
            XCTAssertEqual(fixture.runner.followOffsets, [0, 6, 11])
        }
    }

    func testSerialOnlyDisconnectReopensReaderAndKeepsOneRemoteGate() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.followStatuses = [75, 0]
        fixture.runner.dropFirstStream = true
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertFalse(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.stream.starts, 2)
        XCTAssertEqual(fixture.stream.stops, 2)
        XCTAssertEqual(fixture.runner.events.filter { $0 == "launch" }.count, 1)
        XCTAssertFalse(fixture.runner.events.contains("quiesce"))
    }

    func testLostLaunchAcknowledgementRetriesTheSameClaim() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.launchStatuses = [255, 0]
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertFalse(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.launchRequests.count, 2)
        XCTAssertEqual(fixture.runner.launchRequests.first, fixture.runner.launchRequests.last)
        XCTAssertFalse(fixture.runner.events.contains("quiesce"))
    }

    func testFinishedRemoteStatusIsNeverTreatedAsSSHFailure() throws {
        for status: Int32 in [9, 75, 124, 255] {
            let fixture = try Fixture(status: status)
            defer { fixture.remove() }
            fixture.runner.finalOwnerStatus = true
            let result = try fixture.launcher.runX86(options: fixture.options)
            XCTAssertTrue(result.manifest.verdict.isFailure)
            XCTAssertEqual(fixture.runner.finishStatus, String(status))
            XCTAssertEqual(fixture.runner.events.filter { $0 == "gate.end" }.count, 1)
            XCTAssertTrue(fixture.runner.events.contains("harvest"))
            XCTAssertFalse(fixture.runner.events.contains("quiesce"))
        }
    }

    func testExhaustedReconnectsLeaveRemoteGateAndEvidenceAlone() throws {
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

    func testOwnerDeathStopsHarvestsAndCleansWithoutReconnect() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.followStatuses = [76]
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.followOffsets.count, 1)
        for event in ["quiesce", "harvest", "remove"] { XCTAssertTrue(fixture.runner.events.contains(event)) }
    }

    func testSignalDuringLaunchRetriesStopTransportAndIgnoresSecondSignal() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.interruptLaunch = true
        fixture.runner.stopStatuses = [255, 0]
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.events.filter { $0 == "quiesce" }.count, 2)
        XCTAssertTrue(fixture.runner.events.contains("harvest"))
        XCTAssertTrue(fixture.runner.events.contains("remove"))
        let signals = X86LaunchSignals(runner: fixture.runner, installHandlers: false)
        signals.receive(SIGINT)
        fixture.runner.interrupted = false
        signals.receive(SIGINT)
        XCTAssertFalse(fixture.runner.interrupted)
    }

    func testLaunchExitOneRetriesAndInvalidRequestPreservesExitTwo() throws {
        for statuses: [Int32] in [[1, 75, 0], [2]] {
            let fixture = try Fixture(status: 0)
            defer { fixture.remove() }
            fixture.runner.launchStatuses = statuses
            let result = try fixture.launcher.runX86(options: fixture.options)
            XCTAssertEqual(fixture.runner.launchRequests.count, statuses.count)
            XCTAssertEqual(result.manifest.verdict.isFailure, statuses == [2])
            if statuses == [2] { XCTAssertEqual(fixture.runner.finishStatus, "2") }
        }
    }

    func testHealthyFollowResetsConsecutiveGateFailureBudget() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.followStatuses = [9, 75, 9, 75, 9, 75, 0]
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertFalse(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.followOffsets.count, 7)
    }

    func testSerialFailureCannotExhaustGateRetries() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.stream.isReading = false
        fixture.runner.followStatuses = [75, 75, 75, 75, 0]
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertFalse(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.followOffsets.count, 5)
    }

    func testRecoverStopsHarvestsAndRemovesPreviouslyUnreachableRun() throws {
        let fixture = try Fixture(status: 255)
        defer { fixture.remove() }
        fixture.runner.quiesceStatus = 1
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertEqual(result.manifest.env["BREENIX_REMOTE_QUIESCED"], "0")
        fixture.runner.quiesceStatus = 0
        try fixture.launcher.recoverX86(id: result.manifest.id)
        let recovered = try fixture.launcher.store.readManifest(id: result.manifest.id)
        XCTAssertEqual(recovered.env["BREENIX_REMOTE_QUIESCED"], "1")
        XCTAssertTrue(recovered.verdict.isFailure)
        XCTAssertTrue(fixture.runner.events.contains("harvest"))
        XCTAssertTrue(fixture.runner.events.contains("remove"))
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

    func testThrownFollowRetriesWithoutCancellingRemoteGate() throws {
        let fixture = try Fixture(status: 1)
        defer { fixture.remove() }
        fixture.runner.throwGate = true
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertEqual(fixture.runner.events.filter { $0 == "finish" }.count, 1)
        XCTAssertTrue(fixture.runner.events.contains("quiesce"))
        XCTAssertTrue(fixture.runner.events.contains("remove"))
        XCTAssertTrue(fixture.runner.events.contains("harvest"))
    }

    func testInterruptUnwindsOwnedStreamAndRetainsFailureStatus() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.interruptGate = true
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertEqual(fixture.runner.finishStatus, "130")
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.stream.stops, 1)
        XCTAssertTrue(fixture.runner.events.contains("quiesce"))
    }

    func testNoFramesStillRegistersAtLaunchAndFinishesExactlyOnceWithNonzeroStatus() throws {
        for status: Int32 in [130, 1, 255] {
            let fixture = try Fixture(status: status)
            defer { fixture.remove() }
            fixture.runner.sendsFrames = false
            fixture.runner.noBoots = true
            let result = try fixture.launcher.runX86(options: fixture.options)
            XCTAssertTrue(result.manifest.verdict.isFailure)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "start" }.count, 1)
            XCTAssertEqual(fixture.runner.events.filter { $0 == "finish" }.count, 1)
            XCTAssertFalse(fixture.runner.events.contains("record"))
            XCTAssertEqual(fixture.runner.finishStatus, String(status))
            XCTAssertLessThan(fixture.runner.events.firstIndex(of: "start")!, fixture.runner.events.firstIndex(of: "gate.end")!)
        }
    }

    func testFailedPullCannotLeavePassingManifestOrVigilStatus() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.pullStatus = 9
        let result = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertTrue(result.manifest.verdict.isFailure)
        XCTAssertEqual(fixture.runner.finishStatus, "1")
        XCTAssertFalse(fixture.runner.events.contains("remove"))
        XCTAssertTrue(try String(contentsOf: XCTUnwrap(result.runDirectory).appendingPathComponent("serial_user.txt"), encoding: .utf8).contains("live user"))
    }

    func testNoisyRegistrationResponseDoesNotCreateSecondRecord() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        fixture.runner.startNoise = true
        _ = try fixture.launcher.runX86(options: fixture.options)
        XCTAssertEqual(fixture.runner.events.filter { $0 == "finish" }.count, 1)
        XCTAssertFalse(fixture.runner.events.contains("record"))
    }

    func testLiveSequencePreservesEachSuiteRecordAndOverallGateStatus() throws {
        let fixture = try Fixture(status: 1)
        defer { fixture.remove() }
        fixture.runner.suites = "files-io,directories"
        var options = fixture.options
        options.suite = fixture.runner.suites
        _ = try fixture.launcher.runX86(options: options)
        XCTAssertEqual(fixture.runner.startArguments?[3], "")
        XCTAssertEqual(fixture.runner.events.filter { $0 == "record" }.count, 2)
        XCTAssertEqual(fixture.runner.finishStatus, "1")
    }

    func testEarlyStreamExitCancelsReadSourceWithoutSSH() throws {
        let stream = try SSHSerialStream(ProcessRequest(executable: "/usr/bin/true")) { _ in XCTFail("unexpected output") }
        defer { stream.stop() }
        let deadline = Date().addingTimeInterval(2)
        while stream.isReading && Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
        XCTAssertFalse(stream.isReading, "EOF must cancel the read source")
    }

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

    func testHeartbeatDeadlineWorksWithoutEOFWhenOutputPipeIsFull() throws {
        let fixture = try Fixture(status: 0)
        defer { fixture.remove() }
        let directory = fixture.root.appendingPathComponent("breenix_gate_1")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for stream in ["user", "kernel"] {
            try Data(repeating: 65, count: 8 * 1024 * 1024).write(to: directory.appendingPathComponent("serial_\(stream).log"))
        }
        let process = Process(), input = Pipe(), output = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
        process.arguments = ["-u", "-c", RemoteCommand.serialReader, fixture.root.path, "1"]
        process.standardInput = input; process.standardOutput = output
        try process.run()
        defer { if process.isRunning { process.terminate() }; process.waitUntilExit() }
        try input.fileHandleForWriting.write(contentsOf: Data([1]))
        let deadline = Date().addingTimeInterval(8)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        XCTAssertFalse(process.isRunning, "heartbeat must expire with stdin open and stdout blocked")
        if !process.isRunning { XCTAssertEqual(process.terminationStatus, 0) }
        try input.fileHandleForWriting.close()
        try output.fileHandleForReading.close()
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
        live.start()
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
        for supervisorMatches in [false, true] {
        let code = """
        import pathlib, json, shutil
        proc = pathlib.Path(\(String(reflecting: fixture.root.path))) / "proc"
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
    var starts = 0
    var isReading = true
    private var stopped = false
    var onStop: (() -> Void)?
    func start(_ request: ProcessRequest, receive: @escaping @Sendable (Data) -> Void) throws -> X86SerialStream {
        self.receive = receive
        starts += 1
        stopped = false
        isReading = true
        return self
    }
    func stop() { if !stopped { stops += 1; stopped = true; onStop?() }; receive = nil }
}

private final class LaunchRunner: ProcessRunner {
    var events: [String] = []
    var status: Int32
    var followStatuses: [Int32] = []
    var followChunks: [Data] = []
    var launchStatuses: [Int32] = []
    var launchRequests: [ProcessRequest] = []
    var dropFirstStream = false
    var finalOwnerStatus = false
    var followOffsets: [UInt64] = []
    var quiesceStatus: Int32 = 0
    var extractionStatus: Int32 = 0
    var throwGate = false
    var interruptGate = false
    var interruptLaunch = false
    var stopStatuses: [Int32] = []
    var registers = true
    var startNoise = false
    var sendsFrames = true
    var noBoots = false
    var pullStatus: Int32 = 0
    var onInterruptGate: (() -> Void)?
    var suites: String?
    var startArguments: [String]?
    var finishStatus: String?
    var stream: FakeStream
    var interrupted = false
    let lock = NSLock()
    init(status: Int32, stream: FakeStream) { self.status = status; self.stream = stream }
    func interrupt() { lock.lock(); interrupted = true; lock.unlock() }
    func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)?) throws -> ProcessResult {
        if request.executable == "/fake-vigil" {
            let action = request.arguments[safe: 0] ?? ""
            events.append(action)
            if action == "start" {
                startArguments = request.arguments
                return ProcessResult(stdout: registers ? Data(((startNoise ? "diagnostic\n" : "") + (request.arguments[safe: 7] ?? "") + "\n").utf8) : Data(), exitCode: 0)
            }
            if action == "finish" { finishStatus = request.arguments[safe: 2] }
            return ProcessResult(exitCode: 0)
        }
        if request.executable == "/usr/bin/tar" {
            if extractionStatus != 0 { return ProcessResult(exitCode: extractionStatus) }
            let root = URL(fileURLWithPath: request.arguments.last!).appendingPathComponent("gate-tmp")
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            if noBoots { return ProcessResult(exitCode: 0) }
            let directory = root.appendingPathComponent("breenix_gate_1")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            for serial in ["user", "kernel"] {
                try Data("final \(serial)\n".utf8).write(to: directory.appendingPathComponent("serial_\(serial).log"))
            }
            if let suites {
                let rows = suites.split(separator: ",").enumerated().map { index, suite in
                    ["suite": String(suite), "verdict": index == 0 ? "PASS" : "FAIL", "started": index * 10, "ended": (index + 1) * 10] as [String: Any]
                }
                try JSONSerialization.data(withJSONObject: rows).write(to: directory.appendingPathComponent("suite-results.json"))
            }
            return ProcessResult(exitCode: 0)
        }
        let remote = request.arguments.last ?? ""
        if remote.hasSuffix("# gate-start") {
            events.append("launch")
            launchRequests.append(request)
            if interruptLaunch { onInterruptGate?() }
            return ProcessResult(exitCode: launchStatuses.isEmpty ? 0 : launchStatuses.removeFirst())
        }
        if remote.hasSuffix("# gate-follow") {
            let offset = UInt64(remote.split(separator: " ").dropLast(3).last ?? "0") ?? 0
            followOffsets.append(offset)
            let next = followStatuses.isEmpty ? status : followStatuses.removeFirst()
            let chunk = followChunks.isEmpty ? ((next == 255 && !finalOwnerStatus) || throwGate ? Data() : Data("GATE: PASS (1/1 boot tests passed)\n".utf8)) : followChunks.removeFirst()
            if !finalOwnerStatus && (next == 9 || next == 75 || next == 76) {
                if dropFirstStream { stream.isReading = false; dropFirstStream = false }
                outputHandler?(chunk)
                events.append("gate.end")
                return ProcessResult(stdout: chunk, exitCode: next)
            }
            for serial in sendsFrames ? ["user", "kernel"] : [] {
                stream.receive?(frame(boot: 1, stream: serial, data: Data("live \(serial)".utf8)))
            }
            if let args = startArguments, sendsFrames {
                XCTAssertTrue(try String(contentsOfFile: args[5], encoding: .utf8).contains("live user"))
            }
            if interruptGate {
                onInterruptGate?()
                XCTAssertTrue(interrupted)
            }
            outputHandler?(chunk)
            events.append("gate.end")
            if throwGate { throw NSError(domain: "fake gate", code: 1) }
            return ProcessResult(stdout: chunk, stderr: (next == 255 && !finalOwnerStatus) || throwGate ? Data() : Data("BREENIX_GATE_FINISHED=\(next)\n".utf8), exitCode: next == 255 && !finalOwnerStatus ? 255 : 0)
        }
        if remote.contains("python3 -c 'import base64") { events.append("quiesce"); return ProcessResult(exitCode: stopStatuses.isEmpty ? quiesceStatus : stopStatuses.removeFirst()) }
        if remote.contains("tar -czf -") { events.append("harvest"); return ProcessResult(stdout: Data([1]), exitCode: pullStatus) }
        if remote.contains("rm -rf") && !remote.contains("git clone") { events.append("remove") }
        return ProcessResult(exitCode: 0)
    }
}

private struct Fixture {
    let root: URL
    let stream: FakeStream
    let runner: LaunchRunner
    var launcher: BeastLauncher
    let options: BeastLaunchOptions
    init(status: Int32) throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("live-x86-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        stream = FakeStream()
        runner = LaunchRunner(status: status, stream: stream)
        stream.onStop = { [runner] in runner.events.append("stream.stop") }
        launcher = BeastLauncher(store: RunStore(root: root), runner: runner, vigilScript: URL(fileURLWithPath: "/fake-vigil"), serialStreaming: stream, targetGuard: nil)
        launcher.reconnectLimit = 2
        launcher.reconnectDelay = { _ in }
        launcher.makeSignals = { [runner] _ in
            let signals = X86LaunchSignals(runner: runner, installHandlers: false)
            runner.onInterruptGate = { [weak signals] in signals?.receive(SIGINT) }
            return signals
        }
        options = BeastLaunchOptions(sha: String(repeating: "a", count: 40), runID: "live-test", qemuProfile: .q35, suite: "processes")
    }
    func remove() { try? FileManager.default.removeItem(at: root) }
}
