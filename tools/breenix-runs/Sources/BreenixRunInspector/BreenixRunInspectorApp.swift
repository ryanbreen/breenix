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
    @State private var detail: RunDetailViewModel?
    @State private var loadError: String?

    var body: some View {
        NavigationSplitView {
            SidebarView(rows: runs.map(\.row), selection: $selectedRunID)
                .navigationSplitViewColumnWidth(min: 280, ideal: 360, max: 460)
        } detail: {
            detailView
        }
        .task {
            loadRuns()
        }
        .onChange(of: selectedRunID) { _, newValue in
            loadDetail(id: newValue)
        }
    }

    @ViewBuilder
    private var detailView: some View {
        if let detail {
            TabView {
                SubsystemsPane(viewModel: detail.subsystems)
                    .tabItem {
                        Label("Subsystems", systemImage: "checklist")
                    }
                MessagesPane(messages: detail.messages)
                    .tabItem {
                        Label("Messages", systemImage: "text.alignleft")
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

    private func loadRuns() {
        do {
            let index = try store.readIndex()
            var manifests: [RunManifest] = []
            for entry in index.runs {
                manifests.append(try store.readManifest(id: entry.id))
            }
            let rows = SidebarViewModel.rows(for: manifests)
            let manifestsByID = Dictionary(uniqueKeysWithValues: manifests.map { ($0.id, $0) })
            runs = rows.compactMap { row in
                guard let manifest = manifestsByID[row.id] else {
                    return nil
                }
                return LoadedRun(row: row, manifest: manifest)
            }
            loadError = nil

            if self.selectedRunID == nil {
                self.selectedRunID = runs.first?.id
            } else if let currentSelection = self.selectedRunID, !runs.contains(where: { $0.id == currentSelection }) {
                self.selectedRunID = runs.first?.id
            }
            loadDetail(id: self.selectedRunID)
        } catch {
            runs = []
            detail = nil
            loadError = String(describing: error)
        }
    }

    private func loadDetail(id: String?) {
        guard let id else {
            detail = nil
            return
        }
        guard let run = runs.first(where: { $0.id == id }) else {
            detail = nil
            return
        }

        do {
            detail = try RunDetailViewModel.load(manifest: run.manifest, store: store)
            loadError = nil
        } catch {
            detail = nil
            loadError = String(describing: error)
        }
    }
}
