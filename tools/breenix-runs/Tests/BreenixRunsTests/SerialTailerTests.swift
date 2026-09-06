import Foundation
@testable import BreenixRuns
import XCTest

final class SerialTailerTests: XCTestCase {
    func testTailerReadsAppendedChunksAndReturnsAfterDoneAtEOF() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let serialURL = root.appendingPathComponent("serial.txt")
        FileManager.default.createFile(atPath: serialURL.path, contents: nil)

        let chunks = [Data("first\n".utf8), Data("second\n".utf8), Data("third\n".utf8)]
        let done = LockedBool(false)
        let writerFinished = expectation(description: "writer finished")
        Thread {
            for chunk in chunks {
                Thread.sleep(forTimeInterval: 0.02)
                append(chunk, to: serialURL)
            }
            done.set(true)
            writerFinished.fulfill()
        }.start()

        let tailer = SerialTailer(pollInterval: 0.005, timeout: 1, stablePollsBeforeDone: 2)
        var received = Data()
        try tailer.follow(fileURL: serialURL, isWriterDone: { done.value }) { data in
            received.append(data)
        }

        wait(for: [writerFinished], timeout: 1)
        XCTAssertEqual(String(decoding: received, as: UTF8.self), "first\nsecond\nthird\n")
    }

    func testTailerTimeoutsWhenDonePredicateNeverTurnsTrue() throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let serialURL = root.appendingPathComponent("serial.txt")
        try Data("prefix\n".utf8).write(to: serialURL)

        let tailer = SerialTailer(pollInterval: 0.005, timeout: 0.05, stablePollsBeforeDone: 1)
        XCTAssertThrowsError(try tailer.follow(fileURL: serialURL, isWriterDone: { false }) { _ in }) { error in
            guard case SerialTailerError.timeout(let path, let seconds) = error else {
                return XCTFail("expected SerialTailerError.timeout, got \(error)")
            }
            XCTAssertEqual(path, serialURL.path)
            XCTAssertEqual(seconds, 0.05)
        }
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-tail-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

private final class LockedBool: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: Bool

    init(_ value: Bool) {
        self.storage = value
    }

    var value: Bool {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func set(_ value: Bool) {
        lock.lock()
        storage = value
        lock.unlock()
    }
}

private func append(_ data: Data, to url: URL) {
    let handle = try! FileHandle(forWritingTo: url)
    try! handle.seekToEnd()
    handle.write(data)
    try! handle.close()
}
