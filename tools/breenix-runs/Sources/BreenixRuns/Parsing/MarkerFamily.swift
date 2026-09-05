import Foundation

public enum MarkerFamily: String, Codable, CaseIterable, Equatable, Sendable {
    case bootStageAarch64 = "bootStage.aarch64"
    case bootBannerAarch64 = "bootBanner.aarch64"
    case kernelLogX86 = "kernelLog.x86"
    case testCase = "test.case"
    case testComplete = "test.complete"
    case testBootTests = "test.bootTests"
    case testKTAP = "test.ktap"
    case testBTRT = "test.btrt"
    case heartbeat
    case execSmoke
    case oracleGeneric = "oracle.generic"
    case oracleFutexHandoff = "oracle.futexHandoff"
    case oracleFcntlPM = "oracle.fcntlPM"
    case oracleIRQHold = "oracle.irqHold"
    case oraclePollTCP = "oracle.pollTCP"
    case oracleTimerScale = "oracle.timerScale"
    case censusTTBR0ASID = "census.ttbr0ASID"
    case censusPinnedHome = "census.pinnedHome"
    case censusStrand = "census.strand"
    case faultEL1First = "fault.el1First"
    case faultAbort = "fault.abort"
    case faultPanic = "fault.panic"
    case faultSoftLockup = "fault.softLockup"
    case faultExt2Stall = "fault.ext2Stall"
    case lockOrder
    case devicePCICensus = "device.pciCensus"
    case traceNoise
}

// Marker fields stay flat so the emitted JSON has the same stable,
// discriminator-shaped encoding style as RunManifest's payload enums.
public enum MarkerFieldValue: Codable, Equatable, Sendable {
    case string(String)
    case int(Int)
    case bool(Bool)

    private enum CodingKeys: String, CodingKey {
        case kind
        case string
        case int
        case bool
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "string":
            self = .string(try container.decode(String.self, forKey: .string))
        case "int":
            self = .int(try container.decode(Int.self, forKey: .int))
        case "bool":
            self = .bool(try container.decode(Bool.self, forKey: .bool))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .kind,
                in: container,
                debugDescription: "Unknown marker field value kind \(kind)"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .string(let value):
            try container.encode("string", forKey: .kind)
            try container.encode(value, forKey: .string)
        case .int(let value):
            try container.encode("int", forKey: .kind)
            try container.encode(value, forKey: .int)
        case .bool(let value):
            try container.encode("bool", forKey: .kind)
            try container.encode(value, forKey: .bool)
        }
    }

    public var stringValue: String? {
        if case .string(let value) = self {
            return value
        }
        return nil
    }

    public var intValue: Int? {
        if case .int(let value) = self {
            return value
        }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let value) = self {
            return value
        }
        return nil
    }
}
