import BreenixRuns
import SwiftUI

struct SubsystemsPane: View {
    var viewModel: SubsystemsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Subsystems")
                    .font(.title3.bold())
                Text("\(viewModel.arch), \(viewModel.profile)")
                    .foregroundStyle(.secondary)
                Spacer()
                Text("reached \(viewModel.reachedCount) / \(viewModel.totalCount)")
                    .font(.callout.monospacedDigit())
                if let stoppedIndex = viewModel.stoppedIndex {
                    Text("stopped at #\(stoppedIndex)")
                        .font(.callout.monospacedDigit())
                        .foregroundStyle(.red)
                }
            }

            if viewModel.rows.isEmpty {
                emptyPlaceholder
            } else {
                List(viewModel.rows) { row in
                    SubsystemRow(row: row)
                }
                .listStyle(.inset)
            }
        }
    }

    private var emptyPlaceholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "checklist")
                .font(.system(size: 24))
                .foregroundStyle(.secondary)
            Text("No subsystem stages for this run's architecture")
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct SubsystemRow: View {
    var row: SubsystemRowViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: row.status.symbolName)
                    .foregroundStyle(row.status.color)
                    .frame(width: 16)
                Text(row.name)
                    .fontWeight(row.status == .stoppedHere ? .semibold : .regular)
                Spacer()
                Text(row.reachedLine.map { "L\($0)" } ?? "-")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            if row.status == .stoppedHere {
                VStack(alignment: .leading, spacing: 3) {
                    Text("means: \(row.failureMeaning)")
                    Text("check: \(row.checkHint)")
                    if let line = row.failureArmLine, let text = row.failureArmText {
                        Text("failure arm: L\(line) \(text)")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.leading, 26)
                .textSelection(.enabled)
            }
        }
        .padding(.vertical, 4)
    }
}

private extension SubsystemStatus {
    var symbolName: String {
        switch self {
        case .reached:
            return "checkmark.circle.fill"
        case .stoppedHere:
            return "xmark.octagon.fill"
        case .notReached:
            return "circle"
        }
    }

    var color: Color {
        switch self {
        case .reached:
            return .green
        case .stoppedHere:
            return .red
        case .notReached:
            return .secondary
        }
    }
}
