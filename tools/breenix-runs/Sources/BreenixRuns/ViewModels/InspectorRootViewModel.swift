import Foundation
import Combine

@MainActor
public final class InspectorRootViewModel: ObservableObject {
    @Published public private(set) var runs: [RunInspectorLoadedRun] = []
    @Published public private(set) var loadWarnings: [String] = []
    @Published public var selectedRunID: String?
    @Published public var selectedComparisonRunID: String?
    @Published public private(set) var detail: RunDetailViewModel?
    @Published public private(set) var diff: RunDiffResult?
    @Published public private(set) var loadError: String?
    @Published public private(set) var diffError: String?

    private let store: RunStore
    private var watcher: RunStoreWatcher?

    public init(store: RunStore) {
        self.store = store
    }

    deinit {
        watcher?.stop()
    }

    public func startWatchingStore() {
        guard watcher == nil else { return }
        let watcher = RunStoreWatcher(store: store) { [weak self] in
            Task { @MainActor in
                await self?.refresh()
            }
        }
        watcher.start()
        self.watcher = watcher
    }

    public func stopWatchingStore() {
        watcher?.stop()
        watcher = nil
    }

    /// Shared reload action for the toolbar, menu and store watcher.
    public func refresh() async {
        await loadRuns()
    }

    public func loadRuns() async {
        do {
            let loadedList = try await RunInspectorLoader.loadRunList(store: store)
            loadWarnings = loadedList.warnings
            runs = loadedList.runs
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

    public func loadDetail(id: String?) async {
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

    public func loadDiff(id: String?) async {
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
