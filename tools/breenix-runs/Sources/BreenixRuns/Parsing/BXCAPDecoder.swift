import Foundation

public enum BXCAPSection: String, Codable, Equatable, Sendable {
    case begin = "BEGIN"
    case edge = "EDGE"
    case cpu = "CPU"
    case thread = "THR"
    case dispatch = "DISP"
    case event = "EV"
    case counter = "CNT"
    case ring = "RING"
    case note = "NOTE"
    case end = "END"
}

public struct BXCAPRow: Codable, Equatable, Sendable, Identifiable {
    public var id: String { "\(lineNumber)-\(section.rawValue)-\(fields)-\(note ?? "")" }

    public var section: BXCAPSection
    public var fields: [String: String]
    public var note: String?
    public var lineNumber: Int
    public var raw: String

    public init(section: BXCAPSection, fields: [String: String], note: String? = nil, lineNumber: Int, raw: String) {
        self.section = section
        self.fields = fields
        self.note = note
        self.lineNumber = lineNumber
        self.raw = raw
    }
}

public struct BXCAPCapture: Codable, Equatable, Sendable, Identifiable {
    public var id: Int { seq }

    public var seq: Int
    public var version: Int
    public var edge: String?
    public var beginLine: Int
    public var endLine: Int?
    public var beginFields: [String: String]
    public var rows: [BXCAPRow]
    public var endFields: [String: String]
    public var verdict: String?
    public var sectionsSkipped: String?
    public var truncated: Bool

    public init(
        seq: Int,
        version: Int,
        edge: String?,
        beginLine: Int,
        endLine: Int?,
        beginFields: [String: String],
        rows: [BXCAPRow],
        endFields: [String: String],
        verdict: String?,
        sectionsSkipped: String?,
        truncated: Bool
    ) {
        self.seq = seq
        self.version = version
        self.edge = edge
        self.beginLine = beginLine
        self.endLine = endLine
        self.beginFields = beginFields
        self.rows = rows
        self.endFields = endFields
        self.verdict = verdict
        self.sectionsSkipped = sectionsSkipped
        self.truncated = truncated
    }
}

public struct BXCAPRefusal: Codable, Equatable, Sendable, Identifiable {
    public var id: String { "\(seq.map(String.init) ?? "seq?")-\(version.map(String.init) ?? "v?")-\(startLine)" }

    public var seq: Int?
    public var version: Int?
    public var startLine: Int
    public var endLine: Int?
    public var reason: String
    public var rawBegin: String

    public init(seq: Int?, version: Int?, startLine: Int, endLine: Int?, reason: String, rawBegin: String) {
        self.seq = seq
        self.version = version
        self.startLine = startLine
        self.endLine = endLine
        self.reason = reason
        self.rawBegin = rawBegin
    }
}

public struct BXCAPDecodeResult: Codable, Equatable, Sendable {
    public var captures: [BXCAPCapture]
    public var refusals: [BXCAPRefusal]

    public init(captures: [BXCAPCapture] = [], refusals: [BXCAPRefusal] = []) {
        self.captures = captures
        self.refusals = refusals
    }

    public var isEmpty: Bool {
        captures.isEmpty && refusals.isEmpty
    }
}

public enum BXCAPDecoder {
    private struct Marker {
        var section: BXCAPSection
        var payload: String
        var lineNumber: Int
        var raw: String
    }

    private struct Builder {
        var seq: Int
        var version: Int
        var edge: String?
        var beginLine: Int
        var beginFields: [String: String]
        var rows: [BXCAPRow] = []
    }

    private struct RefusedBuilder {
        var seq: Int?
        var version: Int?
        var startLine: Int
        var rawBegin: String
        var reason: String
    }

    public static func decode(text: String) -> BXCAPDecodeResult {
        decode(lines: text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init))
    }

    public static func decode(lines: [String]) -> BXCAPDecodeResult {
        let foundMarkers = lines.enumerated().flatMap { offset, line in
            markers(in: line, lineNumber: offset + 1)
        }
        return decode(markers: foundMarkers)
    }

    public static func decode(serialLines: [SerialLine]) -> BXCAPDecodeResult {
        let foundMarkers = serialLines.flatMap { line in
            markers(in: line.text, lineNumber: line.lineNumber)
        }
        return decode(markers: foundMarkers)
    }

    private static func decode(markers: [Marker]) -> BXCAPDecodeResult {
        var active: [Int: Builder] = [:]
        var refusedActive: [Int: RefusedBuilder] = [:]
        var stack: [Int] = []
        var captures: [BXCAPCapture] = []
        var refusals: [BXCAPRefusal] = []

        for marker in markers {
            switch marker.section {
            case .begin:
                let fields = keyValueFields(marker.payload)
                let version = fields["v"].flatMap(Int.init)
                let seq = fields["seq"].flatMap(Int.init)
                guard let version, version == 1, let seq else {
                    if let seq {
                        refusedActive[seq] = RefusedBuilder(
                            seq: seq,
                            version: version,
                            startLine: marker.lineNumber,
                            rawBegin: marker.raw,
                            reason: "unsupported version"
                        )
                        stack.append(seq)
                    } else {
                        refusals.append(BXCAPRefusal(
                            seq: nil,
                            version: version,
                            startLine: marker.lineNumber,
                            endLine: nil,
                            reason: "missing seq",
                            rawBegin: marker.raw
                        ))
                    }
                    continue
                }
                active[seq] = Builder(
                    seq: seq,
                    version: version,
                    edge: fields["edge"],
                    beginLine: marker.lineNumber,
                    beginFields: fields
                )
                stack.append(seq)

            case .end:
                let fields = keyValueFields(marker.payload)
                guard let seq = fields["seq"].flatMap(Int.init) else {
                    continue
                }
                if let refused = refusedActive.removeValue(forKey: seq) {
                    refusals.append(BXCAPRefusal(
                        seq: refused.seq,
                        version: refused.version,
                        startLine: refused.startLine,
                        endLine: marker.lineNumber,
                        reason: refused.reason,
                        rawBegin: refused.rawBegin
                    ))
                    remove(seq, from: &stack)
                    continue
                }
                guard let builder = active.removeValue(forKey: seq) else {
                    continue
                }
                if fields["v"].flatMap(Int.init) != 1 {
                    refusals.append(BXCAPRefusal(
                        seq: seq,
                        version: fields["v"].flatMap(Int.init),
                        startLine: builder.beginLine,
                        endLine: marker.lineNumber,
                        reason: "unsupported version",
                        rawBegin: builder.beginFields.description
                    ))
                    remove(seq, from: &stack)
                    continue
                }
                captures.append(finish(builder: builder, endLine: marker.lineNumber, endFields: fields, missingEnd: false))
                remove(seq, from: &stack)

            case .edge, .cpu, .thread, .dispatch, .event, .counter, .ring, .note:
                guard let seq = stack.last, var builder = active[seq] else {
                    continue
                }
                builder.rows.append(row(from: marker))
                active[seq] = builder
            }
        }

        for builder in active.values.sorted(by: { $0.beginLine < $1.beginLine }) {
            captures.append(finish(builder: builder, endLine: nil, endFields: [:], missingEnd: true))
        }
        for refused in refusedActive.values.sorted(by: { $0.startLine < $1.startLine }) {
            refusals.append(BXCAPRefusal(
                seq: refused.seq,
                version: refused.version,
                startLine: refused.startLine,
                endLine: nil,
                reason: refused.reason,
                rawBegin: refused.rawBegin
            ))
        }

        captures.sort { $0.seq < $1.seq }
        refusals.sort { lhs, rhs in
            if lhs.startLine == rhs.startLine {
                return (lhs.seq ?? 0) < (rhs.seq ?? 0)
            }
            return lhs.startLine < rhs.startLine
        }
        return BXCAPDecodeResult(captures: captures, refusals: refusals)
    }

    private static func finish(builder: Builder, endLine: Int?, endFields: [String: String], missingEnd: Bool) -> BXCAPCapture {
        BXCAPCapture(
            seq: builder.seq,
            version: builder.version,
            edge: builder.edge,
            beginLine: builder.beginLine,
            endLine: endLine,
            beginFields: builder.beginFields,
            rows: builder.rows,
            endFields: endFields,
            verdict: endFields["verdict"],
            sectionsSkipped: endFields["sections_skipped"],
            truncated: missingEnd || endFields["truncated"] == "1"
        )
    }

    private static func row(from marker: Marker) -> BXCAPRow {
        if marker.section == .note {
            return BXCAPRow(section: .note, fields: [:], note: marker.payload, lineNumber: marker.lineNumber, raw: marker.raw)
        }
        return BXCAPRow(section: marker.section, fields: keyValueFields(marker.payload), lineNumber: marker.lineNumber, raw: marker.raw)
    }

    private static func markers(in line: String, lineNumber: Int) -> [Marker] {
        var markers: [Marker] = []
        var searchStart = line.startIndex

        while let start = line[searchStart...].range(of: "[BXCAP:")?.lowerBound {
            guard let end = line[start...].firstIndex(of: "]") else {
                break
            }
            let raw = String(line[start...end])
            if let marker = marker(from: raw, lineNumber: lineNumber) {
                markers.append(marker)
            }
            searchStart = line.index(after: end)
        }

        return markers
    }

    private static func marker(from raw: String, lineNumber: Int) -> Marker? {
        guard raw.hasPrefix("[BXCAP:"), raw.hasSuffix("]") else {
            return nil
        }
        let start = raw.index(raw.startIndex, offsetBy: "[BXCAP:".count)
        let end = raw.index(before: raw.endIndex)
        let body = raw[start..<end]
        let headAndPayload = body.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: false)
        guard let head = headAndPayload.first,
              let section = BXCAPSection(rawValue: String(head)) else {
            return nil
        }
        let payload = headAndPayload.count > 1 ? String(headAndPayload[1]) : ""
        return Marker(section: section, payload: payload, lineNumber: lineNumber, raw: raw)
    }

    private static func keyValueFields(_ payload: String) -> [String: String] {
        var fields: [String: String] = [:]
        for token in payload.split(whereSeparator: \.isWhitespace) {
            let pieces = token.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard pieces.count == 2 else {
                continue
            }
            fields[String(pieces[0])] = String(pieces[1])
        }
        return fields
    }

    private static func remove(_ seq: Int, from stack: inout [Int]) {
        if let index = stack.lastIndex(of: seq) {
            stack.remove(at: index)
        }
    }
}
