import Foundation

public struct SubsystemsViewModel: Equatable, Sendable {
    public var arch: String
    public var profile: String
    public var reachedCount: Int
    public var totalCount: Int
    public var stoppedIndex: Int?
    public var rows: [SubsystemRowViewModel]

    public init(manifest: RunManifest, states: [StageState]) {
        self.arch = manifest.arch.rawValue
        self.profile = manifest.profile
        self.reachedCount = states.filter(\.isReached).count
        self.totalCount = states.count
        self.stoppedIndex = states.first { $0.isStoppedHere }?.index
        self.rows = states.map(SubsystemRowViewModel.init(state:))
    }
}

public struct SubsystemRowViewModel: Equatable, Sendable, Identifiable {
    public var id: Int { index }
    public var index: Int
    public var name: String
    public var status: SubsystemStatus
    public var reachedLine: Int?
    public var failureMeaning: String
    public var checkHint: String
    public var failureArmLine: Int?
    public var failureArmText: String?

    public init(state: StageState) {
        self.index = state.index
        self.name = state.stage.name
        if state.isReached {
            self.status = .reached
        } else if state.isStoppedHere {
            self.status = .stoppedHere
        } else {
            self.status = .notReached
        }
        self.reachedLine = state.reachedLine
        self.failureMeaning = state.stage.failureMeaning
        self.checkHint = state.stage.checkHint
        self.failureArmLine = state.failureArm?.lineNumber
        self.failureArmText = state.failureArm?.text
    }
}

public enum SubsystemStatus: Equatable, Sendable {
    case reached
    case stoppedHere
    case notReached
}
