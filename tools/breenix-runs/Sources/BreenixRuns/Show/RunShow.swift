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
            let gateStdoutText = try store.readGateStdoutText(manifest: manifest)
            sections.append(renderTraces(TracesViewModel.build(serialIndex: serialIndex, gateStdoutText: gateStdoutText)))
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

    private static func renderTraces(_ viewModel: TracesViewModel) -> String {
        [
            renderHostFacts(viewModel.hostFacts),
            renderBXCAP(viewModel.bxcap),
            renderFatalRegs(viewModel.fatalRegs)
        ].joined(separator: "\n\n")
    }

    private static func renderHostFacts(_ records: [BootFactsRecord]) -> String {
        var lines = ["Host facts (#827)"]
        guard !records.isEmpty else {
            lines.append("not present: no [GATE_BOOT_FACTS] records in serial text or gate-stdout.txt")
            return lines.joined(separator: "\n")
        }

        for record in records {
            let lineText = hostFactLineText(record)
            lines.append("boot \(record.boot)\(lineText)")
            for key in record.fields.keys.sorted() {
                lines.append("  \(key)=\(record.fields[key] ?? "")")
            }
        }
        return lines.joined(separator: "\n")
    }

    private static func hostFactLineText(_ record: BootFactsRecord) -> String {
        guard let lineNumber = record.lineNumber else {
            return ""
        }
        if let sourceFile = record.sourceFile {
            return " \(sourceFile):L\(lineNumber)"
        }
        return " L\(lineNumber)"
    }

    private static func renderBXCAP(_ result: BXCAPDecodeResult) -> String {
        var lines = ["Kernel capture (BXCAP v1)"]
        guard !result.captures.isEmpty || !result.refusals.isEmpty else {
            lines.append("not present: no [BXCAP:...] records in serial text")
            return lines.joined(separator: "\n")
        }

        for capture in result.captures {
            let status = capture.truncated ? "truncated" : "complete"
            let verdict = capture.verdict.map { " verdict=\($0)" } ?? ""
            let skipped = capture.sectionsSkipped.map { " sections_skipped=\($0)" } ?? ""
            lines.append("seq \(capture.seq) \(capture.edge ?? "edge?") \(status)\(verdict)\(skipped)")
            lines.append("  rows=\(capture.rows.count) begin=L\(capture.beginLine) end=\(capture.endLine.map(String.init) ?? "-")")
            let qualityRows = capture.rows.compactMap { row -> String? in
                guard let quality = row.fields["q"] else {
                    return nil
                }
                return "\(row.section.rawValue):\(quality)"
            }
            if !qualityRows.isEmpty {
                lines.append("  q=\(qualityRows.joined(separator: ","))")
            }
        }
        for refusal in result.refusals {
            lines.append("refused L\(refusal.startLine): \(refusal.reason) v=\(refusal.version.map(String.init) ?? "?") seq=\(refusal.seq.map(String.init) ?? "?")")
        }
        return lines.joined(separator: "\n")
    }

    private static func renderFatalRegs(_ records: [FatalRegsRecord]) -> String {
        var lines = ["Fatal registers"]
        guard !records.isEmpty else {
            lines.append("not present: no [FATAL_REGS] records in serial text")
            return lines.joined(separator: "\n")
        }

        for record in records {
            let label = record.label ?? "unlabelled"
            let grid = record.hasCompleteRegisterGrid ? "x0...x30 grid complete" : "x0...x30 grid truncated"
            lines.append("\(label) cpu=\(record.cpu.map(String.init) ?? "?") L\(record.startLine)-L\(record.endLine) \(grid)")
            let traceCPU = record.dispatchTraceCPU ?? record.cpu
            lines.append("  DISPATCH_TRACE cpu=\(traceCPU.map(String.init) ?? "?") entries=\(record.dispatchEntries.count)")
            if record.noDispatchesRecorded {
                lines.append("  no dispatches recorded")
            }
        }
        return lines.joined(separator: "\n")
    }
}
