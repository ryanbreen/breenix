import Foundation

/// Gate-owned classification; serial markers cannot override it.
struct GateProvenance: Codable {
    var schemaVersion: Int
    var id: String
    var arch: Arch
    var profile: String
    var verdict: String
    var exitCode: Int
    var startedAt: Date
    var endedAt: Date
    var command: [String]
    var serials: [String]
    var captures: [String]

    static func read(from directory: URL) throws -> GateProvenance? {
        let url = directory.appendingPathComponent("run-inspector.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let value = try RunStore.decoder.decode(Self.self, from: Data(contentsOf: url))
        guard value.schemaVersion == 1, UUID(uuidString: value.id) != nil,
              value.endedAt >= value.startedAt, !value.command.isEmpty,
              (value.serials + value.captures).allSatisfy({
                  !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("/")
                      // "manifest.json" is RunStore's own reserved filename
                      // (RunStore.manifestURL) -- a declared evidence file with this
                      // name would be silently destroyed by, and would corrupt the
                      // byte count reported by, the run's real manifest.json write.
                      && $0 != "manifest.json"
              }) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "Invalid gate provenance at \(url.path)"))
        }
        return value
    }

    var projectedVerdict: Verdict {
        Verdict.projectGateVerdict(verdict, exitCode: exitCode, command: command)
    }
}
