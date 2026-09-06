import Foundation

public struct FatalRegsRecord: Codable, Equatable, Sendable, Identifiable {
    public var id: String { "\(startLine)-\(label ?? "unlabelled")-\(cpu.map(String.init) ?? "cpu?")" }

    public var startLine: Int
    public var endLine: Int
    public var label: String?
    public var cpu: Int?
    public var headerFields: [String: String]
    public var registers: [Int: String]
    public var truncated: Bool
    public var dispatchTraceCPU: Int?
    public var dispatchEntries: [FatalDispatchEntry]
    public var noDispatchesRecorded: Bool
    public var rawHeader: String

    public init(
        startLine: Int,
        endLine: Int,
        label: String?,
        cpu: Int?,
        headerFields: [String: String],
        registers: [Int: String],
        truncated: Bool,
        dispatchTraceCPU: Int?,
        dispatchEntries: [FatalDispatchEntry],
        noDispatchesRecorded: Bool,
        rawHeader: String
    ) {
        self.startLine = startLine
        self.endLine = endLine
        self.label = label
        self.cpu = cpu
        self.headerFields = headerFields
        self.registers = registers
        self.truncated = truncated
        self.dispatchTraceCPU = dispatchTraceCPU
        self.dispatchEntries = dispatchEntries
        self.noDispatchesRecorded = noDispatchesRecorded
        self.rawHeader = rawHeader
    }

    public var hasCompleteRegisterGrid: Bool {
        (0...30).allSatisfy { registers[$0] != nil }
    }
}

public struct FatalDispatchEntry: Codable, Equatable, Sendable, Identifiable {
    public var id: Int { index }

    public var index: Int
    public var path: String
    public var oldTID: Int
    public var newTID: Int
    public var elr: String
    public var spsr: String
    public var x30: String
    public var sp: String
    public var fromEL0: Bool

    public init(index: Int, path: String, oldTID: Int, newTID: Int, elr: String, spsr: String, x30: String, sp: String, fromEL0: Bool) {
        self.index = index
        self.path = path
        self.oldTID = oldTID
        self.newTID = newTID
        self.elr = elr
        self.spsr = spsr
        self.x30 = x30
        self.sp = sp
        self.fromEL0 = fromEL0
    }
}

public enum FatalRegsDecoder {
    private static let headerMarker = "[FATAL_REGS]"

    public static func decode(text: String) -> [FatalRegsRecord] {
        decode(lines: text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init))
    }

    public static func decode(lines: [String]) -> [FatalRegsRecord] {
        let serialLines = lines.enumerated().map { offset, text in
            SerialLine(lineNumber: offset + 1, range: SerialByteRange(offset: 0, length: 0), text: text, hits: [])
        }
        return decode(serialLines: serialLines)
    }

    public static func decode(serialLines lines: [SerialLine]) -> [FatalRegsRecord] {
        var records: [FatalRegsRecord] = []
        var index = 0

        while index < lines.count {
            guard let headerRange = lines[index].text.range(of: headerMarker) else {
                index += 1
                continue
            }

            let (record, nextIndex) = decodeRecord(startingAt: index, headerRange: headerRange, lines: lines)
            records.append(record)
            index = max(nextIndex, index + 1)
        }

        return records
    }

    private static func decodeRecord(
        startingAt startIndex: Int,
        headerRange: Range<String.Index>,
        lines: [SerialLine]
    ) -> (FatalRegsRecord, Int) {
        let headerLine = lines[startIndex]
        let headerText = String(headerLine.text[headerRange.lowerBound...])
        let headerPayload = headerText.dropFirst(headerMarker.count)
            .trimmingCharacters(in: .whitespaces)
        let headerFields = keyValueFields(String(headerPayload), separator: .whitespaces)
        let label = headerFields["label"]
        let cpu = headerFields["cpu"].flatMap(Int.init)

        var registers: [Int: String] = [:]
        var cursor = startIndex + 1
        var lastConsumed = startIndex

        while cursor < lines.count && registers.count < 31 {
            guard let parsed = parseRegisterLine(lines[cursor].text) else {
                break
            }
            for (register, value) in parsed {
                registers[register] = value
            }
            lastConsumed = cursor
            cursor += 1
        }

        let registerGridComplete = (0...30).allSatisfy { registers[$0] != nil }
        var dispatchTraceCPU: Int?
        var dispatchEntries: [FatalDispatchEntry] = []
        var noDispatchesRecorded = false

        if registerGridComplete, cursor < lines.count {
            if let traceCPU = parseDispatchHeader(lines[cursor].text) {
                dispatchTraceCPU = traceCPU
                lastConsumed = cursor
                cursor += 1
            }

            while cursor < lines.count {
                let line = lines[cursor].text
                if line.trimmingCharacters(in: .whitespaces) == "(no dispatches recorded)" {
                    noDispatchesRecorded = true
                    lastConsumed = cursor
                    cursor += 1
                    continue
                }
                guard let entry = parseDispatchEntry(line) else {
                    break
                }
                dispatchEntries.append(entry)
                lastConsumed = cursor
                cursor += 1
            }
        }

        let truncated = !registerGridComplete
        let record = FatalRegsRecord(
            startLine: headerLine.lineNumber,
            endLine: lines[lastConsumed].lineNumber,
            label: label,
            cpu: cpu,
            headerFields: headerFields,
            registers: registers,
            truncated: truncated,
            dispatchTraceCPU: dispatchTraceCPU,
            dispatchEntries: dispatchEntries,
            noDispatchesRecorded: noDispatchesRecorded,
            rawHeader: headerText
        )
        return (record, cursor)
    }

    private static func parseRegisterLine(_ line: String) -> [(Int, String)]? {
        let tokens = line.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !tokens.isEmpty else {
            return nil
        }

        var registers: [(Int, String)] = []
        for token in tokens {
            let pieces = token.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard pieces.count == 2,
                  pieces[0].hasPrefix("x"),
                  let number = Int(pieces[0].dropFirst()),
                  (0...30).contains(number),
                  isHex(String(pieces[1])) else {
                return nil
            }
            registers.append((number, String(pieces[1])))
        }
        return registers
    }

    private static func parseDispatchHeader(_ line: String) -> Int? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        let prefix = "DISPATCH_TRACE cpu="
        guard trimmed.hasPrefix(prefix), trimmed.hasSuffix(":") else {
            return nil
        }
        let value = trimmed.dropFirst(prefix.count).dropLast()
        return Int(value)
    }

    private static func parseDispatchEntry(_ line: String) -> FatalDispatchEntry? {
        let tokens = line.split(whereSeparator: \.isWhitespace).map(String.init)
        guard tokens.count == 7 || tokens.count == 8,
              tokens[0].hasPrefix("["),
              tokens[0].hasSuffix("]"),
              let index = Int(tokens[0].dropFirst().dropLast()),
              ["K", "I", "U", "R", "F", "B"].contains(tokens[1]) else {
            return nil
        }

        var fields = keyValueFields(tokens.dropFirst(2).joined(separator: " "), separator: .whitespaces)
        guard let oldAndNew = fields.removeValue(forKey: "old"),
              let arrow = oldAndNew.range(of: "->tid="),
              let oldTID = Int(oldAndNew[..<arrow.lowerBound]),
              let newTID = Int(oldAndNew[arrow.upperBound...]),
              let elr = fields["elr"],
              let spsr = fields["spsr"],
              let x30 = fields["x30"],
              let sp = fields["sp"] else {
            return nil
        }

        return FatalDispatchEntry(
            index: index,
            path: tokens[1],
            oldTID: oldTID,
            newTID: newTID,
            elr: elr,
            spsr: spsr,
            x30: x30,
            sp: sp,
            fromEL0: tokens.last == "EL0"
        )
    }

    private static func keyValueFields(_ text: String, separator: CharacterSet) -> [String: String] {
        var fields: [String: String] = [:]
        for token in text.components(separatedBy: separator) where !token.isEmpty {
            let pieces = token.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard pieces.count == 2 else {
                continue
            }
            fields[String(pieces[0])] = String(pieces[1])
        }
        return fields
    }

    private static func isHex(_ text: String) -> Bool {
        guard text.hasPrefix("0x"), text.count > 2 else {
            return false
        }
        return text.dropFirst(2).allSatisfy { $0.isHexDigit }
    }
}
