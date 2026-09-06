import BreenixRuns
import SwiftUI

struct ComparePane: View {
    var result: RunDiffResult

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Compare")
                    .font(.title3.bold())
                Text("\(result.lhsID) vs \(result.rhsID)")
                    .font(.callout.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
            }

            List {
                Section("Subsystem-state delta") {
                    if result.subsystemDelta.isEmpty {
                        EmptyTraceRow(systemName: "checklist", text: "No subsystem-state delta between these two runs")
                    } else {
                        ForEach(result.subsystemDelta) { row in
                            SubsystemDeltaRowView(row: row)
                        }
                    }
                }

                Section("Marker-count delta") {
                    if result.markerCountDelta.isEmpty {
                        EmptyTraceRow(systemName: "number", text: "No marker-count delta between these two runs")
                    } else {
                        ForEach(result.markerCountDelta) { row in
                            MarkerDeltaRowView(row: row)
                        }
                    }
                }

                Section("Host-facts delta") {
                    switch result.hostFactsDelta {
                    case .notSampled(let lhsSampled, let rhsSampled):
                        EmptyTraceRow(
                            systemName: "externaldrive.badge.questionmark",
                            text: "Not sampled: lhs=\(RunDiff.sampledText(lhsSampled)) rhs=\(RunDiff.sampledText(rhsSampled))"
                        )
                    case .sampled(let values):
                        if values.hasDelta {
                            HostFactsDeltaRow(values: values)
                        } else {
                            EmptyTraceRow(systemName: "externaldrive.badge.checkmark", text: "No host-facts delta between these two runs")
                        }
                    }
                }

                Section("Verdict delta") {
                    if result.verdictDelta.differs {
                        VerdictDeltaRow(delta: result.verdictDelta)
                    } else {
                        EmptyTraceRow(systemName: "checkmark.seal", text: "No verdict delta between these two runs")
                    }
                }
            }
            .listStyle(.inset)
        }
    }
}

private struct SubsystemDeltaRowView: View {
    var row: SubsystemDeltaRow

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Image(systemName: "arrow.left.arrow.right")
                    .frame(width: 18)
                    .foregroundStyle(.orange)
                Text(row.stageName)
                    .fontWeight(.semibold)
                Spacer()
            }
            Text("lhs=\(RunDiff.stageText(row.lhs)) rhs=\(RunDiff.stageText(row.rhs))")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private struct MarkerDeltaRowView: View {
    var row: MarkerCountDeltaRow

    var body: some View {
        HStack {
            Image(systemName: "number")
                .frame(width: 18)
                .foregroundStyle(.teal)
            Text(row.family.rawValue)
                .fontWeight(.semibold)
            Spacer()
            Text("lhs=\(row.lhsCount) rhs=\(row.rhsCount)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
    }
}

private struct HostFactsDeltaRow: View {
    var values: HostFactsComparison

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Label("Start sample", systemImage: "externaldrive.connected.to.line.below")
                .fontWeight(.semibold)
            Text("wall=\(RunDiff.formatSignedSeconds(values.startWallDeltaSeconds)) qemu.aarch64=\(RunDiff.formatSignedInt(values.qemuPeersAarch64Delta)) qemu.x86_64=\(RunDiff.formatSignedInt(values.qemuPeersX86_64Delta)) loadavg1=\(RunDiff.formatSignedDouble(values.loadavg1Delta))")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}

private struct VerdictDeltaRow: View {
    var delta: VerdictDelta

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Label("Verdict changed", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text("lhs=\(delta.lhsText) (\(RunDiff.verdictStateText(delta.lhsState))) rhs=\(delta.rhsText) (\(RunDiff.verdictStateText(delta.rhsState)))")
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
    }
}
