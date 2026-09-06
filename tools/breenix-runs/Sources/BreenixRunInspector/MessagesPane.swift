import BreenixRuns
import SwiftUI

struct MessagesPane: View {
    var messages: [MessageLineViewModel]

    @State private var selectedBuckets = Set(MessageFamilyBucket.allCases)
    @State private var searchText = ""
    @State private var hideHeartbeats = true

    private var filteredMessages: [MessageLineViewModel] {
        messages.filter { message in
            MessageFilter.includes(
                message.line,
                selectedBuckets: selectedBuckets,
                searchText: searchText
            ) && (!hideHeartbeats || message.bucket != .heartbeat)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Messages")
                    .font(.title3.bold())
                Text("\(filteredMessages.count) / \(messages.count)")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer()
                Toggle("Hide heartbeats", isOn: $hideHeartbeats)
                    .toggleStyle(.checkbox)
            }

            HStack(spacing: 12) {
                TextField("Search serial", text: $searchText)
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 220)

                ForEach(MessageFamilyBucket.allCases, id: \.self) { bucket in
                    Toggle(bucket.label, isOn: bucketBinding(bucket))
                        .toggleStyle(.checkbox)
                }
            }

            if filteredMessages.isEmpty {
                emptyPlaceholder
            } else {
                List(filteredMessages) { message in
                    MessageRow(message: message)
                }
                .listStyle(.inset)
            }
        }
    }

    private var emptyPlaceholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "line.3.horizontal.decrease.circle")
                .font(.system(size: 24))
                .foregroundStyle(.secondary)
            Text(messages.isEmpty ? "No messages in this run" : "No messages match the current filter")
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func bucketBinding(_ bucket: MessageFamilyBucket) -> Binding<Bool> {
        Binding(
            get: {
                selectedBuckets.contains(bucket)
            },
            set: { enabled in
                if enabled {
                    selectedBuckets.insert(bucket)
                } else {
                    selectedBuckets.remove(bucket)
                }
            }
        )
    }
}

private struct MessageRow: View {
    var message: MessageLineViewModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("L\(message.lineNumber)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 52, alignment: .trailing)
            Text(message.familyText)
                .font(.caption.monospaced())
                .foregroundStyle(message.bucket.color)
                .frame(width: 150, alignment: .leading)
            Text(message.text)
                .font(.body.monospaced())
                .textSelection(.enabled)
        }
        .padding(.vertical, 2)
    }
}

private extension MessageFamilyBucket {
    var color: Color {
        switch self {
        case .boot:
            return .teal
        case .tests:
            return .green
        case .oracles:
            return .purple
        case .heartbeat:
            return .blue
        case .faults:
            return .red
        case .traceNoise:
            return .orange
        case .other:
            return .secondary
        }
    }
}
