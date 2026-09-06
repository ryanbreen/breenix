import BreenixRuns
import SwiftUI

struct SidebarView: View {
    var rows: [SidebarRowViewModel]
    @Binding var selection: String?

    var body: some View {
        List(rows, selection: $selection) { row in
            SidebarRow(row: row)
                .tag(row.id)
        }
    }
}

private struct SidebarRow: View {
    var row: SidebarRowViewModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Circle()
                .fill(row.verdictState.color)
                .frame(width: 8, height: 8)
                .accessibilityLabel(row.verdictState.accessibilityLabel)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(row.arch)
                        .fontWeight(.semibold)
                    Text(row.profile)
                    Spacer(minLength: 4)
                    Text(row.timeText)
                        .foregroundStyle(.secondary)
                }
                .font(.caption)

                HStack(spacing: 8) {
                    Text(row.verdictText)
                        .font(.caption.monospaced())
                        .foregroundStyle(row.verdictState.textColor)
                    Spacer(minLength: 4)
                    Text(row.shortSHA)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.vertical, 3)
    }
}

private extension VerdictDisplayState {
    var color: Color {
        switch self {
        case .success:
            return .green
        case .failure:
            return .red
        case .attributed:
            return .orange
        case .inFlight:
            return .blue
        case .unknown:
            return .gray
        }
    }

    var textColor: Color {
        switch self {
        case .success:
            return .green
        case .failure:
            return .red
        case .attributed:
            return .orange
        case .inFlight:
            return .blue
        case .unknown:
            return .secondary
        }
    }

    var accessibilityLabel: String {
        switch self {
        case .success:
            return "passing"
        case .failure:
            return "failing"
        case .attributed:
            return "attributed"
        case .inFlight:
            return "running"
        case .unknown:
            return "unknown"
        }
    }
}
