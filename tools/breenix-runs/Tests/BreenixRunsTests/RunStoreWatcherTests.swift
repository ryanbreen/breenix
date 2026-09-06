import Foundation
@testable import BreenixRuns
import XCTest

final class RunStoreWatcherTests: XCTestCase {
    func testCheckNowFiresOnChangeWhenIndexModificationTimeChanges() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        try store.prepareRoot()
        try Data("{}".utf8).write(to: store.indexURL)
        var fireCount = 0
        let watcher = RunStoreWatcher(store: store, onChange: { fireCount += 1 })
        watcher.checkNow()
        XCTAssertEqual(fireCount, 0)
        let old = try XCTUnwrap(FileManager.default.attributesOfItem(atPath: store.indexURL.path)[.modificationDate] as? Date)
        try Data("{\"changed\":true}".utf8).write(to: store.indexURL)
        try FileManager.default.setAttributes([.modificationDate: old.addingTimeInterval(10)], ofItemAtPath: store.indexURL.path)
        watcher.checkNow()
        XCTAssertEqual(fireCount, 1)
        watcher.checkNow()
        XCTAssertEqual(fireCount, 1)
    }

    func testCheckNowDetectsIndexCreatedAfterMissingBaseline() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RunStore(root: root)
        var fireCount = 0
        let watcher = RunStoreWatcher(store: store, onChange: { fireCount += 1 })
        watcher.checkNow()
        XCTAssertEqual(fireCount, 0)
        try store.prepareRoot()
        try Data("{}".utf8).write(to: store.indexURL)
        watcher.checkNow()
        XCTAssertEqual(fireCount, 1)
    }
}
