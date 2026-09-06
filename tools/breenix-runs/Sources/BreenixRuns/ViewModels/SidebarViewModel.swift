import Foundation

/// Library-side sidebar row model for the SwiftUI target described in DESIGN.md
/// §2.2 and §2.5; keeping it here lets XCTest cover row sorting without linking
/// AppKit or SwiftUI.
public struct SidebarRowViewModel: Equatable, Sendable, Identifiable {
    public var id: String
    public var arch: String
    public var profile: String
    public var verdictText: String
    public var verdictState: VerdictDisplayState
    public var shortSHA: String
    public var timeText: String
    public var startedAt: Date

    public init(
        id: String,
        arch: String,
        profile: String,
        verdictText: String,
        verdictState: VerdictDisplayState,
        shortSHA: String,
        timeText: String,
        startedAt: Date
    ) {
        self.id = id
        self.arch = arch
        self.profile = profile
        self.verdictText = verdictText
        self.verdictState = verdictState
        self.shortSHA = shortSHA
        self.timeText = timeText
        self.startedAt = startedAt
    }
}

public enum VerdictDisplayState: Equatable, Sendable {
    case success
    case failure
    case attributed
    case inFlight
    case unknown
}

public enum SidebarViewModel {
    public static func rows(for manifests: [RunManifest]) -> [SidebarRowViewModel] {
        manifests
            .sorted(by: newestFirst)
            .map { row($0) }
    }

    public static func rows(for entries: [RunIndexEntry]) -> [SidebarRowViewModel] {
        entries
            .sorted(by: newestFirst)
            .map { entry in
                row(
                    id: entry.id,
                    startedAt: entry.startedAt,
                    arch: entry.arch,
                    profile: entry.profile,
                    verdict: entry.verdict,
                    gitSHA: nil
                )
            }
    }

    public static func row(for manifest: RunManifest) -> SidebarRowViewModel {
        row(manifest)
    }

    public static func displayState(for verdict: Verdict) -> VerdictDisplayState {
        switch verdict {
        case .pass:
            return .success
        case .fail, .refused:
            return .failure
        case .attributed:
            return .attributed
        case .running:
            return .inFlight
        case .unknown:
            return .unknown
        case .gateScript(_, let exitCode):
            return exitCode == 0 ? .success : .failure
        }
    }

    public static func displayText(for verdict: Verdict) -> String {
        switch verdict {
        case .pass:
            return "PASS"
        case .fail:
            return "FAIL"
        case .attributed(let reason):
            return attributedText(reason)
        case .running:
            return "running..."
        case .unknown:
            return "unknown"
        case .refused:
            return "REFUSED"
        case .gateScript(_, let exitCode):
            return exitCode == 0 ? "PASS" : "FAIL \(exitCode)"
        }
    }

    private static func row(_ manifest: RunManifest) -> SidebarRowViewModel {
        row(
            id: manifest.id,
            startedAt: manifest.startedAt,
            arch: manifest.arch,
            profile: manifest.profile,
            verdict: manifest.verdict,
            gitSHA: manifest.kernel.gitSHA
        )
    }

    private static func row(
        id: String,
        startedAt: Date,
        arch: Arch,
        profile: String,
        verdict: Verdict,
        gitSHA: String?
    ) -> SidebarRowViewModel {
        SidebarRowViewModel(
            id: id,
            arch: arch.rawValue,
            profile: profile,
            verdictText: displayText(for: verdict),
            verdictState: displayState(for: verdict),
            shortSHA: shortSHA(gitSHA),
            timeText: timeText(startedAt),
            startedAt: startedAt
        )
    }

    private static func newestFirst(_ lhs: RunManifest, _ rhs: RunManifest) -> Bool {
        if lhs.startedAt == rhs.startedAt {
            return lhs.id < rhs.id
        }
        return lhs.startedAt > rhs.startedAt
    }

    private static func newestFirst(_ lhs: RunIndexEntry, _ rhs: RunIndexEntry) -> Bool {
        if lhs.startedAt == rhs.startedAt {
            return lhs.id < rhs.id
        }
        return lhs.startedAt > rhs.startedAt
    }

    private static func shortSHA(_ gitSHA: String?) -> String {
        guard let gitSHA, !gitSHA.isEmpty else {
            return "(local)"
        }
        return String(gitSHA.prefix(8))
    }

    private static func timeText(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }

    private static func attributedText(_ reason: String) -> String {
        if reason.isEmpty {
            return "PASS+ATTR"
        }
        if let hash = reason.firstIndex(of: "#") {
            let suffix = reason[hash...].split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true).first
            return suffix.map { "PASS+\($0)" } ?? "PASS+ATTR"
        }
        return "PASS+ATTR"
    }
}
