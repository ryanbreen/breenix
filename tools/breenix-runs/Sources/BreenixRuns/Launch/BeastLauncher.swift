import Foundation

public enum BeastLauncherError: Error, Equatable, CustomStringConvertible {
    case unsupportedHost(String)
    case prepareCloneFailed(exitCode: Int, output: String)
    case missingLocalSHA
    case invalidBootCount(Int)
    case invalidSuiteID(String)
    case invalidLaneKey(String)

    public var description: String {
        switch self {
        case .unsupportedHost(let host):
            return "unsupported x86 host \(host); PR-5 supports only beast and does not fall back to local TCG on this Mac"
        case .prepareCloneFailed(let exitCode, let output):
            return "prepare clone failed with exit \(exitCode): \(output)"
        case .missingLocalSHA:
            return "could not resolve local git SHA; pass --sha explicitly"
        case .invalidBootCount(let boots):
            return "--boots requires a positive integer, got \(boots)"
        case .invalidLaneKey(let key):
            return "invalid lane key \(key.debugDescription)"
        case .invalidSuiteID(let id):
            return "--suite requires a suite id (lowercase words of a-z and 0-9 joined by '-'), got \(id.debugDescription)"
        }
    }
}

public enum X86Profile: String, CaseIterable, Sendable {
    case gate
}

/// Hardware selected by qemu-uefi; separate from the gate's test mode.
public enum X86HardwareProfile: String, CaseIterable, Sendable {
    case `default`, q35, e1000e, rtl8139
    case virtioNet = "virtio-net"
    case virtioModern = "virtio-modern"
    case ahci, nvme, smp4
}

public struct BeastLaunchOptions: Sendable {
    public var qemuProfile: X86HardwareProfile?
    /// An effort-suite id: the gate boots the production kernel running /sbin/suite-<id>.
    public var suite: String?
    public var boots: Int
    public var mode: RemoteGateMode
    public var sha: String
    public var gitDirty: Bool?
    public var tags: [String]
    public var persist: Bool
    public var runID: String?
    public var laneKey: String?
    public var fresh: Bool

    public init(
        boots: Int = 1,
        mode: RemoteGateMode = .full,
        sha: String,
        gitDirty: Bool? = nil,
        tags: [String] = [],
        persist: Bool = true,
        runID: String? = nil,
        qemuProfile: X86HardwareProfile? = nil,
        suite: String? = nil,
        laneKey: String? = nil,
        fresh: Bool = false
    ) {
        self.qemuProfile = qemuProfile
        self.suite = suite
        self.laneKey = laneKey
        self.fresh = fresh
        self.boots = boots
        self.mode = mode
        self.sha = sha
        self.gitDirty = gitDirty
        self.tags = tags
        self.persist = persist
        self.runID = runID
    }
}

public struct BeastLaunchResult: Sendable {
    public var manifest: RunManifest
    public var runDirectory: URL?
    public var manifestURL: URL?
    public var stored: Bool

    public init(manifest: RunManifest, runDirectory: URL?, manifestURL: URL?, stored: Bool) {
        self.manifest = manifest
        self.runDirectory = runDirectory
        self.manifestURL = manifestURL
        self.stored = stored
    }
}

public struct BeastLauncher {
    public var store: RunStore
    public var runner: ProcessRunner
    public var timeoutSecs: Int
    public var fullBackstopSecs: Int
    public var pathsTemplate: BeastPaths
    public var slotHelperBase64: String?
    public var treeHelperBase64: String?

    public init(
        store: RunStore,
        runner: ProcessRunner = RealProcessRunner(),
        timeoutSecs: Int = 900,
        pathsTemplate: BeastPaths = BeastPaths(clonePath: ""),
        slotHelperBase64: String? = nil,
        treeHelperBase64: String? = nil
    ) {
        self.store = store
        self.runner = runner
        self.timeoutSecs = timeoutSecs
        self.fullBackstopSecs = Int(ProcessInfo.processInfo.environment["BREENIX_FULL_BACKSTOP"] ?? "")
            .flatMap { $0 > 0 ? $0 : nil } ?? max(1800, timeoutSecs)
        self.pathsTemplate = pathsTemplate
        self.slotHelperBase64 = slotHelperBase64
        self.treeHelperBase64 = treeHelperBase64
    }

    public static func localGitIdentity(repoRoot: URL, runner: ProcessRunner) throws -> (sha: String?, dirty: Bool?) {
        try HostFacts.gitIdentity(repoRoot: repoRoot, runner: runner)
    }

    /// Builds the exact plan a real `runX86(options:)` call would execute,
    /// with NO side effects - this is what `--dry-run` prints, and `runX86`
    /// calls this same function internally so the two can never drift apart.
    public func plan(options: BeastLaunchOptions) throws -> RemoteCommand.Plan {
        try validate(options: options)
        let id = options.runID ?? RunManifest.makeID(startedAt: Date(), arch: .x86_64, profile: "gate")
        var paths = paths(forRunID: id)
        paths.laneKey = options.laneKey
        paths.requestedSHA = options.sha
        paths.fresh = options.fresh
        return RemoteCommand.plan(
            sha: options.sha,
            boots: options.boots,
            mode: options.mode,
            timeoutSecs: timeoutSecs,
            paths: paths,
            qemuProfile: options.qemuProfile,
            suite: options.suite,
            fullBackstopSecs: fullBackstopSecs,
            slotHelperBase64: slotHelperBase64,
            treeHelperBase64: treeHelperBase64
        )
    }

    public func runX86(options: BeastLaunchOptions) throws -> BeastLaunchResult {
        let startedAt = Date()
        let id = options.runID ?? RunManifest.makeID(startedAt: startedAt, arch: .x86_64, profile: "gate")
        var plannedOptions = options
        plannedOptions.runID = id
        let planResult = try plan(options: plannedOptions)

        let startFacts = parseHostFactsSample(runner: runner, paths: planResult.paths, wallTime: startedAt)
        defer {
            _ = try? runner.run(planResult.removeClone)
        }

        let prepareResult: ProcessResult
        do {
            prepareResult = try runner.run(planResult.prepareClone)
        } catch {
            throw BeastLauncherError.prepareCloneFailed(exitCode: -1, output: "\(error)")
        }
        if prepareResult.exitCode != 0 {
            throw BeastLauncherError.prepareCloneFailed(
                exitCode: Int(prepareResult.exitCode),
                output: prepareResult.stdoutString + prepareResult.stderrString
            )
        }

        let runDirectory = try prepareRunDirectory(id: id, persist: options.persist)
        defer {
            if !options.persist {
                try? FileManager.default.removeItem(at: runDirectory)
            }
        }

        let gateStdoutURL = runDirectory.appendingPathComponent("gate-stdout.txt")
        FileManager.default.createFile(atPath: gateStdoutURL.path, contents: nil)
        let gateOutputHandle = try FileHandle(forWritingTo: gateStdoutURL)
        let gateResult: ProcessResult
        do {
            gateResult = try runner.run(
                planResult.runGate,
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
        let endFacts = parseHostFactsSample(runner: runner, paths: planResult.paths, wallTime: endedAt)
        let pullResult = (try? runner.run(planResult.pullEvidence)) ?? ProcessResult(exitCode: 127)
        let serialRefs = try harvestSerials(pullResult: pullResult, runDirectory: runDirectory)
        let gateStdoutBytes = fileSize(gateStdoutURL)
        let command = readableGateCommand(paths: planResult.paths, boots: options.boots, mode: options.mode)
        let env = gateEnvironment(paths: planResult.paths, timeoutSecs: timeoutSecs, qemuProfile: options.qemuProfile, suite: options.suite)
        var captures = [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: gateStdoutBytes)]
        for screen in screenNames(in: runDirectory) {
            let url = runDirectory.appendingPathComponent(screen)
            captures.append(CaptureRef(name: screen, path: screen, bytes: fileSize(url)))
        }

        let gateStdoutText = String(decoding: try Data(contentsOf: gateStdoutURL), as: UTF8.self)
        let gateVerdictString: String
        if gateStdoutText.contains("PASS-WITH-ATTRIBUTED-LOCKUP:") {
            gateVerdictString = "PASS-WITH-ATTRIBUTED-LOCKUP"
        } else if gateStdoutText.split(separator: "\n").contains(where: {
            $0.trimmingCharacters(in: .whitespaces).hasPrefix("FAIL:")
        }) || gateResult.exitCode != 0 {
            gateVerdictString = "FAIL"
        } else {
            gateVerdictString = "PASS"
        }
        let verdict = Verdict.projectGateVerdict(gateVerdictString, exitCode: Int(gateResult.exitCode), command: command)

        let manifest = RunManifest(
            id: id,
            startedAt: startedAt,
            endedAt: endedAt,
            arch: .x86_64,
            profile: "gate",
            launcher: .beastSSH,
            kernel: KernelIdentity(buildID: nil, gitSHA: options.sha, gitDirty: options.gitDirty, imageSHA256: nil),
            // The shared HostFactsSample fields record beast's Linux CPU model,
            // total RAM, and QEMU version here rather than this Mac's sysctl values.
            host: startFacts.flatMap { start in endFacts.map { HostFactsTrace(start: start, end: $0) } },
            verdict: verdict,
            verdictSource: .gateScript(command: command, exitCode: Int(gateResult.exitCode)),
            serials: serialRefs,
            captures: captures,
            command: command,
            env: env,
            tags: options.tags,
            notes: nil
        )

        if options.persist {
            try store.writeManifest(manifest)
        }

        return BeastLaunchResult(
            manifest: manifest,
            runDirectory: runDirectory,
            manifestURL: options.persist ? store.manifestURL(id: id) : nil,
            stored: options.persist
        )
    }

    private func validate(options: BeastLaunchOptions) throws {
        guard options.boots > 0 else {
            throw BeastLauncherError.invalidBootCount(options.boots)
        }
        guard pathsTemplate.host == "beast" else {
            throw BeastLauncherError.unsupportedHost(pathsTemplate.host)
        }
        guard !options.sha.isEmpty else {
            throw BeastLauncherError.missingLocalSHA
        }
        if let lane = options.laneKey,
           lane.count != 64 || !lane.unicodeScalars.allSatisfy({ ("a"..."f").contains($0) || ("0"..."9").contains($0) }) {
            throw BeastLauncherError.invalidLaneKey(lane)
        }
        if let suite = options.suite, !RemoteCommand.isSuiteList(suite) {
            throw BeastLauncherError.invalidSuiteID(suite)
        }
    }

    private func paths(forRunID id: String) -> BeastPaths {
        var paths = pathsTemplate
        if paths.clonePath.isEmpty {
            let parent = URL(fileURLWithPath: paths.canonicalRepoDir)
                .deletingLastPathComponent()
                .path
            paths.clonePath = "\(parent)/breenix-\(id)"
        }
        return paths
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

    private func parseHostFactsSample(runner: ProcessRunner, paths: BeastPaths, wallTime: Date) -> HostFactsSample? {
        guard let result = try? runner.run(RemoteCommand.hostFactsRequest(paths: paths)),
              result.exitCode == 0 else {
            return nil
        }
        return RemoteCommand.parseHostFacts(result.stdoutString, wallTime: wallTime)
    }

    private func harvestSerials(pullResult: ProcessResult, runDirectory: URL) throws -> [SerialRef] {
        let userURL = runDirectory.appendingPathComponent("serial_user.txt")
        let kernelURL = runDirectory.appendingPathComponent("serial_kernel.txt")
        FileManager.default.createFile(atPath: userURL.path, contents: nil)
        FileManager.default.createFile(atPath: kernelURL.path, contents: nil)

        let tarballURL = runDirectory.appendingPathComponent("gate-tmp.tar.gz")
        let gateTmpURL = runDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        defer {
            try? FileManager.default.removeItem(at: gateTmpURL)
            try? FileManager.default.removeItem(at: tarballURL)
        }

        if pullResult.exitCode == 0 && !pullResult.stdout.isEmpty {
            try pullResult.stdout.write(to: tarballURL)
            _ = try? runner.run(ProcessRequest(
                executable: "/usr/bin/tar",
                arguments: ["-xzf", tarballURL.path, "-C", runDirectory.path]
            ))
            try mergeSerials(from: gateTmpURL, userURL: userURL, kernelURL: kernelURL)
            keepScreens(from: gateTmpURL, runDirectory: runDirectory)
        }

        return [
            SerialRef(name: "serial_user.txt", path: "serial_user.txt", bytes: fileSize(userURL), stream: .com1),
            SerialRef(name: "serial_kernel.txt", path: "serial_kernel.txt", bytes: fileSize(kernelURL), stream: .com2)
        ]
    }

    /// A suite gate saves each boot's final screen as breenix_gate_<n>/screen.png.
    /// Each is kept as screen-<n>.png, so a screen is always the boot it came from and
    /// a boot that saved none has none.
    private func keepScreens(from gateTmp: URL, runDirectory: URL) {
        let iterations = (try? FileManager.default.contentsOfDirectory(atPath: gateTmp.path)) ?? []
        for iteration in iterations where iteration.hasPrefix("breenix_gate_") {
            let boot = iteration.dropFirst("breenix_gate_".count)
            guard !boot.isEmpty, boot.allSatisfy(\.isNumber) else { continue }
            let screen = gateTmp.appendingPathComponent(iteration).appendingPathComponent("screen.png")
            guard FileManager.default.fileExists(atPath: screen.path) else { continue }
            let destination = runDirectory.appendingPathComponent("screen-\(boot).png")
            try? FileManager.default.removeItem(at: destination)
            try? FileManager.default.copyItem(at: screen, to: destination)
        }
    }

    /// The per-boot screens `keepScreens` kept, in boot order.
    private func screenNames(in runDirectory: URL) -> [String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: runDirectory.path)) ?? []
        return names.filter { $0.hasPrefix("screen-") && $0.hasSuffix(".png") }
            .sorted { naturalSerialKey($0) < naturalSerialKey($1) }
    }

    private func mergeSerials(from gateTmp: URL, userURL: URL, kernelURL: URL) throws {
        guard let iterationDirectories = try? FileManager.default.contentsOfDirectory(
            at: gateTmp,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else {
            return
        }

        let sortedIterations = try iterationDirectories.filter { url in
            let values = try url.resourceValues(forKeys: [.isDirectoryKey])
            return values.isDirectory == true && url.lastPathComponent.hasPrefix("breenix_gate_")
        }.sorted {
            naturalSerialKey($0.path) < naturalSerialKey($1.path)
        }

        let userHandle = try FileHandle(forWritingTo: userURL)
        defer { try? userHandle.close() }
        let kernelHandle = try FileHandle(forWritingTo: kernelURL)
        defer { try? kernelHandle.close() }

        for (index, directory) in sortedIterations.enumerated() {
            try appendSerialIfPresent(
                directory.appendingPathComponent("serial_user.log"),
                gateTmp: gateTmp,
                boot: index + 1,
                to: userHandle
            )
            try appendSerialIfPresent(
                directory.appendingPathComponent("serial_kernel.log"),
                gateTmp: gateTmp,
                boot: index + 1,
                to: kernelHandle
            )
        }
    }

    private func appendSerialIfPresent(_ serial: URL, gateTmp: URL, boot: Int, to handle: FileHandle) throws {
        guard FileManager.default.fileExists(atPath: serial.path) else {
            return
        }
        let rel = relativePath(of: serial, under: gateTmp)
        let separator = "==== breenix-runs boot \(boot): \(rel) ====\n"
        if let data = separator.data(using: .utf8) {
            handle.write(data)
        }
        handle.write(try Data(contentsOf: serial))
        if let newline = "\n".data(using: .utf8) {
            handle.write(newline)
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

    private func relativePath(of url: URL, under root: URL) -> String {
        let rootPath = root.standardizedFileURL.path
        let path = url.standardizedFileURL.path
        guard path.hasPrefix(rootPath + "/") else {
            return url.lastPathComponent
        }
        return String(path.dropFirst(rootPath.count + 1))
    }

    private func readableGateCommand(paths: BeastPaths, boots: Int, mode: RemoteGateMode) -> [String] {
        if let lane = paths.laneKey, let sha = paths.requestedSHA {
            return ["python3", paths.gateTmpPath + "/gate-tree.py", paths.canonicalRepoDir,
                    lane, sha, paths.gateTmpPath, "\(boots)", mode.rawValue]
        }
        return ["\(paths.clonePath)/docker/qemu/run-x86-gate.sh", "\(boots)", mode.rawValue]
    }

    private func gateEnvironment(paths: BeastPaths, timeoutSecs: Int, qemuProfile: X86HardwareProfile?, suite: String?) -> [String: String] {
        var environment = [
            "BREENIX_GATE_TMP": paths.gateTmpPath,
            "BREENIX_REPO_DIR": paths.clonePath,
            "BREENIX_RUST_FORK": paths.rustForkPath,
            "BREENIX_GATE_TIMEOUT": "\(timeoutSecs)",
            "BREENIX_FULL_BACKSTOP": "\(fullBackstopSecs)"
        ]
        environment["BREENIX_QEMU_PROFILE"] = (qemuProfile ?? .default).rawValue
        if let suite {
            environment["BREENIX_BOOT_SUITE"] = suite
            environment["BREENIX_QMP_SOCKET"] = paths.gateTmpPath + "/qmp.sock"
        }
        return environment
    }

    private func fileSize(_ url: URL) -> Int {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else {
            return 0
        }
        return size.intValue
    }
}
