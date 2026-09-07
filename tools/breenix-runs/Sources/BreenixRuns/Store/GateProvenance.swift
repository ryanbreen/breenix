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
              }) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "Invalid gate provenance at \(url.path)"))
        }
        return value
    }

    var projectedVerdict: Verdict {
        if verdict == "PASS-WITH-ATTRIBUTED-LOCKUP" { return .attributed(verdict) }
        if verdict.hasPrefix("REFUSED") { return .refused(verdict) }
        if exitCode != 0 { return .fail(verdict) }
        if verdict == "PASS" { return .gateScript(command: command, exitCode: exitCode) }
        // Keep qualified successes (e.g. feature-mutated builds) visibly qualified.
        return .attributed(verdict)
    }
}
