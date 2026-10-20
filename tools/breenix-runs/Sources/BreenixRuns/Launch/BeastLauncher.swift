import Foundation

public enum BeastLauncherError: Error, Equatable, CustomStringConvertible {
    case unsupportedHost(String)
    case prepareCloneFailed(exitCode: Int, output: String)
    case missingLocalSHA
    case invalidBootCount(Int)
    case invalidSuiteID(String)
    case invalidLaneKey(String)
    case targetPolicyRefused(String)

    public var description: String {
        switch self {
        case .unsupportedHost(let host):
            return "unsupported x86 host \(host); supported host is beast"
        case .prepareCloneFailed(let exitCode, let output):
            return "prepare clone failed with exit \(exitCode): \(output)"
        case .missingLocalSHA:
            return "could not resolve local git SHA; pass --sha explicitly"
        case .invalidBootCount(let boots):
            return "--boots requires a positive integer, got \(boots)"
        case .invalidLaneKey(let key):
            return "invalid lane key \(key.debugDescription)"
        case .targetPolicyRefused(let output):
            return "effort target policy refused this launch: \(output)"
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
    case ahci, nvme
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
    public var catchup: Bool
    /// The commit scripts/effort-target-guard.py accepted; beast's nested launchers skip re-checking it.
    public var targetPolicySHA: String?

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
        fresh: Bool = false,
        catchup: Bool = false
    ) {
        self.qemuProfile = qemuProfile
        self.suite = suite
        self.laneKey = laneKey
        self.fresh = fresh
        self.catchup = catchup
        self.boots = boots
        self.mode = mode
        self.sha = sha
        self.gitDirty = gitDirty
        self.tags = tags
        self.persist = persist
        self.runID = runID
    }
}

public struct BeastLauncher {
    public var store: RunStore
    public var runner: ProcessRunner
    public var timeoutSecs: Int
    public var fullBackstopSecs: Int
    public var pathsTemplate: BeastPaths
    public var slotHelperBase64: String?
    public var progressBundleBase64: String?
    /// scripts/effort-target-guard.py; nil only where a caller deliberately runs without the policy.
    public var targetGuard: URL?
    public var treeHelperBase64: String?
    public var quietOverride: String?
    public let x86QemuEnvironment: [String: String]
    var reconnectLimit = 60
    var reconnectDelay: ((Int) -> Void)? = nil

    public init(
        store: RunStore,
        runner: ProcessRunner = RealProcessRunner(),
        timeoutSecs: Int = 900,
        pathsTemplate: BeastPaths = BeastPaths(clonePath: ""),
        slotHelperBase64: String? = nil,
        progressBundleBase64: String? = nil,
        treeHelperBase64: String? = nil,
        quietOverride: String? = ProcessInfo.processInfo.environment["BREENIX_QUIET"],
        targetGuard: URL?,
        x86QemuEnvironment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.store = store
        self.runner = runner
        self.timeoutSecs = timeoutSecs
        self.fullBackstopSecs = Int(ProcessInfo.processInfo.environment["BREENIX_FULL_BACKSTOP"] ?? "")
            .flatMap { $0 > 0 ? $0 : nil } ?? max(1800, timeoutSecs)
        self.pathsTemplate = pathsTemplate
        self.slotHelperBase64 = slotHelperBase64
        self.progressBundleBase64 = progressBundleBase64
        self.treeHelperBase64 = treeHelperBase64
        self.x86QemuEnvironment = x86QemuEnvironment
        self.targetGuard = targetGuard
        self.quietOverride = quietOverride
    }

    public static func localGitIdentity(repoRoot: URL, runner: ProcessRunner) throws -> (sha: String?, dirty: Bool?) {
        try HostFacts.gitIdentity(repoRoot: repoRoot, runner: runner)
    }

    public static func progressBundle(repoRoot: URL) throws -> String {
        let paths = ["scripts/boot_progress.py", "docs/boot-path.json",
                     "tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json",
                     "tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json"]
        var files = try Dictionary(uniqueKeysWithValues: paths.map { path in
            var data = try Data(contentsOf: repoRoot.appendingPathComponent(path))
            if path.hasSuffix(".json") {
                guard let source = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                    throw CocoaError(.fileReadCorruptFile)
                }
                let compact: [String: Any]
                if let stages = source["stages"] as? [[String: Any]] {
                    compact = ["stages": try stages.map { stage in
                        guard let marker = stage["marker"] as? String else { throw CocoaError(.fileReadCorruptFile) }
                        return ["marker": marker]
                    }]
                } else if let milestones = source["milestones"] as? [[String: Any]] {
                    compact = ["milestones": try milestones.map { milestone in
                        guard let stages = milestone["stages"] as? [String: [Any]] else { throw CocoaError(.fileReadCorruptFile) }
                        return ["stages": stages.mapValues { $0.compactMap { ($0 as? [String: Any])?["marker"] as? String }.map { ["marker": $0] } }]
                    }]
                } else {
                    throw CocoaError(.fileReadCorruptFile)
                }
                // Keep only the classifier's marker inputs so the SSH command
                // stays below Linux's per-argument limit.
                data = try JSONSerialization.data(withJSONObject: compact, options: [.sortedKeys])
            }
            return (path, data.base64EncodedString())
        })
        let programRoot = repoRoot.appendingPathComponent("userspace/programs/src")
        let enumerator = FileManager.default.enumerator(at: programRoot, includingPropertiesForKeys: nil)
        let tagExpression = try NSRegularExpression(pattern: #""\[([A-Za-z][A-Za-z0-9_-]*)\]"#)
        var tags: Set<String> = ["parent", "child"]
        while let source = enumerator?.nextObject() as? URL {
            guard source.pathExtension == "rs" else { continue }
            tags.insert(source.deletingPathExtension().lastPathComponent.lowercased())
            let text = try String(contentsOf: source, encoding: .utf8)
            for match in tagExpression.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
                if let range = Range(match.range(at: 1), in: text) { tags.insert(String(text[range]).lowercased()) }
            }
        }
        files["docs/boot-progress-userspace.json"] = try JSONSerialization.data(withJSONObject: tags.sorted()).base64EncodedString()
        return try JSONSerialization.data(withJSONObject: files, options: [.sortedKeys]).base64EncodedString()
    }

    /// Describes the retired remote gate for an explicitly requested dry run.
    public func plan(options: BeastLaunchOptions) throws -> RemoteCommand.Plan {
        try validate(options: options)
        let id = options.runID ?? RunManifest.makeID(startedAt: Date(), arch: .x86_64, profile: "gate")
        var paths = paths(forRunID: id)
        paths.laneKey = options.laneKey
        paths.requestedSHA = options.sha
        paths.fresh = options.fresh
        paths.targetPolicyStamp = options.targetPolicySHA.map { "x86:" + $0 }
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
            progressBundleBase64: progressBundleBase64,
            treeHelperBase64: treeHelperBase64,
            quietOverride: quietOverride,
            allowX86Qemu: x86QemuEnvironment["BREENIX_ALLOW_X86_QEMU"] == "1"
        )
    }

    /// Runs the effort target policy for `options.sha`, returning the full SHA it accepted
    /// (nil without a guard); a refusal throws before anything reaches beast.
    public func checkTargetPolicy(options: BeastLaunchOptions) throws -> String? {
        try validate(options: options)
        guard let targetGuard else { return nil }
        let repo = targetGuard.deletingLastPathComponent().deletingLastPathComponent()
        let result = try runner.run(ProcessRequest(executable: "/usr/bin/env",
            arguments: ["python3", targetGuard.path, "--repo", repo.path, "--commit", options.sha, "--target", "x86"]
                + (options.catchup ? ["--catchup"] : [])))
        guard result.exitCode == 0 else {
            throw BeastLauncherError.targetPolicyRefused(result.stderrString.trimmingCharacters(in: .whitespacesAndNewlines))
        }
        return result.stdoutString.trimmingCharacters(in: .whitespacesAndNewlines)
    }


    /// Cancel and harvest a run whose observer exhausted its recovery budget.
    /// Retain its failure verdict: later recovery is evidence, not a new boot.
    public func recoverX86(id: String) throws {
        var manifest = try store.readManifest(id: id)
        guard manifest.arch == .x86_64,
              id.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.contains($0) || $0 == "-" }) else {
            throw BeastLauncherError.invalidLaneKey(id)
        }
        var paths = paths(forRunID: id)
        paths.laneKey = manifest.env["BREENIX_LANE_KEY"]
        func run(_ request: ProcessRequest) throws -> ProcessResult {
            var result = ProcessResult(exitCode: 255)
            for attempt in 0...reconnectLimit {
                result = (try? runner.run(request)) ?? ProcessResult(exitCode: 255)
                if result.exitCode == 0 { return result }
                if attempt < reconnectLimit {
                    if let reconnectDelay { reconnectDelay(attempt + 1) }
                    else { Thread.sleep(forTimeInterval: Double(min(attempt + 1, 10))) }
                }
            }
            throw BeastLauncherError.prepareCloneFailed(exitCode: Int(result.exitCode), output: result.stderrString)
        }
        _ = try run(RemoteCommand.stopGateRequest(paths: paths))
        let pull = try run(RemoteCommand.pullEvidenceRequest(paths: paths))
        guard !pull.stdout.isEmpty else { throw CocoaError(.fileReadCorruptFile) }
        manifest.serials = try harvestSerials(pullResult: pull, runDirectory: store.runDirectory(id: id))
        for index in manifest.captures.indices {
            manifest.captures[index].bytes = fileSize(store.runDirectory(id: id).appendingPathComponent(manifest.captures[index].path))
        }
        manifest.env["BREENIX_REMOTE_QUIESCED"] = "1"
        manifest.notes = "Remote cancellation and evidence recovery completed. Original verdict retained."
        try store.writeManifest(manifest)
        _ = try run(RemoteCommand.removeCloneRequest(paths: paths))
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



    private func harvestSerials(pullResult: ProcessResult, runDirectory: URL) throws -> [SerialRef] {
        let userURL = runDirectory.appendingPathComponent("serial_user.txt")
        let kernelURL = runDirectory.appendingPathComponent("serial_kernel.txt")

        let tarballURL = runDirectory.appendingPathComponent("gate-tmp.tar.gz")
        let gateTmpURL = runDirectory.appendingPathComponent("gate-tmp", isDirectory: true)
        defer {
            try? FileManager.default.removeItem(at: gateTmpURL)
            try? FileManager.default.removeItem(at: tarballURL)
        }

        if pullResult.exitCode == 0 && !pullResult.stdout.isEmpty {
            try pullResult.stdout.write(to: tarballURL)
            let extraction = try runner.run(ProcessRequest(
                executable: "/usr/bin/tar",
                arguments: ["-xzf", tarballURL.path, "-C", runDirectory.path]
            ))
            guard extraction.exitCode == 0 else {
                throw NSError(domain: "x86 evidence extraction", code: Int(extraction.exitCode),
                              userInfo: [NSLocalizedDescriptionKey: extraction.stderrString])
            }
            // Preserve live partials on failed harvest; a valid archive replaces them.
            try Data().write(to: userURL)
            try Data().write(to: kernelURL)
            let output = gateTmpURL.appendingPathComponent("gate-output.log")
            if FileManager.default.fileExists(atPath: output.path) {
                try Data(contentsOf: output).write(to: runDirectory.appendingPathComponent("gate-stdout.txt"))
            }
            try mergeSerials(from: gateTmpURL, userURL: userURL, kernelURL: kernelURL)
            keepScreens(from: gateTmpURL, runDirectory: runDirectory)
            let boots = try FileManager.default.contentsOfDirectory(at: gateTmpURL, includingPropertiesForKeys: nil)
            for boot in boots where boot.lastPathComponent.hasPrefix("breenix_gate_") {
                let destination = runDirectory.appendingPathComponent(boot.lastPathComponent)
                if FileManager.default.fileExists(atPath: destination.path) { try FileManager.default.removeItem(at: destination) }
                try FileManager.default.copyItem(at: boot, to: destination)
            }
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
            let bootDirectory = gateTmp.appendingPathComponent(iteration)
            let suites = (try? FileManager.default.contentsOfDirectory(atPath: bootDirectory.path)) ?? []
            for suite in suites where suite.hasPrefix("suite-") {
                let source = bootDirectory.appendingPathComponent(suite).appendingPathComponent("screen.png")
                if FileManager.default.fileExists(atPath: source.path) {
                    let destination = runDirectory.appendingPathComponent("screen-\(boot)-\(suite.dropFirst(6)).png")
                    try? FileManager.default.copyItem(at: source, to: destination)
                }
            }
            let screen = gateTmp.appendingPathComponent(iteration).appendingPathComponent("screen.png")
            guard FileManager.default.fileExists(atPath: screen.path) else { continue }
            let destination = runDirectory.appendingPathComponent("screen-\(boot).png")
            try? FileManager.default.removeItem(at: destination)
            try? FileManager.default.copyItem(at: screen, to: destination)
        }
    }

    /// The per-boot screens `keepScreens` kept, in boot order.

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



    private func fileSize(_ url: URL) -> Int {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else {
            return 0
        }
        return size.intValue
    }
}
