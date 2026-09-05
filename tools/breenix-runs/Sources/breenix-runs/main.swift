import BreenixRuns
import Foundation

struct CLIError: Error, CustomStringConvertible {
    var description: String
}

struct RunArmArguments {
    var profile: ArmProfile = .strict
    var profileWasSet = false
    var boots = 20
    var tags: [String] = []
    var persist = true
}

struct FactsJSONEnvelope: Codable {
    var id: String
    var arch: Arch
    var profile: String
    var host: HostFactsTrace?
}

func usage() -> String {
    """
    Usage:
      breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
      breenix-runs facts <run-id|latest> [--json]
    """
}

func parseRunArm(_ args: ArraySlice<String>) throws -> RunArmArguments {
    var parsed = RunArmArguments()
    var iterator = Array(args).makeIterator()

    while let arg = iterator.next() {
        switch arg {
        case "--boots":
            guard let value = iterator.next(), let boots = Int(value), boots > 0 else {
                throw CLIError(description: "--boots requires a positive integer")
            }
            parsed.boots = boots
        case "--tag":
            guard let value = iterator.next(), !value.isEmpty else {
                throw CLIError(description: "--tag requires a non-empty value")
            }
            parsed.tags.append(value)
        case "--no-store":
            parsed.persist = false
        default:
            guard !arg.hasPrefix("--") else {
                throw CLIError(description: "unknown run arm flag \(arg)")
            }
            guard let profile = ArmProfile(rawValue: arg) else {
                throw CLIError(description: "unknown arm profile \(arg)")
            }
            // Mirrors parseFacts's duplicate-value guard below: a second
            // profile-looking positional argument (`run arm strict prod`) must
            // error, not silently overwrite the first and run the last one.
            guard !parsed.profileWasSet else {
                throw CLIError(description: "run arm accepts exactly one profile, got both \(parsed.profile.rawValue) and \(profile.rawValue)")
            }
            parsed.profile = profile
            parsed.profileWasSet = true
        }
    }

    return parsed
}

func parseFacts(_ args: ArraySlice<String>) throws -> (selector: String, json: Bool) {
    var selector: String?
    var json = false

    for arg in args {
        switch arg {
        case "--json":
            json = true
        default:
            guard !arg.hasPrefix("--") else {
                throw CLIError(description: "unknown facts flag \(arg)")
            }
            guard selector == nil else {
                throw CLIError(description: "facts accepts exactly one run id or latest")
            }
            selector = arg
        }
    }

    guard let selector else {
        throw CLIError(description: "facts requires <run-id|latest>")
    }
    return (selector, json)
}

func repoRoot(startingAt start: URL) throws -> URL {
    var current = start.standardizedFileURL
    while true {
        let gate = current.appendingPathComponent("docker/qemu/run-aarch64-boot-test-strict.sh")
        if FileManager.default.fileExists(atPath: gate.path) {
            return current
        }

        let parent = current.deletingLastPathComponent()
        if parent.path == current.path {
            throw LocalGateLauncherError.repoRootNotFound(start)
        }
        current = parent
    }
}

func loadManifest(selector: String, store: RunStore) throws -> RunManifest {
    if selector == "latest" {
        return try store.latestManifest()
    }
    return try store.readManifest(id: selector)
}

func printJSONFacts(_ manifest: RunManifest) throws {
    let envelope = FactsJSONEnvelope(id: manifest.id, arch: manifest.arch, profile: manifest.profile, host: manifest.host)
    let data = try RunStore.encoder.encode(envelope)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func printFactsBlock(manifest: RunManifest, manifestPath: URL?, includeBootFactsNotice: Bool) {
    print("Run: \(manifest.id)")
    print("Arch: \(manifest.arch.rawValue)")
    print("Profile: \(manifest.profile)")
    print("Kernel BUILD_ID: \(manifest.kernel.buildID ?? "none")")
    print("Git SHA: \(manifest.kernel.gitSHA ?? manifest.host?.start.gitSHA ?? "unknown")")
    print("Git dirty: \(formatBool(manifest.kernel.gitDirty ?? manifest.host?.start.gitDirty))")

    if let host = manifest.host {
        print("")
        print("Host facts trace")
        printSample("Start", host.start)
        printSample("End", host.end)
        printDeltas(start: host.start, end: host.end)
    } else {
        print("")
        print("Host facts trace: unknown")
    }

    if includeBootFactsNotice {
        print("")
        print("[GATE_BOOT_FACTS] record ingestion from the serial is not wired up yet (lands in PR-7 BootFactsParser).")
    }

    if let manifestPath {
        print("")
        print("Stored run id: \(manifest.id)")
        print("Manifest: \(manifestPath.path)")
    } else {
        print("")
        print("Run not stored (--no-store).")
    }
}

func printSample(_ label: String, _ sample: HostFactsSample) {
    print("\(label):")
    print("  wall: \(iso8601(sample.wallTime))")
    print("  qemu peers: aarch64=\(sample.qemuPeersAarch64) x86_64=\(sample.qemuPeersX86_64)")
    print("  loadavg: \(formatDouble(sample.loadavg1)) \(formatDouble(sample.loadavg5)) \(formatDouble(sample.loadavg15))")
    print("  qemu cpu seconds: \(formatSeconds(sample.qemuCPUSeconds))")
    print("  thermal pressure: \(sample.thermalPressure ?? "unavailable")")
    print("  host model: \(sample.hostModel ?? "unknown")")
    print("  phys mem: \(sample.physMem.map(String.init) ?? "unknown")")
    print("  qemu version: \(sample.qemuVersion ?? "unknown")")
    print("  git sha: \(sample.gitSHA ?? "unknown")")
    print("  git dirty: \(formatBool(sample.gitDirty))")
    print("  clock ratio: not sampled in PR-1")
}

func printDeltas(start: HostFactsSample, end: HostFactsSample) {
    print("Deltas:")
    print("  wall duration: \(formatSeconds(end.wallTime.timeIntervalSince(start.wallTime)))")
    print("  qemu peers: aarch64=\(end.qemuPeersAarch64 - start.qemuPeersAarch64) x86_64=\(end.qemuPeersX86_64 - start.qemuPeersX86_64)")
    print("  loadavg: \(formatDelta(start.loadavg1, end.loadavg1)) \(formatDelta(start.loadavg5, end.loadavg5)) \(formatDelta(start.loadavg15, end.loadavg15))")
    if let startCPU = start.qemuCPUSeconds, let endCPU = end.qemuCPUSeconds {
        print("  qemu cpu seconds: \(formatSeconds(endCPU - startCPU))")
    } else {
        print("  qemu cpu seconds: unavailable")
    }
}

func iso8601(_ date: Date) -> String {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return formatter.string(from: date)
}

func formatDouble(_ value: Double?) -> String {
    guard let value else {
        return "unknown"
    }
    return String(format: "%.2f", value)
}

func formatDelta(_ start: Double?, _ end: Double?) -> String {
    guard let start, let end else {
        return "unknown"
    }
    return String(format: "%+.2f", end - start)
}

func formatSeconds(_ value: Double?) -> String {
    guard let value else {
        return "unknown"
    }
    return String(format: "%.2fs", value)
}

func formatBool(_ value: Bool?) -> String {
    guard let value else {
        return "unknown"
    }
    return value ? "true" : "false"
}

func main() -> Int32 {
    do {
        let args = Array(CommandLine.arguments.dropFirst())
        guard let subcommand = args.first else {
            throw CLIError(description: usage())
        }

        let store = RunStore.defaultStore()
        switch subcommand {
        case "run":
            guard args.count >= 2 else {
                throw CLIError(description: "run requires an arch\n\(usage())")
            }
            guard args[1] == "arm" else {
                throw CLIError(description: "run \(args[1]) is not implemented in PR-1")
            }
            let runArgs = try parseRunArm(args.dropFirst(2))
            let root = try repoRoot(startingAt: URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true))
            let launcher = LocalGateLauncher(store: store, repoRoot: root)
            let result = try launcher.runArm(options: LocalGateLaunchOptions(
                profile: runArgs.profile,
                boots: runArgs.boots,
                tags: runArgs.tags,
                persist: runArgs.persist
            ))
            print("")
            printFactsBlock(manifest: result.manifest, manifestPath: result.manifestURL, includeBootFactsNotice: false)
            // A preflight refusal (LocalGateLauncher.bootTestsPreflightRefusalMarker)
            // never ran a boot, but it is still not success: an unmapped verdict
            // here fell through to `return 0`, which reported the CLI's exit
            // status as success for a run that refused to even attempt a boot.
            switch result.manifest.verdict {
            case .gateScript(_, let exitCode):
                return Int32(exitCode)
            case .refused:
                return 1
            default:
                return 0
            }

        case "facts":
            let parsed = try parseFacts(args.dropFirst())
            let manifest = try loadManifest(selector: parsed.selector, store: store)
            if parsed.json {
                try printJSONFacts(manifest)
            } else {
                printFactsBlock(manifest: manifest, manifestPath: store.manifestURL(id: manifest.id), includeBootFactsNotice: true)
            }
            return 0

        default:
            throw CLIError(description: "\(subcommand) is not implemented in PR-1\n\(usage())")
        }
    } catch {
        FileHandle.standardError.write(Data("error: \(error)\n".utf8))
        return 1
    }
}

exit(main())
