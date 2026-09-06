import Foundation

public struct MessageLineViewModel: Equatable, Sendable, Identifiable {
    public var id: Int { lineNumber }
    public var lineNumber: Int
    public var familyText: String
    public var bucket: MessageFamilyBucket
    public var text: String
    public var line: SerialLine

    public init(line: SerialLine) {
        self.lineNumber = line.lineNumber
        self.familyText = line.hits.first?.family.rawValue ?? "other"
        self.bucket = MessageFilter.bucket(for: line)
        self.text = line.text
        self.line = line
    }
}

public enum MessageFamilyBucket: String, CaseIterable, Equatable, Hashable, Sendable {
    case boot
    case tests
    case oracles
    case heartbeat
    case faults
    case traceNoise = "trace-noise"
    case other

    public var label: String {
        rawValue
    }
}

public enum MessageFilter {
    public static func bucket(for family: MarkerFamily) -> MessageFamilyBucket {
        switch family {
        case .bootStageAarch64, .bootBannerAarch64, .kernelLogX86:
            return .boot
        case .testCase, .testComplete, .testBootTests, .testKTAP, .testBTRT:
            return .tests
        case .oracleGeneric,
             .oracleFutexHandoff,
             .oracleFcntlPM,
             .oracleIRQHold,
             .oraclePollTCP,
             .oracleTimerScale,
             .censusTTBR0ASID,
             .censusPinnedHome,
             .censusStrand:
            return .oracles
        case .heartbeat:
            return .heartbeat
        case .faultEL1First,
             .faultAbort,
             .faultPanic,
             .faultSoftLockup,
             .faultExt2Stall,
             .lockOrder:
            return .faults
        case .traceNoise:
            return .traceNoise
        case .execSmoke, .devicePCICensus:
            return .other
        }
    }

    public static func bucket(for line: SerialLine) -> MessageFamilyBucket {
        guard let family = line.hits.first?.family else {
            return .other
        }
        return bucket(for: family)
    }

    public static func includes(
        _ line: SerialLine,
        selectedBuckets: Set<MessageFamilyBucket>,
        searchText: String?
    ) -> Bool {
        if !selectedBuckets.isEmpty, !selectedBuckets.contains(bucket(for: line)) {
            return false
        }

        let needle = searchText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !needle.isEmpty else {
            return true
        }
        return line.text.range(of: needle, options: [.caseInsensitive, .diacriticInsensitive]) != nil
    }

    public static func rows(for index: SerialIndex) -> [MessageLineViewModel] {
        index.lines.map(MessageLineViewModel.init(line:))
    }
}
