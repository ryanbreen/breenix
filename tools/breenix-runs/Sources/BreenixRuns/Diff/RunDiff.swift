import Foundation

public enum StageDiffState: Equatable, Sendable {
    case reached(line: Int)
    case notReached
    case notApplicable
}

public struct SubsystemDeltaRow: Equatable, Sendable, Identifiable {
    public var id: String { stageName }

    public var stageName: String
    public var lhs: StageDiffState
    public var rhs: StageDiffState

    public init(stageName: String, lhs: StageDiffState, rhs: StageDiffState) {
        self.stageName = stageName
        self.lhs = lhs
        self.rhs = rhs
    }
}

public struct MarkerCountDeltaRow: Equatable, Sendable, Identifiable {
    public var id: String { family.rawValue }

    public var family: MarkerFamily
    public var lhsCount: Int
    public var rhsCount: Int

    public init(family: MarkerFamily, lhsCount: Int, rhsCount: Int) {
        self.family = family
        self.lhsCount = lhsCount
        self.rhsCount = rhsCount
    }
}

public struct HostFactsComparison: Equatable, Sendable {
    public var startWallDeltaSeconds: Double
    public var qemuPeersAarch64Delta: Int
    public var qemuPeersX86_64Delta: Int
    public var loadavg1Delta: Double?

    public init(
        startWallDeltaSeconds: Double,
        qemuPeersAarch64Delta: Int,
        qemuPeersX86_64Delta: Int,
        loadavg1Delta: Double?
    ) {
        self.startWallDeltaSeconds = startWallDeltaSeconds
        self.qemuPeersAarch64Delta = qemuPeersAarch64Delta
        self.qemuPeersX86_64Delta = qemuPeersX86_64Delta
        self.loadavg1Delta = loadavg1Delta
    }

    public var hasDelta: Bool {
        startWallDeltaSeconds != 0
            || qemuPeersAarch64Delta != 0
            || qemuPeersX86_64Delta != 0
            || loadavg1Delta.map { $0 != 0 } == true
    }
}

public enum HostFactsDelta: Equatable, Sendable {
    case notSampled(lhsSampled: Bool, rhsSampled: Bool)
    case sampled(HostFactsComparison)
}

public struct VerdictDelta: Equatable, Sendable {
    public var lhsText: String
    public var lhsState: VerdictDisplayState
    public var rhsText: String
    public var rhsState: VerdictDisplayState
    public var differs: Bool

    public init(
        lhsText: String,
        lhsState: VerdictDisplayState,
        rhsText: String,
        rhsState: VerdictDisplayState,
        differs: Bool
    ) {
        self.lhsText = lhsText
        self.lhsState = lhsState
        self.rhsText = rhsText
        self.rhsState = rhsState
        self.differs = differs
    }
}

public struct RunDiffResult: Equatable, Sendable {
    public var lhsID: String
    public var rhsID: String
    public var lhsLabel: String
    public var rhsLabel: String
    public var subsystemDelta: [SubsystemDeltaRow]
    public var markerCountDelta: [MarkerCountDeltaRow]
    public var hostFactsDelta: HostFactsDelta
    public var verdictDelta: VerdictDelta

    public init(
        lhsID: String,
        rhsID: String,
        lhsLabel: String,
        rhsLabel: String,
        subsystemDelta: [SubsystemDeltaRow],
        markerCountDelta: [MarkerCountDeltaRow],
        hostFactsDelta: HostFactsDelta,
        verdictDelta: VerdictDelta
    ) {
        self.lhsID = lhsID
        self.rhsID = rhsID
        self.lhsLabel = lhsLabel
        self.rhsLabel = rhsLabel
        self.subsystemDelta = subsystemDelta
        self.markerCountDelta = markerCountDelta
        self.hostFactsDelta = hostFactsDelta
        self.verdictDelta = verdictDelta
    }
}

public enum RunDiffError: Error, Equatable, CustomStringConvertible {
    case archMismatch(lhs: Arch, rhs: Arch)

    public var description: String {
        switch self {
        case .archMismatch(let lhs, let rhs):
            return "run diff arch mismatch: lhs \(lhs.rawValue), rhs \(rhs.rawValue)"
        }
    }
}

public enum RunDiff {
    public static func compare(lhs: RunManifest, rhs: RunManifest, store: RunStore) throws -> RunDiffResult {
        guard lhs.arch == rhs.arch else {
            throw RunDiffError.archMismatch(lhs: lhs.arch, rhs: rhs.arch)
        }

        let lhsIndex = try RunDetailViewModel.scanSerials(manifest: lhs, store: store)
        let rhsIndex = try RunDetailViewModel.scanSerials(manifest: rhs, store: store)
        let lhsCatalog = try StageCatalog.load(for: lhs.arch)
        let rhsCatalog = try StageCatalog.load(for: rhs.arch)
        let lhsStates = StateMachine.evaluate(catalog: lhsCatalog, index: lhsIndex)
        let rhsStates = StateMachine.evaluate(catalog: rhsCatalog, index: rhsIndex)

        return RunDiffResult(
            lhsID: lhs.id,
            rhsID: rhs.id,
            lhsLabel: runLabel(lhs),
            rhsLabel: runLabel(rhs),
            subsystemDelta: subsystemDelta(lhsCatalog: lhsCatalog, lhsStates: lhsStates, rhsCatalog: rhsCatalog, rhsStates: rhsStates),
            markerCountDelta: markerCountDelta(lhsIndex: lhsIndex, rhsIndex: rhsIndex),
            hostFactsDelta: hostFactsDelta(lhs: lhs.host, rhs: rhs.host),
            verdictDelta: verdictDelta(lhs: lhs.verdict, rhs: rhs.verdict)
        )
    }

    public static func render(_ result: RunDiffResult) -> String {
        [
            renderHeader(result),
            renderSubsystemDelta(result.subsystemDelta),
            renderMarkerCountDelta(result.markerCountDelta),
            renderHostFactsDelta(result.hostFactsDelta),
            renderVerdictDelta(result.verdictDelta)
        ].joined(separator: "\n\n")
    }

    private static func subsystemDelta(
        lhsCatalog: [BootStage],
        lhsStates: [StageState],
        rhsCatalog: [BootStage],
        rhsStates: [StageState]
    ) -> [SubsystemDeltaRow] {
        let lhsByName = Dictionary(uniqueKeysWithValues: lhsStates.map { ($0.stage.name, $0) })
        let rhsByName = Dictionary(uniqueKeysWithValues: rhsStates.map { ($0.stage.name, $0) })
        let orderedNames = orderedUnion(lhsCatalog.map(\.name), rhsCatalog.map(\.name))

        return orderedNames.compactMap { name in
            let lhsState = lhsByName[name].map(stageDiffState) ?? .notApplicable
            let rhsState = rhsByName[name].map(stageDiffState) ?? .notApplicable
            guard lhsState != rhsState else {
                return nil
            }
            if case .reached = lhsState, case .reached = rhsState {
                return nil
            }
            return SubsystemDeltaRow(stageName: name, lhs: lhsState, rhs: rhsState)
        }
    }

    private static func markerCountDelta(lhsIndex: SerialIndex, rhsIndex: SerialIndex) -> [MarkerCountDeltaRow] {
        let lhsCounts = markerCounts(lhsIndex)
        let rhsCounts = markerCounts(rhsIndex)
        return MarkerFamily.allCases.compactMap { family in
            let lhsCount = lhsCounts[family] ?? 0
            let rhsCount = rhsCounts[family] ?? 0
            guard lhsCount != rhsCount else {
                return nil
            }
            return MarkerCountDeltaRow(family: family, lhsCount: lhsCount, rhsCount: rhsCount)
        }
    }

    private static func markerCounts(_ index: SerialIndex) -> [MarkerFamily: Int] {
        var counts: [MarkerFamily: Int] = [:]
        for hit in index.hits {
            counts[hit.family, default: 0] += 1
        }
        return counts
    }

    private static func hostFactsDelta(lhs: HostFactsTrace?, rhs: HostFactsTrace?) -> HostFactsDelta {
        guard let lhs, let rhs else {
            return .notSampled(lhsSampled: lhs != nil, rhsSampled: rhs != nil)
        }

        return .sampled(HostFactsComparison(
            startWallDeltaSeconds: rhs.start.wallTime.timeIntervalSince(lhs.start.wallTime),
            qemuPeersAarch64Delta: rhs.start.qemuPeersAarch64 - lhs.start.qemuPeersAarch64,
            qemuPeersX86_64Delta: rhs.start.qemuPeersX86_64 - lhs.start.qemuPeersX86_64,
            loadavg1Delta: optionalDelta(lhs.start.loadavg1, rhs.start.loadavg1)
        ))
    }

    private static func optionalDelta(_ lhs: Double?, _ rhs: Double?) -> Double? {
        guard let lhs, let rhs else {
            return nil
        }
        return rhs - lhs
    }

    private static func verdictDelta(lhs: Verdict, rhs: Verdict) -> VerdictDelta {
        let lhsText = SidebarViewModel.displayText(for: lhs)
        let rhsText = SidebarViewModel.displayText(for: rhs)
        let lhsState = SidebarViewModel.displayState(for: lhs)
        let rhsState = SidebarViewModel.displayState(for: rhs)
        return VerdictDelta(
            lhsText: lhsText,
            lhsState: lhsState,
            rhsText: rhsText,
            rhsState: rhsState,
            differs: lhsText != rhsText || lhsState != rhsState
        )
    }

    private static func orderedUnion(_ lhs: [String], _ rhs: [String]) -> [String] {
        var seen = Set<String>()
        var ordered: [String] = []
        for name in lhs + rhs where !seen.contains(name) {
            seen.insert(name)
            ordered.append(name)
        }
        return ordered
    }

    private static func stageDiffState(_ state: StageState) -> StageDiffState {
        if let line = state.reachedLine {
            return .reached(line: line)
        }
        return .notReached
    }

    private static func runLabel(_ manifest: RunManifest) -> String {
        "\(manifest.id) (\(manifest.arch.rawValue), \(manifest.profile))"
    }

    private static func renderHeader(_ result: RunDiffResult) -> String {
        """
        Run diff
        lhs: \(result.lhsLabel)
        rhs: \(result.rhsLabel)
        """
    }

    private static func renderSubsystemDelta(_ rows: [SubsystemDeltaRow]) -> String {
        var lines = ["Subsystem-state delta"]
        guard !rows.isEmpty else {
            lines.append("no subsystem-state delta between these two runs")
            return lines.joined(separator: "\n")
        }
        for row in rows {
            lines.append("\(row.stageName): lhs=\(stageText(row.lhs)) rhs=\(stageText(row.rhs))")
        }
        return lines.joined(separator: "\n")
    }

    private static func renderMarkerCountDelta(_ rows: [MarkerCountDeltaRow]) -> String {
        var lines = ["Marker-count delta"]
        guard !rows.isEmpty else {
            lines.append("no marker-count delta between these two runs")
            return lines.joined(separator: "\n")
        }
        for row in rows {
            lines.append("\(row.family.rawValue): lhs=\(row.lhsCount) rhs=\(row.rhsCount)")
        }
        return lines.joined(separator: "\n")
    }

    private static func renderHostFactsDelta(_ delta: HostFactsDelta) -> String {
        var lines = ["Host-facts delta"]
        switch delta {
        case .notSampled(let lhsSampled, let rhsSampled):
            lines.append("not sampled: lhs=\(sampledText(lhsSampled)) rhs=\(sampledText(rhsSampled))")
        case .sampled(let values):
            if values.hasDelta {
                lines.append("start wall delta: \(formatSignedSeconds(values.startWallDeltaSeconds))")
                lines.append("qemu peers delta: aarch64=\(formatSignedInt(values.qemuPeersAarch64Delta)) x86_64=\(formatSignedInt(values.qemuPeersX86_64Delta))")
                lines.append("loadavg1 delta: \(formatSignedDouble(values.loadavg1Delta))")
            } else {
                lines.append("no host-facts delta between these two runs")
            }
        }
        return lines.joined(separator: "\n")
    }

    private static func renderVerdictDelta(_ delta: VerdictDelta) -> String {
        var lines = ["Verdict delta"]
        guard delta.differs else {
            lines.append("no verdict delta between these two runs")
            return lines.joined(separator: "\n")
        }
        lines.append("lhs: \(delta.lhsText) (\(verdictStateText(delta.lhsState)))")
        lines.append("rhs: \(delta.rhsText) (\(verdictStateText(delta.rhsState)))")
        return lines.joined(separator: "\n")
    }

    public static func stageText(_ state: StageDiffState) -> String {
        switch state {
        case .reached(let line):
            return "reached L\(line)"
        case .notReached:
            return "not reached"
        case .notApplicable:
            return "not applicable"
        }
    }

    public static func verdictStateText(_ state: VerdictDisplayState) -> String {
        switch state {
        case .success:
            return "success"
        case .failure:
            return "failure"
        case .attributed:
            return "attributed"
        case .inFlight:
            return "in flight"
        case .unknown:
            return "unknown"
        }
    }

    public static func sampledText(_ sampled: Bool) -> String {
        sampled ? "sampled" : "not sampled"
    }

    public static func formatSignedSeconds(_ value: Double) -> String {
        String(format: "%+.2fs", value)
    }

    public static func formatSignedDouble(_ value: Double?) -> String {
        guard let value else {
            return "unknown"
        }
        return String(format: "%+.2f", value)
    }

    public static func formatSignedInt(_ value: Int) -> String {
        String(format: "%+d", value)
    }
}
