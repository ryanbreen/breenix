import Foundation

public struct ProcessRequest: Equatable {
    public var executable: String
    public var arguments: [String]
    public var environment: [String: String]
    public var workingDirectory: URL?
    public var combineOutput: Bool

    public init(
        executable: String,
        arguments: [String] = [],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        combineOutput: Bool = false
    ) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
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
    func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)?) throws -> ProcessResult
}

public extension ProcessRunner {
    func run(_ request: ProcessRequest) throws -> ProcessResult {
        try run(request, outputHandler: nil)
    }
}

public final class RealProcessRunner: ProcessRunner {
    public init() {}

    public func run(_ request: ProcessRequest, outputHandler: ((Data) -> Void)? = nil) throws -> ProcessResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: request.executable)
        process.arguments = request.arguments
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
            guard !data.isEmpty else {
                return
            }
            stdoutBuffer.append(data)
            outputHandler?(data)
        }

        if !request.combineOutput {
            stderrPipe.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                guard !data.isEmpty else {
                    return
                }
                stderrBuffer.append(data)
            }
        }

        try process.run()
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

    private func appendRemaining(from pipe: Pipe, to buffer: LockedBuffer, outputHandler: ((Data) -> Void)?) {
        let data = pipe.fileHandleForReading.availableData
        if !data.isEmpty {
            buffer.append(data)
            outputHandler?(data)
        }
    }
}

private final class LockedBuffer {
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
