import Foundation
import Darwin

/// A stream is an owned child: stop returns only after it has exited and callbacks drained.
public protocol X86SerialStream: AnyObject {
    func stop()
}
public protocol X86SerialStreaming {
    func start(_ request: ProcessRequest, receive: @escaping @Sendable (Data) -> Void) throws -> X86SerialStream
}

public struct SSHSerialStreaming: X86SerialStreaming {
    public init() {}
    public func start(_ request: ProcessRequest, receive: @escaping @Sendable (Data) -> Void) throws -> X86SerialStream {
        try SSHSerialStream(request, receive: receive)
    }
}

final class SSHSerialStream: X86SerialStream, @unchecked Sendable {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let queue = DispatchQueue(label: "breenix.x86.serial-stream")
    private let timer: DispatchSourceTimer
    private let reader: DispatchSourceRead
    private var stopped = false
    private var launched = false

    init(_ request: ProcessRequest, receive: @escaping @Sendable (Data) -> Void) throws {
        timer = DispatchSource.makeTimerSource(queue: queue)
        reader = DispatchSource.makeReadSource(fileDescriptor: output.fileHandleForReading.fileDescriptor, queue: queue)
        process.executableURL = URL(fileURLWithPath: request.executable)
        process.arguments = request.arguments
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        // A disconnected SSH must not deliver SIGPIPE to the launcher.
        _ = fcntl(input.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
        _ = fcntl(input.fileHandleForWriting.fileDescriptor, F_SETFL, O_NONBLOCK)
        do { try process.run(); launched = true } catch {
            timer.resume(); reader.resume()
            timer.cancel(); reader.cancel()
            throw error
        }
        reader.setEventHandler { [weak self, output] in
            let data = output.fileHandleForReading.availableData
            guard !data.isEmpty else {
                self?.reader.cancel()
                self?.timer.cancel()
                return
            }
            receive(data)
        }
        timer.setEventHandler { [input] in
            var heartbeat: UInt8 = 1
            _ = Darwin.write(input.fileHandleForWriting.fileDescriptor, &heartbeat, 1)
        }
        timer.schedule(deadline: .now(), repeating: .seconds(1))
        reader.resume()
        timer.resume()
    }

    var isReading: Bool { queue.sync { !reader.isCancelled } }

    func stop() {
        let shouldStop = queue.sync {
            guard !stopped else { return false }
            stopped = true
            timer.cancel()
            try? input.fileHandleForWriting.close()
            return true
        }
        guard shouldStop else { return }
        guard launched else {
            queue.sync { reader.cancel(); try? output.fileHandleForReading.close() }
            return
        }
        // EOF stops the remote reader; its heartbeat deadline also bounds a lost transport.
        let deadline = Date().addingTimeInterval(7)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning { process.terminate() }
        let killDeadline = Date().addingTimeInterval(1)
        while process.isRunning && Date() < killDeadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning { kill(process.processIdentifier, SIGKILL) }
        process.waitUntilExit()
        queue.sync {
            reader.cancel()
            try? output.fileHandleForReading.close()
        }
    }
    deinit { stop() }
}

/// JSON/base64 framing keeps the two serial byte streams separate, including partial lines.
/// The gate never consumes these files: harvest replaces them after the stream is stopped.
final class LiveX86Serials: @unchecked Sendable {
    private struct Frame: Decodable { var boot: Int; var stream: String; var data: String }
    private let lock = NSLock()
    private var pending = Data()
    private var bootByStream: [String: Int] = [:]
    private var handles: [String: FileHandle] = [:]
    private var registeredID: String?
    private var finished = false
    private let runner: ProcessRunner
    private let script: URL?
    private let options: BeastLaunchOptions
    private let id: String
    private let directory: URL

    init(directory: URL, id: String, options: BeastLaunchOptions, script: URL?, runner: ProcessRunner) throws {
        self.directory = directory; self.id = id; self.options = options; self.script = script; self.runner = runner
        for stream in ["user", "kernel"] {
            let url = directory.appendingPathComponent("serial_\(stream).txt")
            FileManager.default.createFile(atPath: url.path, contents: nil)
            handles[stream] = try FileHandle(forWritingTo: url)
        }
    }

    var registered: Bool {
        lock.lock(); defer { lock.unlock() }
        return registeredID != nil
    }

    // Registration is synchronous at gate launch, outside the frame lock, and bounded.
    func start() {
        guard let script else { return }
        let result = try? runner.run(ProcessRequest(executable: script.path, arguments: [
            "start", "beast", options.suite == nil || options.suite?.contains(",") == true ? "tests" : "suite",
            options.suite?.contains(",") == true ? "" : options.suite ?? "",
            directory.appendingPathComponent("serial_kernel.txt").path,
            directory.appendingPathComponent("serial_user.txt").path,
            (options.qemuProfile ?? .default).rawValue, id, options.sha
        ], environment: ["BREENIX_LAUNCHER_PID": String(getpid())], timeoutSecs: 10))
        // A diagnostic line must not cause duplicate registration. The requested id
        // is also used by the idempotent finished fallback after an uncertain start.
        if let result, result.exitCode == 0, result.stdoutString.split(whereSeparator: \.isNewline).contains(Substring(id)) {
            registeredID = id
        }
    }

    func receive(_ bytes: Data) {
        lock.lock(); defer { lock.unlock() }
        guard !finished else { return }
        pending.append(bytes)
        while let newline = pending.firstIndex(of: 10) {
            let line = pending.prefix(upTo: newline)
            pending.removeSubrange(...newline)
            guard let frame = try? JSONDecoder().decode(Frame.self, from: line), frame.boot > 0,
                  let handle = handles[frame.stream], let data = Data(base64Encoded: frame.data) else { continue }
            if bootByStream[frame.stream] != frame.boot {
                if bootByStream[frame.stream] != nil { handle.write(Data("\n".utf8)) }
                handle.write(Data("==== breenix-runs boot \(frame.boot): breenix_gate_\(frame.boot)/serial_\(frame.stream).log ====\n".utf8))
                bootByStream[frame.stream] = frame.boot
            }
            handle.write(data)
        }
    }

    func finish(status: Int32) {
        lock.lock()
        guard !finished else { lock.unlock(); return }
        finished = true
        for handle in handles.values { try? handle.close() }
        handles.removeAll()
        lock.unlock()
        if let registeredID, let script {
            _ = try? runner.run(ProcessRequest(executable: script.path, arguments: ["finish", registeredID, String(status)], timeoutSecs: 10))
        }
    }
}

/// Catchable signals unwind the launcher instead of abandoning its stream and gate SSH children.
final class X86LaunchSignals: @unchecked Sendable {
    private let sources: [DispatchSourceSignal]
    private let previous: [sig_t?]
    private let lock = NSLock()
    private let runner: ProcessRunner
    private var received: Int32?
    var signalNumber: Int32? { lock.lock(); defer { lock.unlock() }; return received }

    func receive(_ number: Int32) {
        lock.lock(); received = number; lock.unlock()
        runner.interrupt()
    }

    init(runner: ProcessRunner, installHandlers: Bool = true) {
        self.runner = runner
        let numbers: [Int32] = [SIGINT, SIGTERM, SIGHUP]
        if !installHandlers { previous = []; sources = []; return }
        previous = numbers.map { Darwin.signal($0, SIG_IGN) }
        sources = numbers.map { DispatchSource.makeSignalSource(signal: $0, queue: .global()) }
        for (source, number) in zip(sources, numbers) {
            source.setEventHandler { [weak self] in
                guard let self else { return }
                self.receive(number)
            }
            source.resume()
        }
    }
    deinit {
        for (index, number) in [SIGINT, SIGTERM, SIGHUP].prefix(sources.count).enumerated() {
            sources[index].cancel()
            _ = Darwin.signal(number, previous[index])
        }
    }
}
