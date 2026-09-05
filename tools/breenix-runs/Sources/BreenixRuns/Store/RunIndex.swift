import Foundation

public struct RunIndex: Codable, Equatable, Sendable {
    public var schemaVersion: Int
    public var runs: [RunIndexEntry]

    public init(schemaVersion: Int = RunManifest.currentSchemaVersion, runs: [RunIndexEntry]) {
        self.schemaVersion = schemaVersion
        self.runs = runs
    }
}

public struct RunIndexEntry: Codable, Equatable, Sendable {
    public var id: String
    public var startedAt: Date
    public var endedAt: Date?
    public var arch: Arch
    public var profile: String
    public var verdict: Verdict
    public var tags: [String]

    public init(manifest: RunManifest) {
        self.id = manifest.id
        self.startedAt = manifest.startedAt
        self.endedAt = manifest.endedAt
        self.arch = manifest.arch
        self.profile = manifest.profile
        self.verdict = manifest.verdict
        self.tags = manifest.tags
    }
}
