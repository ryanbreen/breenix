import Darwin
import Foundation

public enum RunStoreError: Error, Equatable {
    case runNotFound(String)
    case renameFailed(errno: Int32)
}

public struct RunStore: Sendable {
    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    public static func defaultStore() -> RunStore {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return RunStore(root: base.appendingPathComponent("BreenixRuns", isDirectory: true))
    }

    public var runsDirectory: URL {
        root.appendingPathComponent("runs", isDirectory: true)
    }

    public var indexURL: URL {
        root.appendingPathComponent("index.json")
    }

    public func runDirectory(id: String) -> URL {
        runsDirectory.appendingPathComponent(id, isDirectory: true)
    }

    public func manifestURL(id: String) -> URL {
        runDirectory(id: id).appendingPathComponent("manifest.json")
    }

    public func prepareRoot() throws {
        try FileManager.default.createDirectory(at: runsDirectory, withIntermediateDirectories: true)
        let schemaURL = root.appendingPathComponent("schema-version")
        if !FileManager.default.fileExists(atPath: schemaURL.path) {
            try "\(RunManifest.currentSchemaVersion)\n".data(using: .utf8)?.write(to: schemaURL)
        }
    }

    public func createRunDirectory(id: String) throws -> URL {
        try prepareRoot()
        let directory = runDirectory(id: id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    public func writeManifest(_ manifest: RunManifest) throws {
        _ = try createRunDirectory(id: manifest.id)
        let data = try RunStore.encoder.encode(manifest)
        try writeAtomically(data: data, to: manifestURL(id: manifest.id))
        _ = try rebuildIndex()
    }

    public func readManifest(id: String) throws -> RunManifest {
        let url = manifestURL(id: id)
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw RunStoreError.runNotFound(id)
        }
        return try RunStore.decoder.decode(RunManifest.self, from: Data(contentsOf: url))
    }

    public func readIndex() throws -> RunIndex {
        if !FileManager.default.fileExists(atPath: indexURL.path) {
            return try rebuildIndex()
        }
        return try RunStore.decoder.decode(RunIndex.self, from: Data(contentsOf: indexURL))
    }

    @discardableResult
    public func rebuildIndex() throws -> RunIndex {
        try prepareRoot()
        let runDirectories = try FileManager.default.contentsOfDirectory(
            at: runsDirectory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )

        var entries: [RunIndexEntry] = []
        for directory in runDirectories {
            let resourceValues = try directory.resourceValues(forKeys: [.isDirectoryKey])
            guard resourceValues.isDirectory == true else {
                continue
            }

            let manifestURL = directory.appendingPathComponent("manifest.json")
            guard FileManager.default.fileExists(atPath: manifestURL.path) else {
                continue
            }

            let data = try Data(contentsOf: manifestURL)
            let manifest = try RunStore.decoder.decode(RunManifest.self, from: data)
            entries.append(RunIndexEntry(manifest: manifest))
        }

        entries.sort { lhs, rhs in
            if lhs.startedAt == rhs.startedAt {
                return lhs.id < rhs.id
            }
            return lhs.startedAt < rhs.startedAt
        }

        let index = RunIndex(runs: entries)
        try writeAtomically(data: try RunStore.encoder.encode(index), to: indexURL)
        return index
    }

    public func latestManifest() throws -> RunManifest {
        guard let latest = try readIndex().runs.max(by: { $0.startedAt < $1.startedAt }) else {
            throw RunStoreError.runNotFound("latest")
        }
        return try readManifest(id: latest.id)
    }

    public func writeAtomically(data: Data, to finalURL: URL) throws {
        let tmpURL = finalURL.deletingLastPathComponent()
            .appendingPathComponent(finalURL.lastPathComponent + ".tmp")
        try FileManager.default.createDirectory(at: finalURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: tmpURL, options: [.atomic])
        if rename(tmpURL.path, finalURL.path) != 0 {
            throw RunStoreError.renameFailed(errno: errno)
        }
    }

    public static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }()

    public static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
