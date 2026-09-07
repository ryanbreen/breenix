import BreenixRuns
import SwiftUI

@main
struct BreenixRunInspectorApp: App {
    init() {
        // Packaging diagnostic: use the shipped executable and its Bundle.module.
        if CommandLine.arguments.contains("--resource-probe") {
            do {
                print("Bundle.main=\(Bundle.main.bundleURL.path)")
                for arch in [Arch.aarch64, .x86_64] {
                    let stages = try StageCatalog.load(for: arch)
                    print("catalog=\(arch.rawValue) stages=\(stages.count) path=\(StageCatalog.catalogURL(for: arch)!.path)")
                }
                exit(0)
            } catch {
                FileHandle.standardError.write(Data("resource probe: \(error)\n".utf8))
                exit(1)
            }
        }
    }

    var body: some Scene {
        WindowGroup {
            InspectorRootView()
        }
        .commands {
            CommandGroup(after: .newItem) {
                Button("Refresh") {
                    NotificationCenter.default.post(name: .breenixRunsRefreshRequested, object: nil)
                }
                .keyboardShortcut("r", modifiers: [.command])
            }
        }
    }
}

private extension Notification.Name {
    static let breenixRunsRefreshRequested = Notification.Name("breenixRunsRefreshRequested")
}

struct InspectorRootView: View {
    @StateObject private var viewModel = InspectorRootViewModel(store: RunStore.defaultStore())

    var body: some View {
        NavigationSplitView {
            VStack(alignment: .leading) {
                if !viewModel.loadWarnings.isEmpty {
                    Text(viewModel.loadWarnings.joined(separator: "\n"))
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .accessibilityIdentifier("run-load-warnings")
                }
                SidebarView(rows: viewModel.runs.map(\.row), selection: $viewModel.selectedRunID)
            }
            .navigationSplitViewColumnWidth(min: 280, ideal: 360, max: 460)
        } detail: {
            detailView
        }
        .task {
            viewModel.startWatchingStore()
            await viewModel.refresh()
        }
        .onDisappear { viewModel.stopWatchingStore() }
        .onReceive(NotificationCenter.default.publisher(for: .breenixRunsRefreshRequested)) { _ in
            Task { await viewModel.refresh() }
        }
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await viewModel.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .keyboardShortcut("r", modifiers: [.command])
            }
        }
        .onChange(of: viewModel.selectedRunID) { _, newValue in
            Task {
                await viewModel.loadDetail(id: newValue)
            }
        }
        .onChange(of: viewModel.selectedComparisonRunID) { _, newValue in
            Task {
                await viewModel.loadDiff(id: newValue)
            }
        }
    }

    @ViewBuilder
    private var detailView: some View {
        if let detail = viewModel.detail {
            VStack(alignment: .leading, spacing: 12) {
                Picker("Compare with", selection: $viewModel.selectedComparisonRunID) {
                    Text("No comparison").tag(String?.none)
                    ForEach(viewModel.runs.filter { $0.id != detail.manifest.id && $0.manifest.arch == detail.manifest.arch }, id: \.id) { run in
                        Text("\(run.row.arch) \(run.row.profile) \(run.row.timeText) \(run.row.verdictText)")
                            .tag(String?.some(run.id))
                    }
                }
                .frame(maxWidth: 360)

                TabView {
                    SubsystemsPane(viewModel: detail.subsystems)
                        .accessibilityIdentifier("pane-subsystems")
                        .tabItem {
                            Label("Subsystems", systemImage: "checklist")
                        }
                    MessagesPane(messages: detail.messages)
                        .accessibilityIdentifier("pane-messages")
                        .tabItem {
                            Label("Messages", systemImage: "text.alignleft")
                        }
                    TracesPane(viewModel: detail.traces)
                        .accessibilityIdentifier("pane-traces")
                        .tabItem {
                            Label("Traces", systemImage: "waveform.path.ecg")
                        }
                    if viewModel.selectedComparisonRunID != nil {
                        compareTab
                            .tabItem {
                                Label("Compare", systemImage: "arrow.left.arrow.right")
                            }
                    }
                }
            }
            .padding()
        } else if let loadError = viewModel.loadError {
            VStack(alignment: .leading, spacing: 12) {
                Text("Unable to load run")
                    .font(.headline)
                Text(loadError)
                    .font(.body.monospaced())
                    .textSelection(.enabled)
            }
            .padding()
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            VStack(spacing: 10) {
                Image(systemName: "tray")
                    .font(.system(size: 30))
                    .foregroundStyle(.secondary)
                Text(viewModel.runs.isEmpty ? "No runs in the store" : "Select a run")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder
    private var compareTab: some View {
        if let diff = viewModel.diff {
            ComparePane(result: diff)
        } else if let diffError = viewModel.diffError {
            VStack(alignment: .leading, spacing: 12) {
                Text("Unable to compare runs")
                    .font(.headline)
                Text(diffError)
                    .font(.body.monospaced())
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            ProgressView("Loading compare")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

}
