import Foundation
@testable import BreenixRuns
import XCTest

@MainActor
final class InspectorRootViewModelTests: XCTestCase {
    func testRefreshPicksUpARunWrittenAfterTheInitialLoad() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        let first = sampleManifest(id: "20260906T000000Z-aarch64-testing-first")
        try store.writeManifest(first)
        let viewModel = InspectorRootViewModel(store: store)
        await viewModel.refresh()
        XCTAssertEqual(viewModel.runs.map(\.id), [first.id])
        let second = sampleManifest(id: "20260906T010000Z-aarch64-testing-second")
        try store.writeManifest(second)
        await viewModel.refresh()
        XCTAssertEqual(Set(viewModel.runs.map(\.id)), [first.id, second.id])
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-loader-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func sampleManifest(
        id: String,
        startedAt: Date = Date(timeIntervalSince1970: 1_788_633_600),
        verdict: Verdict = .fail("fixture panic")
    ) -> RunManifest {
        RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: startedAt.addingTimeInterval(60),
            arch: .aarch64,
            profile: "testing",
            launcher: .imported,
            kernel: KernelIdentity(buildID: "006a9bb0022747", gitSHA: "7a19f550", gitDirty: true),
            host: nil,
            verdict: verdict,
            verdictSource: .imported,
            serials: [],
            captures: [],
            command: [],
            env: [:],
            tags: ["fixture"],
            notes: nil
        )
    }
}
