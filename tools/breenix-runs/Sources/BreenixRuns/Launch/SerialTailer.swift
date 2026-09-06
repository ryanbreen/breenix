import Foundation

public enum SerialTailerError: Error, Equatable, CustomStringConvertible {
    case timeout(path: String, seconds: TimeInterval)

    public var description: String {
        switch self {
        case .timeout(let path, let seconds):
            return "timed out after \(String(format: "%.2f", seconds))s while following \(path)"
        }
    }
}

public struct SerialTailer: Sendable {
    public var pollInterval: TimeInterval
    public var timeout: TimeInterval
    public var stablePollsBeforeDone: Int

    public init(
        pollInterval: TimeInterval = 0.25,
        timeout: TimeInterval = 60,
        stablePollsBeforeDone: Int = 2
    ) {
        self.pollInterval = pollInterval
        self.timeout = timeout
        self.stablePollsBeforeDone = stablePollsBeforeDone
    }

    public func follow(
        fileURL: URL,
        isWriterDone: () throws -> Bool,
        sink: (Data) throws -> Void
    ) throws {
        let start = Date()
        var offset: UInt64 = 0
        var stablePolls = 0

        while true {
            let size = fileSize(fileURL)
            if size > offset {
                let chunk = try readChunk(fileURL, offset: offset)
                offset = size
                stablePolls = 0
                if !chunk.isEmpty {
                    try sink(chunk)
                }
            } else {
                stablePolls += 1
                if stablePolls >= stablePollsBeforeDone, try isWriterDone() {
                    return
                }
            }

            if Date().timeIntervalSince(start) >= timeout {
                throw SerialTailerError.timeout(path: fileURL.path, seconds: timeout)
            }
            Thread.sleep(forTimeInterval: pollInterval)
        }
    }

    public static func preferredTailURL(manifest: RunManifest, store: RunStore) throws -> URL {
        if let gateStdout = manifest.captures.first(where: { $0.name == "gate-stdout.txt" }) {
            return store.captureURL(gateStdout, manifest: manifest)
        }
        guard let serial = manifest.serials.first else {
            throw RunShowError.serialMissing(store.runDirectory(id: manifest.id).appendingPathComponent("serial.txt"))
        }
        if serial.path.hasPrefix("/") {
            return URL(fileURLWithPath: serial.path)
        }
        return store.runDirectory(id: manifest.id).appendingPathComponent(serial.path)
    }

    private func fileSize(_ url: URL) -> UInt64 {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else {
            return 0
        }
        return size.uint64Value
    }

    private func readChunk(_ url: URL, offset: UInt64) throws -> Data {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        try handle.seek(toOffset: offset)
        return handle.readDataToEndOfFile()
    }
}
