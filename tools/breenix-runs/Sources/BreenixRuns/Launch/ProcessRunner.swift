import Foundation
import Darwin

public struct ProcessRequest: Equatable {
    public var executable: String
    public var arguments: [String]
    public var environment: [String: String]
    public var workingDirectory: URL?
    public var timeoutSecs: Double?
    public var combineOutput: Bool

    public init(
        executable: String,
        arguments: [String] = [],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        combineOutput: Bool = false,
        timeoutSecs: Double? = nil
    ) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.timeoutSecs = timeoutSecs
        self.combineOutput = combineOutput
    }
}

public struct ProcessResult: Equatable {
    public var stdout: Data
    public var stderr: Data
    public var exitCode: Int32

    public init(stdout: Data = Data(), stderr: Data = Data(), exitCode: Int32) {
        self.stdout = stdout
        self.stderr = stderr
        self.exitCode = exitCode
    }

    public var stdoutString: String {
        String(decoding: stdout, as: UTF8.self)
    }

    public var stderrString: String {
        String(decoding: stderr, as: UTF8.self)
    }
}

public protocol ProcessRunner {
    func interrupt()
    // `@Sendable`: the handler is invoked from Pipe's readabilityHandler, which
    // fires on a GCD dispatch queue Foundation owns, never the caller's thread.
    func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)?) throws -> ProcessResult
}

public extension ProcessRunner {
    func interrupt() {}
    func run(_ request: ProcessRequest) throws -> ProcessResult {
        try run(request, outputHandler: nil)
    }
}

public final class RealProcessRunner: ProcessRunner, @unchecked Sendable {
    private let processLock = NSLock()
    private var active: [Process] = []

    public func interrupt() {
        processLock.lock(); defer { processLock.unlock() }
        for process in active where process.isRunning { kill(process.processIdentifier, SIGTERM) }
    }
    public init() {}

    public func run(_ request: ProcessRequest, outputHandler: (@Sendable (Data) -> Void)? = nil) throws -> ProcessResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: request.executable)
        process.arguments = request.arguments
        if let timeout = request.timeoutSecs {
            // Own a process group so a timed-out script cannot leave its CLI child
            // holding the output pipes open. All output still uses the normal runner.
            process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
            process.arguments = ["-c", Self.boundedProcess, String(timeout), request.executable] + request.arguments
        }
        if let workingDirectory = request.workingDirectory {
            process.currentDirectoryURL = workingDirectory
        }

        var environment = ProcessInfo.processInfo.environment
        for (key, value) in request.environment {
            environment[key] = value
        }
        process.environment = environment

        let stdoutPipe = Pipe()
        let stderrPipe = request.combineOutput ? stdoutPipe : Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        let stdoutBuffer = LockedBuffer()
        let stderrBuffer = request.combineOutput ? stdoutBuffer : LockedBuffer()

        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else { handle.readabilityHandler = nil; return }
            stdoutBuffer.append(data)
            outputHandler?(data)
        }

        if !request.combineOutput {
            stderrPipe.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                guard !data.isEmpty else { handle.readabilityHandler = nil; return }
                stderrBuffer.append(data)
            }
        }

        processLock.lock()
        do {
            try process.run()
            active.append(process)
            processLock.unlock()
        } catch {
            processLock.unlock()
            throw error
        }
        defer {
            processLock.lock()
            active.removeAll { $0 === process }
            processLock.unlock()
        }
        process.waitUntilExit()

        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        appendRemaining(from: stdoutPipe, to: stdoutBuffer, outputHandler: outputHandler)
        if !request.combineOutput {
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            appendRemaining(from: stderrPipe, to: stderrBuffer, outputHandler: nil)
        }

        return ProcessResult(
            stdout: stdoutBuffer.data,
            stderr: request.combineOutput ? Data() : stderrBuffer.data,
            exitCode: process.terminationStatus
        )
    }

    private static let boundedProcess = #"""
import os, signal, subprocess, sys
child = subprocess.Popen(sys.argv[2:], start_new_session=True)
def stop(number, frame):
    try: os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError: pass
    child.wait()
    sys.exit(128 + number)
for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
    signal.signal(number, stop)
try:
    status = child.wait(timeout=float(sys.argv[1]))
except subprocess.TimeoutExpired:
    try: os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError: pass
    child.wait()
    status = 124
try: os.killpg(child.pid, signal.SIGKILL)
except ProcessLookupError: pass
sys.exit(status if status >= 0 else 128 - status)
"""#

    private func appendRemaining(from pipe: Pipe, to buffer: LockedBuffer, outputHandler: (@Sendable (Data) -> Void)?) {
        let data = pipe.fileHandleForReading.availableData
        if !data.isEmpty {
            buffer.append(data)
            outputHandler?(data)
        }
    }
}

// All mutable state (`storage`) is only ever touched while holding `lock`, in
// both `append` and the `data` getter, so a `LockedBuffer` may be shared
// across the readabilityHandler's GCD queue and the caller's thread without a
// data race -- the precondition `@unchecked Sendable` exists for.
private final class LockedBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = Data()

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func append(_ data: Data) {
        lock.lock()
        storage.append(data)
        lock.unlock()
    }
}
