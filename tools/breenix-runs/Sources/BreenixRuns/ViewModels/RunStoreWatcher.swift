import Foundation

/// Polls index.json on a background timer. checkNow() supports deterministic checks.
/// Mutable polling state is protected by lock; callbacks execute outside that lock.
/// Callers must arrange their own callback isolation (the root model hops to MainActor).
public final class RunStoreWatcher: @unchecked Sendable {
    /// Filesystem-stat polling has no benefit below this cadence; a caller
    /// requesting a smaller interval is clamped here rather than allowed to
    /// spin the poll timer.
    public static let minimumPollInterval: TimeInterval = 1

    private let store: RunStore
    let pollInterval: TimeInterval
    private let onChange: () -> Void
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?
    private var lastModified: Date?
    private var primed = false

    public init(store: RunStore, pollInterval: TimeInterval = 5, onChange: @escaping () -> Void) {
        self.store = store
        self.pollInterval = max(pollInterval, Self.minimumPollInterval)
        self.onChange = onChange
    }

    deinit { stop() }

    public func start() {
        lock.lock()
        defer { lock.unlock() }
        guard timer == nil else { return }
        lastModified = currentModificationDate()
        primed = true
        let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        timer.schedule(deadline: .now() + pollInterval, repeating: pollInterval)
        timer.setEventHandler { [weak self] in self?.checkNow() }
        self.timer = timer
        timer.resume()
    }

    public func stop() {
        lock.lock()
        defer { lock.unlock() }
        timer?.cancel()
        timer = nil
    }

    /// The first check establishes a baseline, including when the index is absent.
    public func checkNow() {
        lock.lock()
        let modified = currentModificationDate()
        let changed = primed && modified != lastModified
        primed = true
        lastModified = modified
        lock.unlock()
        if changed { onChange() }
    }

    private func currentModificationDate() -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: store.indexURL.path)[.modificationDate]) as? Date
    }
}
