import Foundation

public struct BootFactsHostMilliseconds: Codable, Equatable, Sendable {
    public var start: Int
    public var end: Int

    public init(start: Int, end: Int) {
        self.start = start
        self.end = end
    }
}

public struct BootFactsRecord: Codable, Equatable, Sendable, Identifiable {
    public var id: String {
        let source = sourceFile ?? "-"
        if let lineNumber {
            return "\(source)-\(lineNumber)-\(boot)-\(raw)"
        }
        return "\(source)-\(boot)-\(raw)"
    }

    public var boot: Int
    public var fields: [String: String]
    public var hostMilliseconds: BootFactsHostMilliseconds?
    public var raw: String
    public var lineNumber: Int?
    public var sourceFile: String?

    public init(
        boot: Int,
        fields: [String: String],
        hostMilliseconds: BootFactsHostMilliseconds? = nil,
        raw: String,
        lineNumber: Int? = nil,
        sourceFile: String? = nil
    ) {
        self.boot = boot
        self.fields = fields
        self.hostMilliseconds = hostMilliseconds
        self.raw = raw
        self.lineNumber = lineNumber
        self.sourceFile = sourceFile
    }
}

public enum BootFactsParser {
    private static let markerPrefix = "[GATE_BOOT_FACTS:boot="

    public static func parse(text: String) -> [BootFactsRecord] {
        parse(lines: text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init))
    }

    public static func parse(lines: [String]) -> [BootFactsRecord] {
        var records: [BootFactsRecord] = []
        for (offset, line) in lines.enumerated() {
            records.append(contentsOf: parseLine(line, lineNumber: offset + 1))
        }
        return records
    }

    public static func parse(serialLines: [SerialLine]) -> [BootFactsRecord] {
        serialLines.flatMap { line in
            parseLine(line.text, lineNumber: line.lineNumber)
        }
    }

    private static func parseLine(_ line: String, lineNumber: Int?) -> [BootFactsRecord] {
        var records: [BootFactsRecord] = []
        var searchStart = line.startIndex

        while let start = line[searchStart...].range(of: markerPrefix)?.lowerBound {
            guard let end = line[start...].firstIndex(of: "]") else {
                break
            }
            let marker = String(line[start...end])
            if let record = parseMarker(marker, lineNumber: lineNumber) {
                records.append(record)
            }
            searchStart = line.index(after: end)
        }

        return records
    }

    private static func parseMarker(_ marker: String, lineNumber: Int?) -> BootFactsRecord? {
        guard marker.hasPrefix(markerPrefix), marker.hasSuffix("]") else {
            return nil
        }

        let payloadStart = marker.index(marker.startIndex, offsetBy: markerPrefix.count)
        let payloadEnd = marker.index(before: marker.endIndex)
        let payload = String(marker[payloadStart..<payloadEnd])
        let pieces = payload.split(separator: ":", omittingEmptySubsequences: false)
        guard let first = pieces.first, let boot = Int(first) else {
            return nil
        }

        var fields: [String: String] = [:]
        for piece in pieces.dropFirst() {
            guard let eq = piece.firstIndex(of: "=") else {
                continue
            }
            let key = String(piece[..<eq])
            let value = String(piece[piece.index(after: eq)...])
            fields[key] = value
        }

        return BootFactsRecord(
            boot: boot,
            fields: fields,
            hostMilliseconds: parseHostMilliseconds(fields["host_ms"]),
            raw: marker,
            lineNumber: lineNumber
        )
    }

    private static func parseHostMilliseconds(_ raw: String?) -> BootFactsHostMilliseconds? {
        guard let raw else {
            return nil
        }
        let pieces = raw.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        guard pieces.count == 2,
              let start = Int(pieces[0]),
              let end = Int(pieces[1]) else {
            return nil
        }
        return BootFactsHostMilliseconds(start: start, end: end)
    }
}
