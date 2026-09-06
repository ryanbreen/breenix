import Foundation

public struct RunShowOptions: Equatable, Sendable {
    public var subsystems: Bool
    public var messages: Bool
    public var traces: Bool

    public init(subsystems: Bool = false, messages: Bool = false, traces: Bool = false) {
        self.subsystems = subsystems
        self.messages = messages
        self.traces = traces
    }

    public var normalized: RunShowOptions {
        if !subsystems && !messages && !traces {
            return RunShowOptions(subsystems: true)
        }
        return self
    }
}

public enum RunShowError: Error, CustomStringConvertible {
    case serialMissing(URL)

    public var description: String {
        switch self {
        case .serialMissing(let url):
            return "serial file is missing at \(url.path)"
        }
    }
}

public enum RunShow {
    // The stage-name column width for `renderSubsystems`. `String(format:)`'s
    // `%-N@` width flag is silently ignored for NSString substitutions on
    // this Foundation, so alignment is done by hand instead (see `padded`
    // below). Sized past the longest known catalog name (46 chars, x86_64's
    // "Softirq nested interrupt rejection test passed") with headroom so a
    // future catalog addition does not silently misalign the table again;
    // `padded` never truncates, so a name that still exceeds this width
    // widens the column for that row rather than corrupting it.
    private static let stageNameColumnWidth = 50

    public static func render(manifest: RunManifest, store: RunStore, options rawOptions: RunShowOptions) throws -> String {
        let options = rawOptions.normalized
        let serialIndex = try scanSerials(manifest: manifest, store: store)
        var sections: [String] = []

        if options.subsystems {
            let catalog = try StageCatalog.load(for: manifest.arch)
            sections.append(renderSubsystems(manifest: manifest, catalog: catalog, index: serialIndex))
        }
        if options.messages {
            sections.append(renderMessages(index: serialIndex))
        }
        if options.traces {
            sections.append(renderTracesNotice())
        }

        return sections.joined(separator: "\n\n")
    }

    private static func scanSerials(manifest: RunManifest, store: RunStore) throws -> SerialIndex {
        guard !manifest.serials.isEmpty else {
            return try MarkerScanner().scan(data: Data())
        }

        var data = Data()
        for serial in manifest.serials {
            let url = serialURL(serial, manifest: manifest, store: store)
            guard FileManager.default.fileExists(atPath: url.path) else {
                throw RunShowError.serialMissing(url)
            }
            if !data.isEmpty, data.last != 0x0A {
                data.append(0x0A)
            }
            data.append(try Data(contentsOf: url))
        }
        return try MarkerScanner().scan(data: data)
    }

    private static func serialURL(_ serial: SerialRef, manifest: RunManifest, store: RunStore) -> URL {
        if serial.path.hasPrefix("/") {
            return URL(fileURLWithPath: serial.path)
        }
        return store.runDirectory(id: manifest.id).appendingPathComponent(serial.path)
    }

    private static func renderSubsystems(manifest: RunManifest, catalog: [BootStage], index: SerialIndex) -> String {
        let states = StateMachine.evaluate(catalog: catalog, index: index)
        let reached = states.filter(\.isReached).count
        let stop = states.first { $0.isStoppedHere }
        var lines: [String] = []
        let stopText = stop.map { " stopped at #\($0.index)" } ?? ""

        lines.append("Subsystems - \(manifest.arch.rawValue), \(manifest.profile) reached \(reached) / \(states.count)\(stopText)")
        for state in states {
            let symbol: String
            if state.isReached {
                symbol = "✓"
            } else if state.isStoppedHere {
                symbol = "✗"
            } else {
                symbol = "○"
            }

            let lineText = state.reachedLine.map { "L\($0)" } ?? "-"
            lines.append("\(symbol) \(padded(state.stage.name, to: stageNameColumnWidth)) \(lineText)")
            if state.isStoppedHere {
                lines.append("    means: \(state.stage.failureMeaning)")
                lines.append("    check: \(state.stage.checkHint)")
                if let arm = state.failureArm {
                    lines.append("    failure arm: L\(arm.lineNumber) \(arm.text)")
                }
            }
        }
        return lines.joined(separator: "\n")
    }

    // Pads `text` with trailing spaces to `width`; never truncates, so a
    // name longer than `width` still renders in full (just misaligning that
    // one row) rather than being silently cut off.
    private static func padded(_ text: String, to width: Int) -> String {
        guard text.count < width else {
            return text
        }
        return text + String(repeating: " ", count: width - text.count)
    }

    private static func renderMessages(index: SerialIndex) -> String {
        var lines = ["Messages"]
        for line in index.lines {
            let tag = line.hits.first?.family.rawValue ?? "other"
            lines.append("L\(line.lineNumber) [\(tag)] \(line.text)")
        }
        return lines.joined(separator: "\n")
    }

    private static func renderTracesNotice() -> String {
        [
            "Traces",
            "[GATE_BOOT_FACTS] record ingestion from the serial is not wired up yet (lands in PR-7 BootFactsParser).",
            "[BXCAP] record ingestion from the serial is not wired up yet (lands in PR-7 BXCAPDecoder).",
            "[FATAL_REGS] record ingestion from the serial is not wired up yet (lands in PR-7 FatalRegsDecoder)."
        ].joined(separator: "\n")
    }
}
