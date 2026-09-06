import BreenixRuns
import SwiftUI

@main
struct BreenixRunInspectorApp: App {
    var body: some Scene {
        WindowGroup {
            InspectorRootView()
        }
    }
}

private struct LoadedRun: Identifiable, Equatable {
    var id: String { row.id }
    var row: SidebarRowViewModel
    var manifest: RunManifest
}

struct InspectorRootView: View {
    private let store = RunStore.defaultStore()

    @State private var runs: [LoadedRun] = []
    @State private var selectedRunID: String?
    @State private var selectedComparisonRunID: String?
    @State private var detail: RunDetailViewModel?
    @State private var diff: RunDiffResult?
    @State private var loadError: String?
    @State private var diffError: String?

    var body: some View {
        NavigationSplitView {
            SidebarView(rows: runs.map(\.row), selection: $selectedRunID)
                .navigationSplitViewColumnWidth(min: 280, ideal: 360, max: 460)
        } detail: {
            detailView
        }
        .task {
            await loadRuns()
        }
        .onChange(of: selectedRunID) { _, newValue in
            Task {
                await loadDetail(id: newValue)
            }
        }
        .onChange(of: selectedComparisonRunID) { _, newValue in
            Task {
                await loadDiff(id: newValue)
            }
        }
    }

    @ViewBuilder
    private var detailView: some View {
        if let detail {
            VStack(alignment: .leading, spacing: 12) {
                Picker("Compare with", selection: $selectedComparisonRunID) {
                    Text("No comparison").tag(String?.none)
                    ForEach(runs.filter { $0.id != detail.manifest.id && $0.manifest.arch == detail.manifest.arch }) { run in
                        Text("\(run.row.arch) \(run.row.profile) \(run.row.timeText) \(run.row.verdictText)")
                            .tag(String?.some(run.id))
                    }
                }
                .frame(maxWidth: 360)

                TabView {
                    SubsystemsPane(viewModel: detail.subsystems)
                        .tabItem {
                            Label("Subsystems", systemImage: "checklist")
                        }
                    MessagesPane(messages: detail.messages)
                        .tabItem {
                            Label("Messages", systemImage: "text.alignleft")
                        }
                    TracesPane(viewModel: detail.traces)
                        .tabItem {
                            Label("Traces", systemImage: "waveform.path.ecg")
                        }
                    if selectedComparisonRunID != nil {
                        compareTab
                            .tabItem {
                                Label("Compare", systemImage: "arrow.left.arrow.right")
                            }
                    }
                }
            }
            .padding()
        } else if let loadError {
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
                Text(runs.isEmpty ? "No runs in the store" : "Select a run")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder
    private var compareTab: some View {
        if let diff {
            ComparePane(result: diff)
        } else if let diffError {
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

    private func loadRuns() async {
        do {
            let loadedRuns = try await RunInspectorLoader.loadRuns(store: store)
            runs = loadedRuns.map { LoadedRun(row: $0.row, manifest: $0.manifest) }
            loadError = nil

            if self.selectedRunID == nil {
                self.selectedRunID = runs.first?.id
            } else if let currentSelection = self.selectedRunID, !runs.contains(where: { $0.id == currentSelection }) {
                self.selectedRunID = runs.first?.id
            }
            if let comparison = self.selectedComparisonRunID, !runs.contains(where: { $0.id == comparison }) {
                self.selectedComparisonRunID = nil
            }
            await loadDetail(id: self.selectedRunID)
        } catch {
            runs = []
            detail = nil
            diff = nil
            loadError = String(describing: error)
            diffError = nil
        }
    }

    private func loadDetail(id: String?) async {
        guard let id else {
            guard selectedRunID == nil else {
                return
            }
            detail = nil
            return
        }
        guard selectedRunID == id else {
            return
        }
        guard let run = runs.first(where: { $0.id == id }) else {
            detail = nil
            diff = nil
            return
        }

        do {
            let loadedDetail = try await RunInspectorLoader.loadDetail(manifest: run.manifest, store: store)
            guard selectedRunID == id else {
                return
            }
            detail = loadedDetail
            loadError = nil
            if selectedComparisonRunID == id {
                selectedComparisonRunID = nil
            }
            await loadDiff(id: selectedComparisonRunID)
        } catch {
            guard selectedRunID == id else {
                return
            }
            detail = nil
            diff = nil
            loadError = String(describing: error)
        }
    }

    private func loadDiff(id: String?) async {
        guard let id, let detail else {
            diff = nil
            diffError = nil
            return
        }
        guard let rhs = runs.first(where: { $0.id == id }) else {
            diff = nil
            diffError = nil
            return
        }

        diff = nil
        diffError = nil
        do {
            let loadedDiff = try await RunInspectorLoader.loadDiff(lhs: detail.manifest, rhs: rhs.manifest, store: store)
            guard selectedRunID == detail.manifest.id, selectedComparisonRunID == id else {
                return
            }
            diff = loadedDiff
            diffError = nil
        } catch {
            guard selectedRunID == detail.manifest.id, selectedComparisonRunID == id else {
                return
            }
            diff = nil
            diffError = String(describing: error)
        }
    }
}
