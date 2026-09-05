import Foundation

public struct HostFactsTrace: Codable, Equatable, Sendable {
    public var start: HostFactsSample
    public var end: HostFactsSample

    public init(start: HostFactsSample, end: HostFactsSample) {
        self.start = start
        self.end = end
    }
}

public struct HostFactsSample: Codable, Equatable, Sendable {
    public var wallTime: Date
    public var qemuPeersAarch64: Int
    public var qemuPeersX86_64: Int
    public var loadavg1: Double?
    public var loadavg5: Double?
    public var loadavg15: Double?
    public var qemuCPUSeconds: Double?
    public var thermalPressure: String?
    public var hostModel: String?
    public var physMem: UInt64?
    public var qemuVersion: String?
    public var gitSHA: String?
    public var gitDirty: Bool?
    public var clockRatio: Double?

    public init(
        wallTime: Date,
        qemuPeersAarch64: Int,
        qemuPeersX86_64: Int,
        loadavg1: Double? = nil,
        loadavg5: Double? = nil,
        loadavg15: Double? = nil,
        qemuCPUSeconds: Double? = nil,
        thermalPressure: String? = nil,
        hostModel: String? = nil,
        physMem: UInt64? = nil,
        qemuVersion: String? = nil,
        gitSHA: String? = nil,
        gitDirty: Bool? = nil,
        clockRatio: Double? = nil
    ) {
        self.wallTime = wallTime
        self.qemuPeersAarch64 = qemuPeersAarch64
        self.qemuPeersX86_64 = qemuPeersX86_64
        self.loadavg1 = loadavg1
        self.loadavg5 = loadavg5
        self.loadavg15 = loadavg15
        self.qemuCPUSeconds = qemuCPUSeconds
        self.thermalPressure = thermalPressure
        self.hostModel = hostModel
        self.physMem = physMem
        self.qemuVersion = qemuVersion
        self.gitSHA = gitSHA
        self.gitDirty = gitDirty
        self.clockRatio = clockRatio
    }
}

// The launcher's OWN sample of the host at start/end of a run, kept distinct
// from the guest-annotated GATE_BOOT_FACTS record a kernel emits into its own
// serial (DESIGN.md Sec 5.3: "never merged into a GATE_BOOT_FACTS row"). Every
// value is read through `runner` (an injected ProcessRunner) rather than a
// native macOS API so HostFactsTests can assert against fixture strings with
// no real processes spawned.
public enum HostFacts {
    public static func sample(
        runner: ProcessRunner,
        repoRoot: URL,
        wallTime: Date = Date(),
        qemuPID: Int? = nil
    ) throws -> HostFactsSample {
        let aarch64Peers = try qemuPeerCount(processName: "qemu-system-aarch64", runner: runner)
        let x86Peers = try qemuPeerCount(processName: "qemu-system-x86_64", runner: runner)
        let load = try loadAverage(runner: runner)
        let qemuCPUSeconds: Double?
        if let qemuPID {
            qemuCPUSeconds = try HostFacts.qemuCPUSeconds(pid: qemuPID, runner: runner)
        } else {
            qemuCPUSeconds = nil
        }
        let host = try hostIdentity(runner: runner)
        let git = try gitIdentity(repoRoot: repoRoot, runner: runner)

        return HostFactsSample(
            wallTime: wallTime,
            qemuPeersAarch64: aarch64Peers,
            qemuPeersX86_64: x86Peers,
            loadavg1: load?.0,
            loadavg5: load?.1,
            loadavg15: load?.2,
            qemuCPUSeconds: qemuCPUSeconds,
            thermalPressure: try thermalPressure(runner: runner),
            hostModel: host.model,
            physMem: host.physMem,
            qemuVersion: try qemuVersion(runner: runner),
            gitSHA: git.sha,
            gitDirty: git.dirty,
            clockRatio: nil
        )
    }

    static func qemuPeerCount(processName: String, runner: ProcessRunner) throws -> Int {
        let result = try runner.run(ProcessRequest(executable: "/usr/bin/pgrep", arguments: ["-c", processName]))
        return parseInt(result.stdoutString) ?? 0
    }

    static func loadAverage(runner: ProcessRunner) throws -> (Double, Double, Double)? {
        let result = try runner.run(ProcessRequest(executable: "/usr/sbin/sysctl", arguments: ["-n", "vm.loadavg"]))
        return parseLoadAverage(result.stdoutString)
    }

    static func qemuCPUSeconds(pid: Int, runner: ProcessRunner) throws -> Double? {
        let result = try runner.run(ProcessRequest(executable: "/bin/ps", arguments: ["-o", "time=", "-p", "\(pid)"]))
        return parseProcessTime(result.stdoutString)
    }

    static func thermalPressure(runner: ProcessRunner) throws -> String? {
        let result = try runner.run(ProcessRequest(executable: "/usr/bin/pmset", arguments: ["-g", "therm"]))
        let value = result.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)
        if result.exitCode != 0 || value.isEmpty {
            return nil
        }
        return value
    }

    static func hostIdentity(runner: ProcessRunner) throws -> (model: String?, physMem: UInt64?) {
        let result = try runner.run(ProcessRequest(executable: "/usr/sbin/sysctl", arguments: ["hw.model", "hw.memsize"]))
        var model: String?
        var physMem: UInt64?
        for line in result.stdoutString.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2 else {
                continue
            }
            let key = parts[0].trimmingCharacters(in: .whitespaces)
            let value = parts[1].trimmingCharacters(in: .whitespaces)
            if key == "hw.model" {
                model = value
            } else if key == "hw.memsize" {
                physMem = UInt64(value)
            }
        }
        return (model, physMem)
    }

    static func qemuVersion(runner: ProcessRunner) throws -> String? {
        let result = try runner.run(ProcessRequest(executable: "/usr/bin/env", arguments: ["qemu-system-aarch64", "--version"]))
        let value = result.stdoutString.split(whereSeparator: \.isNewline).first.map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if result.exitCode != 0 || value?.isEmpty != false {
            return nil
        }
        return value
    }

    static func gitIdentity(repoRoot: URL, runner: ProcessRunner) throws -> (sha: String?, dirty: Bool?) {
        let shaResult = try runner.run(ProcessRequest(
            executable: "/usr/bin/git",
            arguments: ["rev-parse", "HEAD"],
            workingDirectory: repoRoot
        ))
        let statusResult = try runner.run(ProcessRequest(
            executable: "/usr/bin/git",
            arguments: ["status", "--porcelain"],
            workingDirectory: repoRoot
        ))

        let sha = shaResult.exitCode == 0 ? nonEmptyTrimmed(shaResult.stdoutString) : nil
        let dirty = statusResult.exitCode == 0 ? !statusResult.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty : nil
        return (sha, dirty)
    }

    static func parseLoadAverage(_ text: String) -> (Double, Double, Double)? {
        let cleaned = text.replacingOccurrences(of: "{", with: " ")
            .replacingOccurrences(of: "}", with: " ")
        let values = cleaned.split(whereSeparator: \.isWhitespace).compactMap { Double($0) }
        guard values.count >= 3 else {
            return nil
        }
        return (values[0], values[1], values[2])
    }

    static func parseProcessTime(_ text: String) -> Double? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return nil
        }

        let daySplit = trimmed.split(separator: "-", maxSplits: 1).map(String.init)
        let daySeconds: Double
        let timePart: String
        if daySplit.count == 2, let days = Double(daySplit[0]) {
            daySeconds = days * 24 * 60 * 60
            timePart = daySplit[1]
        } else {
            daySeconds = 0
            timePart = trimmed
        }

        let pieces = timePart.split(separator: ":").map(String.init)
        if pieces.count == 2,
           let minutes = Double(pieces[0]),
           let seconds = Double(pieces[1]) {
            return daySeconds + minutes * 60 + seconds
        }
        if pieces.count == 3,
           let hours = Double(pieces[0]),
           let minutes = Double(pieces[1]),
           let seconds = Double(pieces[2]) {
            return daySeconds + hours * 60 * 60 + minutes * 60 + seconds
        }
        return nil
    }

    private static func parseInt(_ text: String) -> Int? {
        Int(text.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    private static func nonEmptyTrimmed(_ text: String) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
