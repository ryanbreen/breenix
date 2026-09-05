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

    public init(store: RunStore, repoRoot: URL, runner: ProcessRunner = RealProcessRunner(), hostLock: HostLock = FallbackQEMUHostLock()) {
        self.store = store
        self.repoRoot = repoRoot
        self.runner = runner
        self.hostLock = hostLock
    }

    public func runArm(options: LocalGateLaunchOptions) throws -> LocalGateLaunchResult {
        let startedAt = Date()
        let id = RunManifest.makeID(startedAt: startedAt, arch: .aarch64, profile: options.profile.rawValue)
        let runDirectory = try prepareRunDirectory(id: id, persist: options.persist)
        let gateTmp = runDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        try FileManager.default.createDirectory(at: gateTmp, withIntermediateDirectories: true)

        try hostLock.acquire(runner: runner)

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
        let gateStdoutBytes = fileSize(gateStdoutURL)
        let qemuLogURL = runDirectory.appendingPathComponent("qemu.log")
        FileManager.default.createFile(atPath: qemuLogURL.path, contents: nil)

        let kernel = KernelIdentity(
            buildID: buildID,
            gitSHA: startFacts.gitSHA ?? endFacts.gitSHA,
            gitDirty: startFacts.gitDirty ?? endFacts.gitDirty,
            imageSHA256: nil
        )
        let verdict = Verdict.gateScript(command: command, exitCode: Int(result.exitCode))
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
            if values.isRegularFile == true && url.lastPathComponent.hasPrefix("serial") && url.pathExtension == "txt" {
                urls.append(url)
            }
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
