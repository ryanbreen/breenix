import BreenixRuns
import SwiftUI

struct TracesPane: View {
    var viewModel: TracesViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Traces")
                    .font(.title3.bold())
                Text(summaryText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer()
            }

            List {
                Section("Host facts (#827)") {
                    if viewModel.hostFacts.isEmpty {
                        EmptyTraceRow(systemName: "externaldrive.badge.questionmark", text: "Not present in serial text or gate-stdout.txt")
                    } else {
                        ForEach(viewModel.hostFacts) { record in
                            BootFactsRow(record: record)
                        }
                    }
                }

                Section("Kernel capture (BXCAP v1)") {
                    if viewModel.bxcap.captures.isEmpty && viewModel.bxcap.refusals.isEmpty {
                        EmptyTraceRow(systemName: "waveform.path.ecg", text: "Not present in serial text")
                    } else {
                        ForEach(viewModel.bxcap.captures) { capture in
                            BXCAPCaptureRow(capture: capture)
                        }
                        ForEach(viewModel.bxcap.refusals) { refusal in
                            BXCAPRefusalRow(refusal: refusal)
                        }
                    }
                }

                Section("Fatal registers") {
                    if viewModel.fatalRegs.isEmpty {
                        EmptyTraceRow(systemName: "cpu", text: "Not present in serial text")
                    } else {
                        ForEach(viewModel.fatalRegs) { record in
                            FatalRegsRow(record: record)
                        }
                    }
                }
            }
            .listStyle(.inset)
        }
    }

    private var summaryText: String {
        "\(viewModel.hostFacts.count) host, \(viewModel.bxcap.captures.count) captures, \(viewModel.fatalRegs.count) fatal"
    }
}

struct EmptyTraceRow: View {
    var systemName: String
    var text: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: systemName)
                .frame(width: 18)
                .foregroundStyle(.secondary)
            Text(text)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private struct BootFactsRow: View {
    var record: BootFactsRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Image(systemName: "externaldrive.connected.to.line.below")
                    .frame(width: 18)
                    .foregroundStyle(.teal)
                Text("boot \(record.boot)")
                    .fontWeight(.semibold)
                Spacer()
                if let lineNumber = record.lineNumber {
                    Text(lineLabel(lineNumber: lineNumber, sourceFile: record.sourceFile))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
            Text(fieldSummary(record.fields))
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private func lineLabel(lineNumber: Int, sourceFile: String?) -> String {
    if let sourceFile {
        return "\(sourceFile):L\(lineNumber)"
    }
    return "L\(lineNumber)"
}

private struct BXCAPCaptureRow: View {
    var capture: BXCAPCapture

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Image(systemName: capture.truncated ? "waveform.path.ecg.rectangle" : "waveform.path.ecg")
                    .frame(width: 18)
                    .foregroundStyle(capture.truncated ? .orange : .green)
                Text("seq \(capture.seq)")
                    .fontWeight(.semibold)
                Text(capture.edge ?? "edge?")
                    .foregroundStyle(.secondary)
                Spacer()
                Text(capture.truncated ? "truncated" : "complete")
                    .font(.caption.monospaced())
                    .foregroundStyle(capture.truncated ? .orange : .green)
            }

            Text("rows=\(capture.rows.count) verdict=\(capture.verdict ?? "-") sections_skipped=\(capture.sectionsSkipped ?? "-")")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)

            let qRows = capture.rows.compactMap { row -> String? in
                row.fields["q"].map { "\(row.section.rawValue):\($0)" }
            }
            if !qRows.isEmpty {
                Text("q=\(qRows.joined(separator: ", "))")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, 4)
    }
}

private struct BXCAPRefusalRow: View {
    var refusal: BXCAPRefusal

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Image(systemName: "exclamationmark.triangle.fill")
                    .frame(width: 18)
                    .foregroundStyle(.red)
                Text("refused")
                    .fontWeight(.semibold)
                Spacer()
                Text("L\(refusal.startLine)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Text("\(refusal.reason) v=\(refusal.version.map(String.init) ?? "?") seq=\(refusal.seq.map(String.init) ?? "?")")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private struct FatalRegsRow: View {
    var record: FatalRegsRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Image(systemName: record.truncated ? "xmark.octagon.fill" : "cpu.fill")
                    .frame(width: 18)
                    .foregroundStyle(record.truncated ? .red : .green)
                Text(record.label ?? "unlabelled")
                    .fontWeight(.semibold)
                Text("cpu \(record.cpu.map(String.init) ?? "?")")
                    .foregroundStyle(.secondary)
                Spacer()
                Text("L\(record.startLine)-L\(record.endLine)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            Text("\(record.hasCompleteRegisterGrid ? "x0...x30 grid complete" : "x0...x30 grid truncated"); DISPATCH_TRACE cpu=\((record.dispatchTraceCPU ?? record.cpu).map(String.init) ?? "?") entries=\(record.dispatchEntries.count)")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private func fieldSummary(_ fields: [String: String]) -> String {
    fields.keys.sorted().map { key in
        "\(key)=\(fields[key] ?? "")"
    }.joined(separator: " ")
}
