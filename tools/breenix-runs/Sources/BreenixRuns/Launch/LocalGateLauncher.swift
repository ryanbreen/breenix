import Foundation

public enum LocalGateLauncherError: Error, Equatable, CustomStringConvertible {
    case unsupportedArch(String)
    case unsupportedProfile(String)
    case qemuAlreadyRunning(String)
    case repoRootNotFound(URL)

    public var description: String {
        switch self {
        case .unsupportedArch(let arch):
            return "unsupported arch \(arch); PR-1 implements only run arm"
        case .unsupportedProfile(let profile):
            return "unsupported arm profile \(profile)"
        case .qemuAlreadyRunning(let detail):
            return detail
        case .repoRootNotFound(let start):
            return "could not find Breenix repo root from \(start.path)"
        }
    }
}

public protocol HostLock {
    func acquire(runner: ProcessRunner) throws
}

// DESIGN.md Sec 1.6: "a host-wide lock (docker/qemu/lib/qemu-host-lock.sh) lands
// today. The Inspector's local launcher acquires that lock when it exists and
// refuses to launch when it does not exist and another qemu-system-aarch64 is
// already running". PR-1 does not yet probe for that script (tracked as a
// follow-up); until then this is the fallback half of that sentence: a plain
// `pgrep` refusal so PR-1 cannot reintroduce the #826 host-contention confound
// by launching a second concurrent aarch64 boot on this Mac.
public struct FallbackQEMUHostLock: HostLock {
    public init() {}

    public func acquire(runner: ProcessRunner) throws {
        let result = try runner.run(ProcessRequest(executable: "/usr/bin/pgrep", arguments: ["-x", "qemu-system-aarch64"]))
        let pids = result.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)
        if !pids.isEmpty {
            throw LocalGateLauncherError.qemuAlreadyRunning(
                "refusing to launch: qemu-system-aarch64 is already running (\(pids.replacingOccurrences(of: "\n", with: ", ")))"
            )
        }
    }
}

public enum ArmProfile: String, CaseIterable, Sendable {
    case strict
    case prod
    case testing

    // DESIGN.md Sec 5.1: "The three arm profiles map to
    // run-aarch64-boot-test-strict.sh / run-aarch64-prod-profile-boot-test.sh /
    // run-aarch64-testing-profile-boot-test.sh."
    var scriptName: String {
        switch self {
        case .strict:
            return "run-aarch64-boot-test-strict.sh"
        case .prod:
            return "run-aarch64-prod-profile-boot-test.sh"
        case .testing:
            return "run-aarch64-testing-profile-boot-test.sh"
        }
    }
}

public struct LocalGateLaunchOptions: Sendable {
    public var profile: ArmProfile
    public var boots: Int
    public var tags: [String]
    public var persist: Bool

    public init(profile: ArmProfile = .strict, boots: Int = 20, tags: [String] = [], persist: Bool = true) {
        self.profile = profile
        self.boots = boots
        self.tags = tags
        self.persist = persist
    }
}

public struct LocalGateLaunchResult: Sendable {
    public var manifest: RunManifest
    public var runDirectory: URL
    public var manifestURL: URL?
    public var gateStdoutURL: URL
    public var serialURL: URL
    public var stored: Bool

    public init(
        manifest: RunManifest,
        runDirectory: URL,
        manifestURL: URL?,
        gateStdoutURL: URL,
        serialURL: URL,
        stored: Bool
    ) {
        self.manifest = manifest
        self.runDirectory = runDirectory
        self.manifestURL = manifestURL
        self.gateStdoutURL = gateStdoutURL
        self.serialURL = serialURL
        self.stored = stored
    }
}

public struct LocalGateLauncher {
    public var store: RunStore
    public var repoRoot: URL
    public var runner: ProcessRunner
    public var hostLock: HostLock

    // The exact refusal text `require_boot_tests_kernel()` prints in
    // docker/qemu/run-aarch64-boot-test-strict.sh:194-217 before exiting 1 --
    // "Error: $kernel was not built with --features boot_tests." -- with no
    // OUTPUT_DIR or serial.txt ever created. This is a preflight refusal, not a
    // boot outcome (DESIGN.md Sec 5.1: "the launcher surfaces that refusal as a
    // first-class run state .refused(preflight) -- a run that never booted,
    // distinct from a run that failed"). Matched against gate-stdout.txt rather
    // than the serial (which the preflight never gets far enough to write).
    private static let bootTestsPreflightRefusalMarker = "was not built with --features boot_tests"

    public init(store: RunStore, repoRoot: URL, runner: ProcessRunner = RealProcessRunner(), hostLock: HostLock = FallbackQEMUHostLock()) {
        self.store = store
        self.repoRoot = repoRoot
        self.runner = runner
        self.hostLock = hostLock
    }

    public func runArm(options: LocalGateLaunchOptions) throws -> LocalGateLaunchResult {
        let startedAt = Date()
        let id = RunManifest.makeID(startedAt: startedAt, arch: .aarch64, profile: options.profile.rawValue)

        // Refuse before touching disk: acquiring the host lock ahead of creating
        // any run directory means a refused launch (another qemu-system-aarch64
        // already running) leaves no orphaned, empty, manifest-less directory
        // behind (it used to run after directory creation, littering one such
        // directory per refused attempt).
        try hostLock.acquire(runner: runner)

        let runDirectory = try prepareRunDirectory(id: id, persist: options.persist)
        // `--no-store` means exactly that: nothing from this run survives past
        // the process. Discard the whole scratch directory (merged serial.txt,
        // gate-stdout.txt, qemu.log, and gate-tmp/ below) on every exit path,
        // including a thrown error partway through the gate run.
        defer {
            if !options.persist {
                try? FileManager.default.removeItem(at: runDirectory)
            }
        }

        // gate-tmp/ exists solely so BREENIX_GATE_TMP can point the gate script
        // at a directory this run owns exclusively (DESIGN.md Sec 1.6 / Sec 1.1):
        // the universal escape hatch every gate already honours, so no gate
        // script needs modifying to be captured. It is pure staging --
        // harvestSerials() below copies its bytes into the run's own
        // serial.txt -- so it is removed once harvested rather than kept as a
        // second, permanent copy of every per-boot serial (the storage layout
        // in DESIGN.md Sec 3.2 does not list a gate-tmp/ entry under runs/<id>/).
        let gateTmp = runDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        try FileManager.default.createDirectory(at: gateTmp, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: gateTmp)
        }

        let startFacts = try HostFacts.sample(runner: runner, repoRoot: repoRoot, wallTime: startedAt)
        let scriptURL = repoRoot.appendingPathComponent("docker/qemu/\(options.profile.scriptName)")
        let command = [scriptURL.path, "\(options.boots)"]
        let env = ["BREENIX_GATE_TMP": gateTmp.path]
        let gateStdoutURL = runDirectory.appendingPathComponent("gate-stdout.txt")
        FileManager.default.createFile(atPath: gateStdoutURL.path, contents: nil)
        let gateOutputHandle = try FileHandle(forWritingTo: gateStdoutURL)

        let result: ProcessResult
        do {
            result = try runner.run(
                ProcessRequest(
                    executable: scriptURL.path,
                    arguments: ["\(options.boots)"],
                    environment: env,
                    workingDirectory: repoRoot,
                    combineOutput: true
                ),
                outputHandler: { data in
                    gateOutputHandle.write(data)
                    FileHandle.standardOutput.write(data)
                }
            )
        } catch {
            try? gateOutputHandle.close()
            throw error
        }
        try gateOutputHandle.close()

        let endedAt = Date()
        let endFacts = try HostFacts.sample(runner: runner, repoRoot: repoRoot, wallTime: endedAt)
        let serialURL = runDirectory.appendingPathComponent("serial.txt")
        let serialBytes = try harvestSerials(from: gateTmp, to: serialURL)
        let buildID = try extractBuildID(from: serialURL)
        let gateStdoutText = String(decoding: try Data(contentsOf: gateStdoutURL), as: UTF8.self)
        let gateStdoutBytes = fileSize(gateStdoutURL)
        let qemuLogURL = runDirectory.appendingPathComponent("qemu.log")
        FileManager.default.createFile(atPath: qemuLogURL.path, contents: nil)

        let kernel = KernelIdentity(
            buildID: buildID,
            gitSHA: startFacts.gitSHA ?? endFacts.gitSHA,
            gitDirty: startFacts.gitDirty ?? endFacts.gitDirty,
            imageSHA256: nil
        )
        // A preflight refusal (kernel not built with --features boot_tests) is a
        // run that never booted at all, distinct from a run whose boots ran and
        // failed -- see the `bootTestsPreflightRefusalMarker` doc comment above.
        let verdict: Verdict
        if result.exitCode != 0 && gateStdoutText.contains(LocalGateLauncher.bootTestsPreflightRefusalMarker) {
            verdict = .refused("kernel not built with --features boot_tests (docker/qemu/run-aarch64-boot-test-strict.sh:194-217)")
        } else {
            verdict = .gateScript(command: command, exitCode: Int(result.exitCode))
        }
        let manifest = RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: endedAt,
            arch: .aarch64,
            profile: options.profile.rawValue,
            launcher: .localQEMU,
            kernel: kernel,
            host: HostFactsTrace(start: startFacts, end: endFacts),
            verdict: verdict,
            verdictSource: .gateScript(command: command, exitCode: Int(result.exitCode)),
            serials: [SerialRef(name: "serial.txt", path: "serial.txt", bytes: serialBytes, stream: .single)],
            captures: [
                CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: gateStdoutBytes),
                CaptureRef(name: "qemu.log", path: "qemu.log", bytes: fileSize(qemuLogURL))
            ],
            command: command,
            env: env,
            tags: options.tags,
            notes: nil
        )

        if options.persist {
            try store.writeManifest(manifest)
        }

        return LocalGateLaunchResult(
            manifest: manifest,
            runDirectory: runDirectory,
            manifestURL: options.persist ? store.manifestURL(id: id) : nil,
            gateStdoutURL: gateStdoutURL,
            serialURL: serialURL,
            stored: options.persist
        )
    }

    private func prepareRunDirectory(id: String, persist: Bool) throws -> URL {
        if persist {
            return try store.createRunDirectory(id: id)
        }

        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("breenix-runs-\(id)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    // Separator lines (`==== breenix-runs boot N: <relative path> ====`) exist
    // because markers repeat within a boot and across boots (DESIGN.md Sec 1.3:
    // "Markers repeat... The Inspector stores every occurrence with its line
    // number, and exposes counts"), so a merged serial with no boundary marker
    // would make a marker's n-th occurrence globally ambiguous as to which boot
    // produced it. The relative path is kept in the label for traceability back
    // to the gate's own per-iteration output directory naming.
    private func harvestSerials(from gateTmp: URL, to outputURL: URL) throws -> Int {
        let serials = try serialFiles(under: gateTmp)
        guard !serials.isEmpty else {
            FileManager.default.createFile(atPath: outputURL.path, contents: nil)
            return 0
        }

        FileManager.default.createFile(atPath: outputURL.path, contents: nil)
        let handle = try FileHandle(forWritingTo: outputURL)
        defer { try? handle.close() }

        for (index, serial) in serials.enumerated() {
            let rel = serial.path.replacingOccurrences(of: gateTmp.path + "/", with: "")
            let separator = "==== breenix-runs boot \(index + 1): \(rel) ====\n"
            if let data = separator.data(using: .utf8) {
                handle.write(data)
            }
            handle.write(try Data(contentsOf: serial))
            if let newline = "\n".data(using: .utf8) {
                handle.write(newline)
            }
        }

        return fileSize(outputURL)
    }

    // Matches `serial*.txt` anywhere under gate-tmp, EXCEPT inside a
    // `*_failures` directory. Every aarch64 gate's own failure-preservation
    // copy (`report_failure()` / `cleanup()`) is a redundant snapshot of a boot
    // whose primary serial already lives in that boot's own per-iteration
    // OUTPUT_DIR within this SAME gate-tmp -- it exists so evidence survives
    // the gate's OWN directory reuse across separate invocations sharing one
    // fixed BREENIX_GATE_TMP, which does not apply here since every run gets
    // its own gate-tmp. Left unfiltered, the prod-profile gate's copy in
    // particular collides with this filter's name-based test: it preserves the
    // failing boot at `breenix_prod_profile_failures/<timestamp>/serial.txt`
    // (docker/qemu/run-aarch64-prod-profile-boot-test.sh:225-236) -- literally
    // named `serial.txt`, unlike the strict/testing gates' `<timestamp>-bootN.txt`
    // copies (run-aarch64-boot-test-strict.sh:493,
    // run-aarch64-testing-profile-boot-test.sh:238) which never match the
    // `serial*.txt` prefix test in the first place. Filtering on the directory
    // name rather than relying on that filename accident makes the exclusion
    // hold for all three gates uniformly.
    private func serialFiles(under directory: URL) throws -> [URL] {
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles]
        ) else {
            return []
        }

        var urls: [URL] = []
        for case let url as URL in enumerator {
            let values = try url.resourceValues(forKeys: [.isRegularFileKey])
            guard values.isRegularFile == true,
                  url.lastPathComponent.hasPrefix("serial"),
                  url.pathExtension == "txt" else {
                continue
            }
            let relativeComponents = url.deletingLastPathComponent().path
                .replacingOccurrences(of: directory.path, with: "")
                .split(separator: "/")
            if relativeComponents.contains(where: { $0.hasSuffix("_failures") }) {
                continue
            }
            urls.append(url)
        }

        return urls.sorted { lhs, rhs in
            naturalSerialKey(lhs.path) < naturalSerialKey(rhs.path)
        }
    }

    private func naturalSerialKey(_ path: String) -> String {
        var key = ""
        var digits = ""

        func flushDigits() -> String {
            guard !digits.isEmpty else {
                return ""
            }
            return String(format: "%08d", Int(digits) ?? 0)
        }

        for character in path {
            if character.isNumber {
                digits.append(character)
            } else {
                key += flushDigits()
                digits.removeAll(keepingCapacity: true)
                key.append(character)
            }
        }
        key += flushDigits()
        return key
    }

    // DESIGN.md Sec 4.2 `bootBanner.aarch64` row: pattern sourced verbatim from
    // where BUILD_ID is emitted (kernel/src/main_aarch64.rs:486-488,
    // `serial_println!("  BUILD_ID: {}", env!("BREENIX_BUILD_ID"))`) and
    // produced (kernel/build.rs:28-33, `format!("{:010x}{:04x}", ...)` -- 10 hex
    // seconds-since-epoch digits + 4 hex sub-second digits = 14 hex chars).
    private func extractBuildID(from serialURL: URL) throws -> String? {
        let text = String(decoding: try Data(contentsOf: serialURL), as: UTF8.self)
        guard let range = text.range(of: #"BUILD_ID:\s*([0-9a-fA-F]{14})"#, options: .regularExpression) else {
            return nil
        }
        let matched = String(text[range])
        return matched.split(whereSeparator: \.isWhitespace).last.map(String.init)
    }

    private func fileSize(_ url: URL) -> Int {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else {
            return 0
        }
        return size.intValue
    }
}
