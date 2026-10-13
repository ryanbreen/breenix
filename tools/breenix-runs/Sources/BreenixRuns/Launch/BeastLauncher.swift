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
            return "unsupported x86 host \(host); supported host is beast"
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
    public var progressBundleBase64: String?
    public var vigilScript: URL?
    public var treeHelperBase64: String?
    var makeSignals: (ProcessRunner) -> X86LaunchSignals = { X86LaunchSignals(runner: $0) }
    public var quietOverride: String?
    public var serialStreaming: X86SerialStreaming
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
        vigilScript: URL? = nil,
        serialStreaming: X86SerialStreaming = SSHSerialStreaming(),
        quietOverride: String? = ProcessInfo.processInfo.environment["BREENIX_QUIET"]
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
        self.vigilScript = vigilScript
        self.serialStreaming = serialStreaming
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
            progressBundleBase64: progressBundleBase64,
            treeHelperBase64: treeHelperBase64,
            quietOverride: quietOverride
        )
    }

    public func runX86(options: BeastLaunchOptions) throws -> BeastLaunchResult {
        let startedAt = Date()
        let id = options.runID ?? RunManifest.makeID(startedAt: startedAt, arch: .x86_64, profile: "gate")
        var plannedOptions = options
        plannedOptions.runID = id
        let planResult = try plan(options: plannedOptions)

        let startFacts = parseHostFactsSample(runner: runner, paths: planResult.paths, wallTime: startedAt)

        let prepareResult: ProcessResult
        do {
            prepareResult = try runner.run(planResult.prepareClone)
        } catch {
            throw BeastLauncherError.prepareCloneFailed(exitCode: -1, output: "\(error)")
        }
        if prepareResult.exitCode != 0 {
            _ = try? runner.run(planResult.removeClone)
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

        let signals = makeSignals(runner)
        defer { withExtendedLifetime(signals) {} }
        let live = try LiveX86Serials(directory: runDirectory, id: id, options: options, script: vigilScript, runner: runner)
        live.start()
        var stream = try? serialStreaming.start(RemoteCommand.streamSerialsRequest(paths: planResult.paths, boots: options.boots)) { data in
            live.receive(data)
        }
        var finishStatus: Int32 = 1
        defer { stream?.stop(); live.finish(status: finishStatus) }

        let gateStdoutURL = runDirectory.appendingPathComponent("gate-stdout.txt")
        FileManager.default.createFile(atPath: gateStdoutURL.path, contents: nil)
        let gateOutputHandle = try FileHandle(forWritingTo: gateStdoutURL)
        let reconnectURL = runDirectory.appendingPathComponent("reconnects.jsonl")
        FileManager.default.createFile(atPath: reconnectURL.path, contents: nil)
        let reconnectHandle = try FileHandle(forWritingTo: reconnectURL)
        defer { try? reconnectHandle.close() }
        var reconnects = 0
        func reconnect(_ operation: String, _ status: Int32, recovering: Bool = false) -> Bool {
            guard (recovering || signals.signalNumber == nil), reconnects < reconnectLimit else { return false }
            reconnects += 1
            let record: [String: Any] = ["time": ISO8601DateFormatter().string(from: Date()), "operation": operation,
                                       "attempt": reconnects, "exit_code": status]
            if let data = try? JSONSerialization.data(withJSONObject: record, options: [.sortedKeys]) {
                reconnectHandle.write(data + Data([10]))
            }
            FileHandle.standardError.write(Data("[reconnect] \(operation) attempt=\(reconnects)/\(reconnectLimit) ssh_exit=\(status); following the same remote run\n".utf8))
            if let reconnectDelay { reconnectDelay(reconnects) }
            else {
                let deadline = Date().addingTimeInterval(Double(min(reconnects, 10)))
                while Date() < deadline && (recovering || signals.signalNumber == nil) {
                    Thread.sleep(forTimeInterval: 0.05)
                }
            }
            return recovering || signals.signalNumber == nil
        }
        func transportLost(_ result: ProcessResult) -> Bool {
            [Int32(255), 9, 15, 124, 137, 143].contains(result.exitCode)
        }
        var gateResult = ProcessResult(exitCode: 1)
        var gateError: Error?
        var completed = false
        let startRequest = RemoteCommand.startGateRequest(gate: planResult.runGate, paths: planResult.paths)
        // Retry an uncertain acknowledgement against the same exclusive remote claim.
        while signals.signalNumber == nil {
            do {
                gateResult = try runner.run(startRequest)
                if gateResult.exitCode == 0 || gateResult.exitCode == 2 { break }
                FileHandle.standardError.write(Data("Launch acknowledgement failed: exit \(gateResult.exitCode) \(gateResult.stderrString)\n".utf8))
            } catch { gateResult = ProcessResult(exitCode: 255) }
            if !reconnect("launch", gateResult.exitCode) { break }
        }
        if gateResult.exitCode == 0 && signals.signalNumber == nil {
            reconnects = 0
            while signals.signalNumber == nil {
                if stream == nil || stream?.isReading == false {
                    stream?.stop()
                    live.resetConnection()
                    FileHandle.standardError.write(Data("[serial] reopening live view; gate verdict remains authoritative\n".utf8))
                    stream = try? serialStreaming.start(RemoteCommand.streamSerialsRequest(paths: planResult.paths, boots: options.boots, offsets: live.resumeOffsets)) { live.receive($0) }
                }
                do {
                    gateResult = try runner.run(
                        RemoteCommand.followGateRequest(paths: planResult.paths, offset: UInt64(fileSize(gateStdoutURL))),
                        outputHandler: { data in
                            gateOutputHandle.write(data)
                            FileHandle.standardOutput.write(data)
                        })
                } catch { gateResult = ProcessResult(exitCode: 255) }
                if !gateResult.stdout.isEmpty { reconnects = 0 }
                if gateResult.exitCode == 0,
                   let statusLine = gateResult.stderrString.split(separator: "\n").first(where: { $0.hasPrefix("BREENIX_GATE_FINISHED=") }),
                   let status = Int32(statusLine.dropFirst("BREENIX_GATE_FINISHED=".count)) {
                    gateResult.exitCode = status
                    completed = true
                    break
                }
                if gateResult.exitCode == 75 { reconnects = 0; continue }
                if transportLost(gateResult) {
                    if !reconnect("gate", gateResult.exitCode) { break }
                    continue
                }
                break
            }
        }
        stream?.stop()
        finishStatus = signals.signalNumber.map { 128 + $0 } ?? (completed || [2, 76].contains(gateResult.exitCode) ? gateResult.exitCode : 255)
        try gateOutputHandle.close()
        // Any abandoned observer must attempt cancellation before harvesting.
        // Recovery ignores the already-latched signal and has its own retry budget.
        reconnects = 0
        var quiesced = completed && signals.signalNumber == nil
        if !quiesced {
            var stop = ProcessResult(exitCode: 255)
            repeat {
                stop = (try? runner.run(RemoteCommand.stopGateRequest(paths: planResult.paths))) ?? ProcessResult(exitCode: 255)
            } while stop.exitCode != 0 && reconnect("stop", stop.exitCode, recovering: true)
            quiesced = stop.exitCode == 0
        }
        if !quiesced {
            FileHandle.standardError.write(Data("Remote gate may still be running; evidence: \(planResult.paths.gateTmpPath); owner: \(planResult.paths.ownerPath). Its lane lock can block another run. Recover with breenix-runs recover x86 \(id).\n".utf8))
        }
        reconnects = 0

        let endedAt = Date()
        let endFacts = parseHostFactsSample(runner: runner, paths: planResult.paths, wallTime: endedAt)
        var pullResult = ProcessResult(exitCode: 127)
        if quiesced {
            repeat {
                pullResult = (try? runner.run(planResult.pullEvidence)) ?? ProcessResult(exitCode: 255)
            } while transportLost(pullResult) && reconnect("evidence", pullResult.exitCode, recovering: true)
        }
        if pullResult.exitCode != 0 || pullResult.stdout.isEmpty {
            if finishStatus == 0 { finishStatus = 1 }
        }
        finishStatus = signals.signalNumber.map { 128 + $0 } ?? finishStatus
        let serialRefs: [SerialRef]
        var authoritative = false
        do {
            serialRefs = try harvestSerials(pullResult: pullResult, runDirectory: runDirectory)
            authoritative = pullResult.exitCode == 0 && !pullResult.stdout.isEmpty
        } catch {
            finishStatus = 1
            gateError = error
            serialRefs = [
                SerialRef(name: "serial_user.txt", path: "serial_user.txt", bytes: fileSize(runDirectory.appendingPathComponent("serial_user.txt")), stream: .com1),
                SerialRef(name: "serial_kernel.txt", path: "serial_kernel.txt", bytes: fileSize(runDirectory.appendingPathComponent("serial_kernel.txt")), stream: .com2)
            ]
        }
        let gateStdoutBytes = fileSize(gateStdoutURL)
        let command = readableGateCommand(paths: planResult.paths, boots: options.boots, mode: options.mode)
        var env = gateEnvironment(paths: planResult.paths, timeoutSecs: timeoutSecs, qemuProfile: options.qemuProfile, suite: options.suite)
        env["BREENIX_LANE_KEY"] = options.laneKey
        env["BREENIX_REMOTE_QUIESCED"] = quiesced ? "1" : "0"
        var captures = [CaptureRef(name: "gate-stdout.txt", path: "gate-stdout.txt", bytes: gateStdoutBytes),
                        CaptureRef(name: "reconnects.jsonl", path: "reconnects.jsonl", bytes: fileSize(reconnectURL))]
        for screen in screenNames(in: runDirectory) {
            let url = runDirectory.appendingPathComponent(screen)
            captures.append(CaptureRef(name: screen, path: screen, bytes: fileSize(url)))
        }

        let gateStdoutText = String(decoding: try Data(contentsOf: gateStdoutURL), as: UTF8.self)
        if finishStatus == 0 && !gateStdoutText.split(separator: "\n").contains(where: { $0.hasPrefix("GATE: PASS (") }) {
            finishStatus = 1
        }
        env["BREENIX_GATE_FRESH"] = options.fresh ? "1" : "0"
        for line in gateStdoutText.split(separator: "\n") {
            if line.hasPrefix("[gate-tree] tree="), let field = line.split(separator: " ").first(where: { $0.hasPrefix("tree=") }) {
                env["BREENIX_REPO_DIR"] = String(field.dropFirst(5))
            }
            if line.hasPrefix("[gate-cache] HIT") || line.hasPrefix("[gate-cache] MISS") || line.hasPrefix("[gate-cache] FRESH") {
                env["BREENIX_GATE_CACHE_RESULT"] = String(line)
            }
        }
        let gateVerdictString: String
        if gateStdoutText.contains("PASS-WITH-ATTRIBUTED-LOCKUP:") {
            gateVerdictString = "PASS-WITH-ATTRIBUTED-LOCKUP"
        } else if gateStdoutText.split(separator: "\n").contains(where: {
            $0.trimmingCharacters(in: .whitespaces).hasPrefix("FAIL:")
        }) || finishStatus != 0 {
            gateVerdictString = "FAIL"
        } else {
            gateVerdictString = "PASS"
        }
        let verdict = Verdict.projectGateOutput(gateStdoutText, verdict: gateVerdictString,
                                                exitCode: Int(finishStatus), command: command)

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
            verdictSource: .gateScript(command: command, exitCode: Int(finishStatus)),
            serials: serialRefs,
            captures: captures,
            command: command,
            env: env,
            tags: options.tags,
            notes: quiesced ? nil : "Remote gate may still be running at \(planResult.paths.gateTmpPath); lane lock may remain held. Recover with breenix-runs recover x86 \(id)."
        )

        live.finish(status: finishStatus)
        if let vigilScript {
            do {
                try FinishedX86Registration.file(script: vigilScript, manifest: manifest, runDirectory: runDirectory, runner: runner, liveRegistered: live.registered, authoritative: authoritative)
            } catch {
                if options.persist { try store.writeManifest(manifest) }
                throw error
            }
        }
        if options.persist {
            try store.writeManifest(manifest)
        }
        // Only remove evidence after a completed gate and successful harvest.
        // A disconnected SSH session leaves its remote supervisor and evidence alone.
        if quiesced && gateError == nil && pullResult.exitCode == 0 && !pullResult.stdout.isEmpty {
            reconnects = 0
            var removed = ProcessResult(exitCode: 255)
            repeat {
                removed = (try? runner.run(planResult.removeClone)) ?? ProcessResult(exitCode: 255)
            } while transportLost(removed) && reconnect("cleanup", removed.exitCode, recovering: true)
            if removed.exitCode != 0 {
                FileHandle.standardError.write(Data("Remote cleanup failed: exit \(removed.exitCode); evidence: \(planResult.paths.gateTmpPath). Recover with breenix-runs recover x86 \(id).\n".utf8))
            }
        }
        if let gateError { throw gateError }
        return BeastLaunchResult(
            manifest: manifest,
            runDirectory: runDirectory,
            manifestURL: options.persist ? store.manifestURL(id: id) : nil,
            stored: options.persist
        )
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
        if let lane = paths.laneKey {
            let parent = URL(fileURLWithPath: paths.canonicalRepoDir).deletingLastPathComponent().path
            environment["BREENIX_REPO_DIR"] = parent + "/breenix-gate-cache/" + (paths.fresh ? "fresh/" + URL(fileURLWithPath: paths.clonePath).lastPathComponent : "trees/" + lane)
        }
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
