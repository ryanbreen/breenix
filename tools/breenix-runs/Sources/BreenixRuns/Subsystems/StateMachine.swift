import Foundation

public struct FailureArm: Equatable, Sendable {
    public var subject: String
    public var lineNumber: Int
    public var text: String

    public init(subject: String, lineNumber: Int, text: String) {
        self.subject = subject
        self.lineNumber = lineNumber
        self.text = text
    }
}

public enum StageOutcome: Equatable, Sendable {
    case reached(line: Int)
    case notReached(failureArm: FailureArm?)
}

public struct StageState: Equatable, Sendable {
    public var index: Int
    public var stage: BootStage
    public var outcome: StageOutcome
    public var isStoppedHere: Bool

    public init(index: Int, stage: BootStage, outcome: StageOutcome, isStoppedHere: Bool) {
        self.index = index
        self.stage = stage
        self.outcome = outcome
        self.isStoppedHere = isStoppedHere
    }

    public var isReached: Bool {
        if case .reached = outcome {
            return true
        }
        return false
    }

    public var reachedLine: Int? {
        if case .reached(let line) = outcome {
            return line
        }
        return nil
    }

    public var failureArm: FailureArm? {
        if case .notReached(let arm) = outcome {
            return arm
        }
        return nil
    }
}

public enum StateMachine {
    public static func evaluate(catalog: [BootStage], index: SerialIndex) -> [StageState] {
        let failureArms = detectedFailureArms(in: index)
        var states: [StageState] = catalog.enumerated().map { offset, stage in
            if let line = firstMarkerLine(for: stage, in: index) {
                return StageState(index: offset + 1, stage: stage, outcome: .reached(line: line), isStoppedHere: false)
            }

            return StageState(
                index: offset + 1,
                stage: stage,
                outcome: .notReached(failureArm: failureArms.first { correlates($0, to: stage) }),
                isStoppedHere: false
            )
        }

        if let stopIndex = states.firstIndex(where: { !$0.isReached }) {
            states[stopIndex].isStoppedHere = true
        }
        return states
    }

    public static func evaluate(catalog: [BootStage], serialText: String) throws -> [StageState] {
        try evaluate(catalog: catalog, index: MarkerScanner().scan(data: Data(serialText.utf8)))
    }

    private static func firstMarkerLine(for stage: BootStage, in index: SerialIndex) -> Int? {
        let markers = markerAlternatives(stage.marker)
        for line in index.lines {
            if markers.contains(where: { marker in line.text.contains(marker) }) {
                return line.lineNumber
            }
        }
        return nil
    }

    private static func markerAlternatives(_ marker: String) -> [String] {
        if marker.contains("|") {
            return marker.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        }
        return [marker]
    }

    private static func detectedFailureArms(in index: SerialIndex) -> [FailureArm] {
        index.lines.compactMap { line in
            if let subject = capture(line.text, pattern: #"\[boot\]\s+(.+?)\s+init failed:"#) {
                return FailureArm(subject: subject, lineNumber: line.lineNumber, text: line.text)
            }
            if let subject = capture(line.text, pattern: #"\[boot\]\s+(.+?)\s+failed:"#) {
                return FailureArm(subject: subject, lineNumber: line.lineNumber, text: line.text)
            }
            if let subject = capture(line.text, pattern: #"\[boot\]\s+No\s+(.+?)\s+found\b"#) {
                return FailureArm(subject: subject, lineNumber: line.lineNumber, text: line.text)
            }
            return nil
        }
    }

    private static func capture(_ text: String, pattern: String) -> String? {
        guard let regex = try? NSRegularExpression(pattern: pattern) else {
            return nil
        }
        let nsText = text as NSString
        let range = NSRange(location: 0, length: nsText.length)
        guard let match = regex.firstMatch(in: text, range: range), match.numberOfRanges > 1 else {
            return nil
        }
        return nsText.substring(with: match.range(at: 1)).trimmingCharacters(in: .whitespaces)
    }

    private static func correlates(_ arm: FailureArm, to stage: BootStage) -> Bool {
        // Failure-arm text is not catalog-keyed. Keep the heuristic conservative:
        // correlate only when the captured subsystem phrase and stage name contain
        // one another after case folding, and otherwise leave the stage as plain
        // notReached rather than guessing.
        let subject = arm.subject.lowercased()
        let stageName = stage.name.lowercased()
        return stageName.contains(subject) || subject.contains(stageName)
    }
}
