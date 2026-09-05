import Foundation

public struct SerialByteRange: Codable, Equatable, Hashable, Sendable {
    public var offset: Int
    public var length: Int

    public init(offset: Int, length: Int) {
        self.offset = offset
        self.length = length
    }

    public var endOffset: Int {
        offset + length
    }
}

public struct MarkerHit: Codable, Equatable, Sendable {
    public var family: MarkerFamily
    public var range: SerialByteRange
    public var fields: [String: MarkerFieldValue]
    public var lineNumber: Int

    public init(
        family: MarkerFamily,
        range: SerialByteRange,
        fields: [String: MarkerFieldValue],
        lineNumber: Int
    ) {
        self.family = family
        self.range = range
        self.fields = fields
        self.lineNumber = lineNumber
    }
}

public struct SerialLine: Codable, Equatable, Sendable {
    public var lineNumber: Int
    public var range: SerialByteRange
    public var text: String
    public var hits: [MarkerHit]

    public init(lineNumber: Int, range: SerialByteRange, text: String, hits: [MarkerHit]) {
        self.lineNumber = lineNumber
        self.range = range
        self.text = text
        self.hits = hits
    }
}

public struct SerialIndex: Codable, Equatable, Sendable {
    public var byteCount: Int
    public var lines: [SerialLine]

    public init(byteCount: Int, lines: [SerialLine]) {
        self.byteCount = byteCount
        self.lines = lines
    }

    public var hits: [MarkerHit] {
        lines.flatMap(\.hits)
    }
}
