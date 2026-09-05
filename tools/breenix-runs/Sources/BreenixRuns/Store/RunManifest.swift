import Foundation

public enum Arch: String, Codable, Equatable, Sendable {
    case aarch64
    case x86_64
}

public enum Launcher: String, Codable, Equatable, Sendable {
    case localQEMU
    case beastSSH
    case gateScript
    case imported
}

public struct KernelIdentity: Codable, Equatable, Sendable {
    public var buildID: String?
    public var gitSHA: String?
    public var gitDirty: Bool?
    public var imageSHA256: String?

    public init(buildID: String? = nil, gitSHA: String? = nil, gitDirty: Bool? = nil, imageSHA256: String? = nil) {
        self.buildID = buildID
        self.gitSHA = gitSHA
        self.gitDirty = gitDirty
        self.imageSHA256 = imageSHA256
    }
}

public enum Verdict: Codable, Equatable, Sendable {
    case pass
    case fail(String)
    case attributed(String)
    case running
    case unknown
    case refused(String)
    case gateScript(command: [String], exitCode: Int)

    private enum CodingKeys: String, CodingKey {
        case kind
        case reason
        case command
        case exitCode
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "pass":
            self = .pass
        case "fail":
            self = .fail(try container.decode(String.self, forKey: .reason))
        case "attributed":
            self = .attributed(try container.decode(String.self, forKey: .reason))
        case "running":
            self = .running
        case "unknown":
            self = .unknown
        case "refused":
            self = .refused(try container.decode(String.self, forKey: .reason))
        case "gateScript":
            self = .gateScript(
                command: try container.decode([String].self, forKey: .command),
                exitCode: try container.decode(Int.self, forKey: .exitCode)
            )
        default:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: container, debugDescription: "Unknown verdict kind \(kind)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .pass:
            try container.encode("pass", forKey: .kind)
        case .fail(let reason):
            try container.encode("fail", forKey: .kind)
            try container.encode(reason, forKey: .reason)
        case .attributed(let reason):
            try container.encode("attributed", forKey: .kind)
            try container.encode(reason, forKey: .reason)
        case .running:
            try container.encode("running", forKey: .kind)
        case .unknown:
            try container.encode("unknown", forKey: .kind)
        case .refused(let reason):
            try container.encode("refused", forKey: .kind)
            try container.encode(reason, forKey: .reason)
        case .gateScript(let command, let exitCode):
            try container.encode("gateScript", forKey: .kind)
            try container.encode(command, forKey: .command)
            try container.encode(exitCode, forKey: .exitCode)
        }
    }
}

public enum VerdictSource: Codable, Equatable, Sendable {
    case gateScript(command: [String], exitCode: Int)
    case imported
    case none

    private enum CodingKeys: String, CodingKey {
        case kind
        case command
        case exitCode
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "gateScript":
            self = .gateScript(
                command: try container.decode([String].self, forKey: .command),
                exitCode: try container.decode(Int.self, forKey: .exitCode)
            )
        case "imported":
            self = .imported
        case "none":
            self = .none
        default:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: container, debugDescription: "Unknown verdict source kind \(kind)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .gateScript(let command, let exitCode):
            try container.encode("gateScript", forKey: .kind)
            try container.encode(command, forKey: .command)
            try container.encode(exitCode, forKey: .exitCode)
        case .imported:
            try container.encode("imported", forKey: .kind)
        case .none:
            try container.encode("none", forKey: .kind)
        }
    }
}

public enum SerialStream: String, Codable, Equatable, Sendable {
    case single
    case com1
    case com2
}

public struct SerialRef: Codable, Equatable, Sendable {
    public var name: String
    public var path: String
    public var bytes: Int
    public var stream: SerialStream

    public init(name: String, path: String, bytes: Int, stream: SerialStream) {
        self.name = name
        self.path = path
        self.bytes = bytes
        self.stream = stream
    }
}

public struct CaptureRef: Codable, Equatable, Sendable {
    public var name: String
    public var path: String
    public var bytes: Int

    public init(name: String, path: String, bytes: Int) {
        self.name = name
        self.path = path
        self.bytes = bytes
    }
}

public struct RunManifest: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = 1

    public var schemaVersion: Int
    public var id: String
    public var startedAt: Date
    public var endedAt: Date?
    public var arch: Arch
    public var profile: String
    public var launcher: Launcher
    public var kernel: KernelIdentity
    public var host: HostFactsTrace?
    public var verdict: Verdict
    public var verdictSource: VerdictSource
    public var serials: [SerialRef]
    public var captures: [CaptureRef]
    public var command: [String]
    public var env: [String: String]
    public var tags: [String]
    public var notes: String?

    public init(
        schemaVersion: Int = RunManifest.currentSchemaVersion,
        id: String,
        startedAt: Date,
        endedAt: Date?,
        arch: Arch,
        profile: String,
        launcher: Launcher,
        kernel: KernelIdentity,
        host: HostFactsTrace?,
        verdict: Verdict,
        verdictSource: VerdictSource,
        serials: [SerialRef],
        captures: [CaptureRef],
        command: [String],
        env: [String: String],
        tags: [String],
        notes: String?
    ) {
        self.schemaVersion = schemaVersion
        self.id = id
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.arch = arch
        self.profile = profile
        self.launcher = launcher
        self.kernel = kernel
        self.host = host
        self.verdict = verdict
        self.verdictSource = verdictSource
        self.serials = serials
        self.captures = captures
        self.command = command
        self.env = env
        self.tags = tags
        self.notes = notes
    }

    public static func makeID(startedAt: Date, arch: Arch, profile: String, random: String? = nil) -> String {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyyMMdd'T'HHmmss'Z'"

        let suffix = random ?? String(format: "%04x", Int.random(in: 0...0xffff))
        return "\(formatter.string(from: startedAt))-\(arch.rawValue)-\(profile)-\(suffix)"
    }
}
