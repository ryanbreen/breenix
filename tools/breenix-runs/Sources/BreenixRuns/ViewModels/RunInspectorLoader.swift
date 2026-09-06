import Foundation

public struct RunInspectorLoadedRun: Equatable, Sendable {
    public var row: SidebarRowViewModel
    public var manifest: RunManifest

    public var id: String {
        row.id
    }

    public init(row: SidebarRowViewModel, manifest: RunManifest) {
        self.row = row
        self.manifest = manifest
    }
}

public enum RunInspectorLoader {
    public static func loadRuns(store: RunStore) async throws -> [RunInspectorLoadedRun] {
        try await Task.detached(priority: .userInitiated) {
            let index = try store.readIndex()
            var manifests: [RunManifest] = []
            for entry in index.runs {
                manifests.append(try store.readManifest(id: entry.id))
            }
            let rows = SidebarViewModel.rows(for: manifests)
            let manifestsByID = Dictionary(uniqueKeysWithValues: manifests.map { ($0.id, $0) })
            return rows.compactMap { row in
                guard let manifest = manifestsByID[row.id] else {
                    return nil
                }
                return RunInspectorLoadedRun(row: row, manifest: manifest)
            }
        }.value
    }

    public static func loadDetail(manifest: RunManifest, store: RunStore) async throws -> RunDetailViewModel {
        try await Task.detached(priority: .userInitiated) {
            try RunDetailViewModel.load(manifest: manifest, store: store)
        }.value
    }

    public static func loadDiff(lhs: RunManifest, rhs: RunManifest, store: RunStore) async throws -> RunDiffResult {
        try await Task.detached(priority: .userInitiated) {
            try RunDiff.compare(lhs: lhs, rhs: rhs, store: store)
        }.value
    }
}
