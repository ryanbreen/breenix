import BreenixRuns
import Foundation

func runList(_ args: ArraySlice<String>, store: RunStore) throws {
    var filters: [String: String] = [:]
    var iterator = args.makeIterator()
    while let flag = iterator.next() {
        guard ["--arch", "--profile", "--verdict"].contains(flag),
              let value = iterator.next(), !value.hasPrefix("--"), filters[flag] == nil else {
            throw CLIError(description: "list accepts --arch <aarch64|x86_64>, --profile <name>, --verdict <pass|fail|attributed|running|unknown>")
        }
        filters[flag] = value
    }
    if let arch = filters["--arch"], Arch(rawValue: arch) == nil {
        throw CLIError(description: "unknown list architecture \(arch)")
    }
    if let verdict = filters["--verdict"], !["pass", "fail", "attributed", "running", "unknown"].contains(verdict) {
        throw CLIError(description: "unknown list verdict \(verdict)")
    }
    let entries = try store.readIndex().runs
    print("ID\tSTARTED\tARCH/PROFILE\tVERDICT\tSOURCE\tCAPTURE")
    for row in SidebarViewModel.rows(for: entries) {
        let manifest: RunManifest
        do { manifest = try store.readManifest(id: row.id) }
        catch {
            FileHandle.standardError.write(Data("warning: skipping \(row.id): \(error)\n".utf8))
            continue
        }
        let verdict: String
        switch row.verdictState {
        case .success: verdict = "pass"
        case .failure: verdict = "fail"
        case .attributed: verdict = "attributed"
        case .inFlight: verdict = "running"
        case .unknown: verdict = "unknown"
        }
        guard filters["--arch"].map({ $0 == row.arch }) ?? true,
              filters["--profile"].map({ $0 == row.profile }) ?? true,
              filters["--verdict"].map({ $0 == verdict }) ?? true else { continue }
        let source: String
        switch manifest.verdictSource {
        case .gateScript: source = "gate"
        case .imported: source = "imported"
        case .none: source = "none"
        }
        let refs = manifest.serials.map { ($0.path, $0.bytes) } + manifest.captures.map { ($0.path, $0.bytes) }
        let missing = refs.filter { path, _ in
            let url = path.hasPrefix("/") ? URL(fileURLWithPath: path) : store.runDirectory(id: row.id).appendingPathComponent(path)
            return !FileManager.default.isReadableFile(atPath: url.path)
        }.count
        // File presence is narrower than asserting the original capture was complete.
        let health = refs.isEmpty ? "none" : missing > 0 ? "unreadable:\(missing)/\(refs.count)" : "files-present:\(refs.count)"
        print("\(row.id)\t\(iso8601(row.startedAt))\t\(row.arch)/\(row.profile)\t\(row.verdictText)\t\(source)\t\(health)")
    }
}
