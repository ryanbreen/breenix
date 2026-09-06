import Foundation

public enum BeastLauncherError: Error, Equatable, CustomStringConvertible {
    case unsupportedHost(String)
    case prepareCloneFailed(exitCode: Int, output: String)
    case missingLocalSHA
    case invalidBootCount(Int)

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
        }
    }
}

public enum X86Profile: String, CaseIterable, Sendable {
    case gate
}

public struct BeastLaunchOptions: Sendable {
    public var boots: Int
    public var mode: RemoteGateMode
    public var sha: String
    public var gitDirty: Bool?
    public var tags: [String]
    public var persist: Bool
    public var runID: String?

    public init(
        boots: Int = 1,
        mode: RemoteGateMode = .full,
        sha: String,
        gitDirty: Bool? = nil,
        tags: [String] = [],
        persist: Bool = true,
        runID: String? = nil
    ) {
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
    public var pathsTemplate: BeastPaths

    public init(
        store: RunStore,
        runner: ProcessRunner = RealProcessRunner(),
        timeoutSecs: Int = 900,
        pathsTemplate: BeastPaths = BeastPaths(clonePath: "")
    ) {
        self.store = store
        self.runner = runner
        self.timeoutSecs = timeoutSecs
        self.pathsTemplate = pathsTemplate
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
        return RemoteCommand.plan(
            sha: options.sha,
            boots: options.boots,
            mode: options.mode,
            timeoutSecs: timeoutSecs,
            paths: paths(forRunID: id)
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
        let env = gateEnvironment(paths: planResult.paths, timeoutSecs: timeoutSecs)

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
            verdict: .gateScript(command: command, exitCode: Int(gateResult.exitCode)),
            verdictSource: .gateScript(command: command, exitCode: Int(gateResult.exitCode)),
            serials: serialRefs,
            captures: [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: gateStdoutBytes)],
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
        }

        return [
            SerialRef(name: "serial_user.txt", path: "serial_user.txt", bytes: fileSize(userURL), stream: .com1),
            SerialRef(name: "serial_kernel.txt", path: "serial_kernel.txt", bytes: fileSize(kernelURL), stream: .com2)
        ]
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
        ["\(paths.clonePath)/docker/qemu/run-x86-gate.sh", "\(boots)", mode.rawValue]
    }

    private func gateEnvironment(paths: BeastPaths, timeoutSecs: Int) -> [String: String] {
        [
            "BREENIX_GATE_TMP": paths.gateTmpPath,
            "BREENIX_REPO_DIR": paths.clonePath,
            "BREENIX_RUST_FORK": paths.rustForkPath,
            "BREENIX_GATE_TIMEOUT": "\(timeoutSecs)"
        ]
    }

    private func fileSize(_ url: URL) -> Int {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else {
            return 0
        }
        return size.intValue
    }
}
