import BreenixRuns
import Foundation

struct TailArguments {
    var selector: String
}

func parseTail(_ args: ArraySlice<String>) throws -> TailArguments {
    let selectors = Array(args)
    guard selectors.count <= 1 else {
        throw CLIError(description: "tail accepts at most one run id")
    }
    if let selector = selectors.first {
        guard !selector.hasPrefix("--") else {
            throw CLIError(description: "unknown tail flag \(selector)")
        }
        return TailArguments(selector: selector)
    }
    return TailArguments(selector: "latest")
}

func runTail(_ arguments: TailArguments, store: RunStore) throws {
    let manifest = try loadManifest(selector: arguments.selector, store: store)
    let url = try SerialTailer.preferredTailURL(manifest: manifest, store: store)
    let tailer = SerialTailer()
    // Stored manifests are written only after today's launchers have finished,
    // so the CLI cannot observe a persisted run that is still in flight yet.
    // This predicate is intentionally true; the reusable tailer below is the
    // part future launcher wiring can point at a concurrent writer.
    try tailer.follow(fileURL: url, isWriterDone: { true }) { data in
        FileHandle.standardOutput.write(data)
    }
}
