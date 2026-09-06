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

    public func captureURL(_ capture: CaptureRef, manifest: RunManifest) -> URL {
        if capture.path.hasPrefix("/") {
            return URL(fileURLWithPath: capture.path)
        }
        return runDirectory(id: manifest.id).appendingPathComponent(capture.path)
    }

    // Shared by RunShow.render(traces:) and RunDetailViewModel.load(): both
    // need the gate-stdout.txt CaptureRef's text to feed TracesViewModel.build
    // (DESIGN.md Sec 1.5 -- gate-stdout.txt is the real GATE_BOOT_FACTS carrier
    // today). One implementation so the two call sites cannot drift.
    public func readGateStdoutText(manifest: RunManifest) throws -> String {
        let chunks = try manifest.captures
            .filter { $0.name == "gate-stdout.txt" }
            .compactMap { capture -> String? in
                let url = captureURL(capture, manifest: manifest)
                guard FileManager.default.fileExists(atPath: url.path) else {
                    return nil
                }
                return String(decoding: try Data(contentsOf: url), as: UTF8.self)
            }
        return chunks.joined(separator: "\n")
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

            // Rebuildability is the whole safety property (DESIGN.md Sec 3.2 point 2):
            // "Delete it, corrupt it, hand-edit it -- it regenerates." A single
            // unreadable/corrupt manifest.json must never poison the index for
            // every OTHER run (and, via writeManifest's unconditional rebuild,
            // block recording of every future run too) -- skip it and keep going.
            do {
                let data = try Data(contentsOf: manifestURL)
                let manifest = try RunStore.decoder.decode(RunManifest.self, from: data)
                entries.append(RunIndexEntry(manifest: manifest))
            } catch {
                FileHandle.standardError.write(Data(
                    "warning: skipping unreadable manifest at \(manifestURL.path): \(error)\n".utf8
                ))
                continue
            }
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

    public func latestFailureManifest() throws -> RunManifest {
        let failures = try readIndex().runs.filter(\.verdict.isFailure)
        guard let latest = failures.max(by: { $0.startedAt < $1.startedAt }) else {
            throw RunStoreError.runNotFound("latest-fail")
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

    // `.iso8601` (JSONEncoder's built-in strategy) formats without fractional
    // seconds, so every persist/reload round trip through it truncated Date
    // fields to whole seconds. `facts <run-id>` (reads the persisted manifest
    // back from disk) then reported different host-fact wall-time/duration
    // values than the in-process `run arm` output (which prints the
    // pre-persistence in-memory manifest). ISO8601DateFormatter with
    // `.withFractionalSeconds` preserves millisecond precision instead; the
    // decoder falls back to a formatter without that option so a manifest
    // written under the old whole-second strategy still parses.
    //
    // A fresh formatter is built per call rather than shared as `static let`
    // state: ISO8601DateFormatter is not documented Sendable, and encode/decode
    // can run concurrently (the CLI and the app both write, DESIGN.md Sec 3.2
    // point 3). Constructing one is cheap relative to the JSON work already
    // happening around it.
    private static func iso8601Formatter(fractional: Bool) -> ISO8601DateFormatter {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = fractional ? [.withInternetDateTime, .withFractionalSeconds] : [.withInternetDateTime]
        return formatter
    }

    public static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .custom { date, encoder in
            var container = encoder.singleValueContainer()
            try container.encode(RunStore.iso8601Formatter(fractional: true).string(from: date))
        }
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }()

    public static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let string = try container.decode(String.self)
            if let date = RunStore.iso8601Formatter(fractional: true).date(from: string)
                ?? RunStore.iso8601Formatter(fractional: false).date(from: string) {
                return date
            }
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Expected a fractional or whole-second ISO8601 date, got \(string)"
            )
        }
        return decoder
    }()
}
