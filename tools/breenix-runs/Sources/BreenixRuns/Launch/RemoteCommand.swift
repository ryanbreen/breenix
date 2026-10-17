import Foundation

extension ProcessRequest: @unchecked Sendable {}
extension ProcessResult: @unchecked Sendable {}

/// The gate script's own `full`/`kthread` MODE parameter
/// (`docker/qemu/run-x86-gate.sh` usage: `[count] [mode]`).
public enum RemoteGateMode: String, CaseIterable, Sendable {
    case full
    case kthread
}

/// Every path/identity value the beast x86 launcher needs, gathered in one
/// place so `RemoteCommand`'s builders take a single argument rather than
/// five positional strings. Deployment settings can be supplied through
/// the initializer for the target environment.
/// Keep the rust-fork source external to disposable clones and use the
/// configured cargo environment for each remote command.
public struct BeastPaths: Equatable, Sendable {
    public var host: String
    public var container: String
    public var canonicalRepoDir: String
    public var clonePath: String
    public var rustForkPath: String
    public var cargoEnvPath: String
    public var laneKey: String?
    public var requestedSHA: String?
    public var fresh = false
    public var targetPolicyStamp: String?

    public init(
        host: String = "beast",
        container: String = "breenix-x86",
        canonicalRepoDir: String = "/root/breenix",
        clonePath: String,
        rustForkPath: String = "/root/breenix/rust-fork-real",
        cargoEnvPath: String = "/root/.cargo/env"
    ) {
        self.host = host
        self.container = container
        self.canonicalRepoDir = canonicalRepoDir
        self.clonePath = clonePath
        self.rustForkPath = rustForkPath
        self.cargoEnvPath = cargoEnvPath
    }

    /// DESIGN.md Sec 1.6 / 5.2: BREENIX_GATE_TMP must be a per-run directory
    /// the launcher owns, INSIDE the per-run clone, never the shared
    /// canonical checkout (#797's concurrent-lane clobber).
    public var gateTmpPath: String {
        clonePath + "/gate-tmp"
    }

    /// Completion tombstones outlive evidence cleanup and delayed launch bootstraps.
    public var ownerPath: String {
        URL(fileURLWithPath: canonicalRepoDir).deletingLastPathComponent()
            .appendingPathComponent("breenix-run-owners")
            .appendingPathComponent(URL(fileURLWithPath: clonePath).lastPathComponent).path
    }
}

/// Pure builders for the beast x86 launcher's remote-facing commands.
///
/// Every function here is a pure function of its arguments: same inputs,
/// same `ProcessRequest`, every time, no I/O. `BeastLauncher` (impure -
/// generates the run id and clone path, then actually calls a
/// `ProcessRunner`) is the only caller with side effects.
public enum RemoteCommand {
    private static let sshTimeoutSecs = 15

    public struct Plan: Equatable, Sendable {
        public var sha: String
        public var boots: Int
        public var mode: RemoteGateMode
        public var timeoutSecs: Int
        public var paths: BeastPaths
        public var prepareClone: ProcessRequest
        public var runGate: ProcessRequest
        public var pullEvidence: ProcessRequest
        public var removeClone: ProcessRequest
    }

    /// An effort-suite id: lowercase words of a-z and 0-9 joined by '-'. Checked
    /// character by character (no regex, whose `$` would also match before a final
    /// newline), because an accepted id goes unquoted into a remote shell command.
    public static func isSuiteID(_ id: String) -> Bool {
        !id.isEmpty && id.split(separator: "-", omittingEmptySubsequences: false).allSatisfy { word in
            !word.isEmpty && word.unicodeScalars.allSatisfy { ("a"..."z").contains($0) || ("0"..."9").contains($0) }
        }
    }

    public static func isSuiteList(_ list: String) -> Bool {
        let ids = list.split(separator: ",", omittingEmptySubsequences: false).map(String.init)
        return ids.allSatisfy(isSuiteID) && Set(ids).count == ids.count
    }

    public static func plan(sha: String, boots: Int, mode: RemoteGateMode, timeoutSecs: Int, paths: BeastPaths, qemuProfile: X86HardwareProfile? = nil, suite: String? = nil, fullBackstopSecs: Int? = nil, slotHelperBase64: String? = nil, progressBundleBase64: String? = nil, treeHelperBase64: String? = nil, quietOverride: String? = nil) -> Plan {
        Plan(
            sha: sha,
            boots: boots,
            mode: mode,
            timeoutSecs: timeoutSecs,
            paths: paths,
            prepareClone: prepareCloneRequest(sha: sha, paths: paths, treeHelperBase64: treeHelperBase64),
            runGate: runGateRequest(boots: boots, mode: mode, timeoutSecs: timeoutSecs, paths: paths, qemuProfile: qemuProfile, suite: suite, fullBackstopSecs: fullBackstopSecs, slotHelperBase64: slotHelperBase64, progressBundleBase64: progressBundleBase64, quietOverride: quietOverride),
            pullEvidence: pullEvidenceRequest(paths: paths),
            removeClone: removeCloneRequest(paths: paths)
        )
    }

    // Fetches only the requested object into the canonical cache, then makes a private
    // --shared clone (object storage shared via alternates, no second network
    // fetch needed - verified live 2026-09-06) and checks out the exact sha
    // under test. This is the private-clone-per-run DESIGN.md 5.2 requires
    // ([[workflow-worktree-isolation]] R83; #797 is concurrent lanes
    // clobbering a shared /tmp path). `rm -rf` before clone is defensive
    // against a stale directory reusing the same id, not expected to fire.
    public static func prepareCloneRequest(sha: String, paths: BeastPaths, treeHelperBase64: String? = nil) -> ProcessRequest {
        if let treeHelperBase64, paths.laneKey != nil {
            let script = "mkdir -p \(paths.gateTmpPath) && printf %s \(treeHelperBase64) | base64 -d > \(paths.gateTmpPath)/gate-tree.py"
            return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: script))
        }
        // Concurrent launchers share this cache: do not update its remote refs,
        // tags or FETCH_HEAD, or spawn background maintenance during preparation.
        let script = "git -C \(paths.canonicalRepoDir) fetch --no-tags --no-write-fetch-head --no-auto-gc origin \(sha)"
            + " && rm -rf \(paths.clonePath)"
            + " && git clone --shared \(paths.canonicalRepoDir) \(paths.clonePath)"
            + " && git -C \(paths.clonePath) checkout --detach \(sha)"
        return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: script))
    }

    // `mkdir -p` runs BEFORE the gate script: if the build steps inside
    // run-x86-gate.sh fail before the per-boot loop creates its own OUTDIR,
    // gate-tmp/ must still exist so pullEvidenceRequest's tar never fails on
    // a missing directory - an evidence-pull failure must never be conflated
    // with a gate failure.
    // A suite id (validated by `isSuiteID`, so safe unquoted) makes the gate boot the
    // production kernel running /sbin/suite-<id>, with a QMP socket in gate-tmp for its screen.
    // An id that is not one never reaches the shell: the request fails the gate instead of
    // quietly running the ordinary one (BeastLauncher refuses such an id before this).
    // Apple's zlib compressor emits a raw DEFLATE stream. Bound the inline
    // transport size rather than letting catalog growth approach ARG_MAX.
    private static func compressedPayload(_ encoded: String) -> String? {
        guard let data = Data(base64Encoded: encoded),
              let compressed = try? (data as NSData).compressed(using: .zlib) else { return nil }
        return (compressed as Data).base64EncodedString()
    }

    public static func runGateRequest(boots: Int, mode: RemoteGateMode, timeoutSecs: Int, paths: BeastPaths, qemuProfile: X86HardwareProfile? = nil, suite: String? = nil, fullBackstopSecs: Int? = nil, slotHelperBase64: String? = nil, progressBundleBase64: String? = nil, quietOverride: String? = nil) -> ProcessRequest {
        // Explicit opt-in test/profile features; never interpolate unchecked shell text.
        let features = ProcessInfo.processInfo.environment["BREENIX_GATE_KERNEL_FEATURES"] ?? ""
        guard features.isEmpty || features.range(of: "^[a-zA-Z0-9_-]+(,[a-zA-Z0-9_-]+)*$", options: .regularExpression) != nil else {
            return sshRequest(paths: paths, remote: "exit 2")
        }
        let featureEnv = features.isEmpty ? "" : " BREENIX_GATE_KERNEL_FEATURES=\(features)"
        let quiet = quietOverride ?? ""
        guard quiet.isEmpty || quiet == "0" || quiet == "1" else {
            return sshRequest(paths: paths, remote: "exit 2")
        }
        let quietEnv = quiet.isEmpty ? "" : " BREENIX_QUIET=\(quiet)"
        let profileEnv: String = qemuProfile.map { " BREENIX_QEMU_PROFILE=\($0.rawValue)" } ?? ""
        if let stamp = paths.targetPolicyStamp, stamp.range(of: "^x86:[0-9a-f]{40}$", options: .regularExpression) == nil {
            return sshRequest(paths: paths, remote: "exit 2")
        }
        let policyEnv = paths.targetPolicyStamp.map { " BREENIX_TARGET_POLICY_CHECKED=\($0)" } ?? ""
        var suiteEnv = ""
        if let suite {
            guard isSuiteList(suite) else {
                return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: "mkdir -p \(paths.gateTmpPath) && echo \"GATE: FAIL (the requested suite is not a suite id)\" && exit 1"))
            }
            suiteEnv = " BREENIX_BOOT_SUITE=\(suite) BREENIX_QMP_SOCKET=\(paths.gateTmpPath)/qmp.sock"
        }
        var slotIdentity = ""
        if let lane = paths.laneKey, let sha = paths.requestedSHA {
            let parent = URL(fileURLWithPath: paths.canonicalRepoDir).deletingLastPathComponent().path
            let worktree = paths.fresh ? "${BREENIX_GATE_CACHE_DIR:-\(parent)/breenix-gate-cache}/fresh/" + URL(fileURLWithPath: paths.clonePath).lastPathComponent : "${BREENIX_GATE_CACHE_DIR:-\(parent)/breenix-gate-cache}/trees/\(lane)"
            slotIdentity = " BREENIX_SLOT_WORKTREE=\"\(worktree)\" BREENIX_SLOT_COMMIT=\(sha)"
        }
        let helper = slotHelperBase64 == nil ? paths.canonicalRepoDir + "/scripts/host-slots.py" : paths.gateTmpPath + "/host-slots.py"
        let packedHelper = slotHelperBase64.flatMap(compressedPayload)
        let packedProgress = progressBundleBase64.flatMap(compressedPayload)
        guard (slotHelperBase64 == nil || packedHelper != nil),
              (progressBundleBase64 == nil || packedProgress != nil) else {
            return sshRequest(paths: paths, remote: "exit 2")
        }
        let installHelper = packedHelper.map {
            " && python3 -c \"import base64,pathlib,sys,zlib; pathlib.Path(sys.argv[1]).write_bytes(zlib.decompress(base64.b64decode(sys.argv[2]),-15))\" \(helper) \($0)"
        } ?? ""
        let progressRoot = paths.gateTmpPath + "/watchdog"
        let installProgress = packedProgress.map {
            " && python3 -c \"import base64,json,pathlib,sys,zlib; root=pathlib.Path(sys.argv[1]); [( (root/k).parent.mkdir(parents=True,exist_ok=True), (root/k).write_bytes(base64.b64decode(v))) for k,v in json.loads(zlib.decompress(base64.b64decode(sys.argv[2]),-15)).items()]\" \(progressRoot) \($0)"
        } ?? ""
        // Historical source retains its original watchdog policy by default.
        let historicalWatch = ProcessInfo.processInfo.environment["BREENIX_WATCH_HISTORICAL"] == "1"
        let progressEnv = progressBundleBase64 == nil ? "" : " BREENIX_SLOT_PROGRESS_HELPER=\(progressRoot)/scripts/boot_progress.py BREENIX_WATCH_HISTORICAL=\(historicalWatch ? 1 : 0)"
        let gate: String
        if let laneKey = paths.laneKey, let sha = paths.requestedSHA {
            gate = "python3 \(paths.gateTmpPath)/gate-tree.py \(paths.canonicalRepoDir) \(laneKey) \(sha) \(paths.gateTmpPath) \(boots) \(mode.rawValue)"
        } else {
            gate = "\(paths.clonePath)/docker/qemu/run-x86-gate.sh \(boots) \(mode.rawValue)"
        }
        // Historical gates lack an internal supervisor: hold both resources
        // around that gate, using the current launcher's helper outside checkout.
        let historicalAdmission = paths.laneKey == nil ? "if [ ! -f \(paths.clonePath)/scripts/host-slots.py ]; then python3 \(helper) acquire x86-build && python3 \(helper) acquire x86-boot || exit 1; fi; " : ""
        let launch = "python3 \(helper) supervise -- bash -c \"\(historicalAdmission)exec \(gate)\""
        let identityCode = #"import json,pathlib,sys;pid=int(sys.argv[1]);birth=pathlib.Path(\"/proc/%d/stat\"%pid).read_text().rsplit(\")\",1)[1].split()[19];pathlib.Path(sys.argv[2]).write_text(json.dumps([pid,birth,sys.argv[3]]))"#
        let script = "mkdir -p \(paths.gateTmpPath)" + installHelper + installProgress
            + " && python3 -c \"\(identityCode)\" \"$$\" \(paths.gateTmpPath)/launcher-gate.json \(helper)"
            + " && source \(paths.cargoEnvPath)"
            + " && exec env BREENIX_GATE_TMP=\(paths.gateTmpPath)"
            + " BREENIX_REPO_DIR=\(paths.clonePath)"
            + " BREENIX_RUST_FORK=\(paths.rustForkPath)"
            + " BREENIX_GATE_TIMEOUT=\(timeoutSecs)"
            + " BREENIX_FULL_BACKSTOP=\(fullBackstopSecs ?? max(1800, timeoutSecs))"
            + " CARGO_BUILD_JOBS=4"
            + (paths.fresh ? " BREENIX_GATE_FRESH=1" : "")
            + featureEnv + quietEnv + profileEnv + policyEnv + suiteEnv + slotIdentity + progressEnv
            + " " + launch
        return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: script))
    }

    /// The exclusive claim is retained for seven days after completion.
    /// A lost launch acknowledgement can safely repeat this request without rerunning it.
    static let detachedOwner = #"""
import base64, fcntl, json, os, pathlib, signal, subprocess, sys, time
root = pathlib.Path(sys.argv[1])
owners = pathlib.Path(sys.argv[4]) if len(sys.argv) > 4 else root
owners.mkdir(parents=True, exist_ok=True)
record = owners / "gate-owner.json"
boot_path = pathlib.Path("/proc/sys/kernel/random/boot_id")
boot_id = boot_path.read_text().strip()
def publish(value):
    temporary = record.with_suffix(".tmp")
    temporary.write_text(json.dumps(value))
    temporary.replace(record)
def birth(pid):
    try:
        return pathlib.Path("/proc/%d/stat" % pid).read_text().rsplit(")",1)[1].split()[19]
    except FileNotFoundError:
        return None
if sys.argv[2] == "worker":
    with (owners / "gate-launch.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if record.exists():
            sys.exit(0)
        owner = dict(pid=os.getpid(), birth=pathlib.Path("/proc/self/stat").read_text().rsplit(")",1)[1].split()[19], boot_id=boot_id, state="running")
        root.mkdir(parents=True, exist_ok=True)
        if (owners / "cancel").exists():
            owner.update(state="finished", exit_code=130)
            publish(owner)
            sys.exit(0)
        child = None
        def stop(number, frame):
            if child is not None and child.poll() is None:
                child.send_signal(signal.SIGTERM)
        signal.signal(signal.SIGTERM, stop)
        publish(owner)
        try:
            with (root / "gate-output.log").open("wb") as output:
                child = subprocess.Popen(["bash", "-lc", base64.b64decode(sys.argv[3]).decode()], start_new_session=True, stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT)
                owner.update(child_pid=child.pid, child_birth=birth(child.pid))
                publish(owner)
        except BaseException as error:
            owner.update(state="finished", exit_code=1, error=str(error))
            publish(owner)
            sys.exit(0)
    status = child.wait()
    owner.update(state="finished", exit_code=status if status >= 0 else 128 - status)
    publish(owner)
else:
    # Keep completed claims for seven days: retries remain idempotent while
    # terminal tombstones have bounded retention. Never prune running owners.
    import shutil
    for directory in owners.parent.glob("breenix-*"):
        candidate = directory / "gate-owner.json"
        try:
            if time.time() - candidate.stat().st_mtime > 7 * 86400 and json.loads(candidate.read_text()).get("state") == "finished":
                shutil.rmtree(directory)
        except (OSError, ValueError):
            pass
    child = subprocess.Popen([sys.executable, "-c", base64.b64decode(sys.argv[3]).decode(), str(root), "worker", sys.argv[2], str(owners)], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True)
    deadline = time.monotonic() + 10
    while not record.exists():
        if child.poll() is not None or time.monotonic() >= deadline:
            sys.exit(75)
        time.sleep(.05)

"""#

    public static func startGateRequest(gate: ProcessRequest, paths: BeastPaths) -> ProcessRequest {
        guard let remote = gate.arguments.last,
              let begin = remote.range(of: "-- bash -lc '") else { return sshRequest(paths: paths, remote: "exit 2") }
        let script = String(remote[begin.upperBound...].dropLast())
        let owner = Data(detachedOwner.utf8).base64EncodedString()
        let command = Data(script.utf8).base64EncodedString()
        var request = sshRequest(paths: paths, remote: "sudo -n incus exec \(paths.container) -- python3 -c 'import base64;exec(base64.b64decode(\"\(owner)\"))' \(paths.gateTmpPath) \(command) \(owner) \(paths.ownerPath) # gate-start")
        request.timeoutSecs = 30
        return request
    }

    /// Follow only; there is no launch operation on the reconnect path.
    /// Exit 75 means a healthy, still-running owner, not a transport failure.
    static let gateFollower = #"""
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
offset = int(sys.argv[2])
owners = pathlib.Path(sys.argv[3]) if len(sys.argv) > 3 else root
deadline = time.monotonic() + 60
while True:
    try:
        owner = json.loads((owners / "gate-owner.json").read_text())
    except FileNotFoundError:
        sys.exit(76)
    try:
        with (root / "gate-output.log").open("rb") as output:
            output.seek(offset)
            data = output.read(1024 * 1024)
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        offset += len(data)
    except FileNotFoundError:
        data = b""
    if owner["state"] == "finished" and not data:
        # Gate stderr is stored in gate-output.log; this channel is owner protocol only.
        print("BREENIX_GATE_FINISHED=" + str(owner["exit_code"]), file=sys.stderr, flush=True)
        sys.exit(0)
    if owner["state"] == "running":
        try:
            fields = pathlib.Path("/proc/%d/stat" % owner["pid"]).read_text().rsplit(")",1)[1].split()
            if owner.get("boot_id") != pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip() or fields[19] != owner["birth"] or fields[0] == "Z":
                sys.exit(76)
        except FileNotFoundError:
            sys.exit(76)
    if time.monotonic() >= deadline:
        sys.exit(75)
    time.sleep(.1)
"""#

    public static func followGateRequest(paths: BeastPaths, offset: UInt64) -> ProcessRequest {
        let encoded = Data(gateFollower.utf8).base64EncodedString()
        var request = sshRequest(paths: paths, remote: "sudo -n incus exec \(paths.container) -- python3 -c 'import base64;exec(base64.b64decode(\"\(encoded)\"))' \(paths.gateTmpPath) \(offset) \(paths.ownerPath) # gate-follow", combineOutput: false, liveStream: true)
        request.timeoutSecs = 75
        return request
    }

    /// Nonblocking output keeps the heartbeat deadline active under backpressure.
    /// Catch up in bounded bursts; the final evidence is harvested independently.
    static let serialReader = #"""
import base64, json, os, pathlib, select, sys, time
root = pathlib.Path(sys.argv[1])
count = int(sys.argv[2])
offsets = {tuple([int(k.split(":")[0]), k.split(":")[1]]): v for k, v in json.loads(sys.argv[3] if len(sys.argv) > 3 else "{}").items()}
last = time.monotonic()
pending = bytearray()
os.set_blocking(1, False)
while time.monotonic() - last < 5:
    ready, writable, _ = select.select([0], [1] if pending else [], [], .01 if pending else 1)
    if ready:
        if not os.read(0, 4096):
            break
        last = time.monotonic()
    if len(pending) < 4 * 1024 * 1024:
        for boot in range(1, count + 1):
            directory = root / ("breenix_gate_%d" % boot)
            if not all((directory / ("serial_%s.log" % stream)).exists() for stream in ("user", "kernel")):
                continue
            for stream in ("user", "kernel"):
                key = (boot, stream)
                path = directory / ("serial_%s.log" % stream)
                try:
                    with path.open("rb") as log:
                        log.seek(offsets.get(key, 0))
                        data = log.read(1024 * 1024)
                        if key not in offsets or data:
                            offsets[key] = log.tell()
                            pending.extend((json.dumps(dict(boot=boot, stream=stream, offset=offsets[key] - len(data), data=base64.b64encode(data).decode())) + "\n").encode())
                except FileNotFoundError:
                    pass
    if pending:
        try:
            written = os.write(1, pending)
            del pending[:written]
        except BlockingIOError:
            pass
        except BrokenPipeError:
            break
"""#

    public static func streamSerialsRequest(paths: BeastPaths, boots: Int, offsets: [String: Int] = [:]) -> ProcessRequest {
        let resume = String(data: try! JSONSerialization.data(withJSONObject: offsets, options: [.sortedKeys]), encoding: .utf8)!
        let encoded = Data(serialReader.utf8).base64EncodedString()
        let python = "import base64;exec(base64.b64decode(\"\(encoded)\"))"
        return sshRequest(paths: paths, remote: "sudo -n incus exec \(paths.container) -- python3 -u -c '\(python)' \(paths.gateTmpPath) \(boots) '\(resume)'", combineOutput: false, liveStream: true)
    }

    /// Signal this run's supervisor and detached worker, including a worker whose
    /// foreground handle died during disconnect. PID birth checks exclude reused PIDs.
    static let gateStopper = #"""
import json, os, pathlib, signal, sys, time

def stop_gate(record, proc):
    try:
        saved = json.loads(record.read_text())
        pid, birth = saved[:2]
    except (FileNotFoundError, ValueError):
        saved = [None, None, str(record.parent / "host-slots.py")]
        pid, birth = saved[:2]
    def identity(pid):
        try:
            fields = (proc / str(pid) / "stat").read_text().rsplit(")", 1)[1].split()
            return fields[19] if fields[0] != "Z" else None
        except (FileNotFoundError, ProcessLookupError):
            return None
    targets = {pid: birth} if pid is not None else {}
    helper = (saved[2] if len(saved) > 2 else str(record.parent / "host-slots.py")).encode()
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            argv = (entry / "cmdline").read_bytes().split(b"\0")
            if helper in argv and b"supervise" in argv:
                worker = int(entry.name)
                token = identity(worker)
                if token is not None:
                    targets[worker] = token
        except (FileNotFoundError, ProcessLookupError):
            pass
    def alive(pid, token):
        return identity(pid) == token
    for target, token in targets.items():
        if alive(target, token):
            try:
                os.kill(target, signal.SIGTERM)
            except ProcessLookupError:
                pass
    deadline = time.monotonic() + 130
    while any(alive(p, t) for p, t in targets.items()) and time.monotonic() < deadline:
        time.sleep(.2)
    return 1 if any(alive(p, t) for p, t in targets.items()) else 0

def cancel(root, owners, proc):
    import fcntl
    owners.mkdir(parents=True, exist_ok=True)
    record = owners / "gate-owner.json"
    with (owners / "gate-launch.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        # The marker closes the pre-owner/pre-identity race, including delayed bootstraps.
        (owners / "cancel").touch()
        missing = False
        try:
            owner = json.loads(record.read_text())
        except FileNotFoundError:
            missing = True
            owner = dict(state="finished", exit_code=130)
            record.write_text(json.dumps(owner))
        same_boot = owner.get("boot_id") == (proc / "sys/kernel/random/boot_id").read_text().strip()
    def alive(pid, birth):
        if not same_boot or pid is None:
            return False
        try:
            fields = (proc / str(pid) / "stat").read_text().rsplit(")",1)[1].split()
            return fields[19] == birth and fields[0] != "Z"
        except FileNotFoundError:
            return False
    if missing or (owner.get("state") != "finished" and same_boot and not alive(owner.get("pid"), owner.get("birth"))):
        # Missing ownership metadata is not proof that the gate stopped. Find
        # this run's detached worker / pre-install shell by its exact run path,
        # then stop its supervisor even if launcher identity was never written.
        targets = {}
        for entry in proc.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                argv = (entry / "cmdline").read_bytes().split(b"\0")
                fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
                worker = str(root).encode() in argv and b"worker" in argv
                shell = any(("BREENIX_GATE_TMP=" + str(root) + " ").encode() in arg for arg in argv)
                if (worker or shell) and fields[0] != "Z":
                    targets[int(entry.name)] = fields[19]
            except FileNotFoundError:
                pass
        same_boot = True  # These identities were sampled from the current /proc.
        for pid, token in targets.items():
            if alive(pid, token):
                try: os.kill(pid, signal.SIGTERM)
                except ProcessLookupError: pass
        deadline = time.monotonic() + 130
        while any(alive(pid, token) for pid, token in targets.items()) and time.monotonic() < deadline:
            time.sleep(.2)
        if any(alive(pid, token) for pid, token in targets.items()):
            return 1
        if stop_gate(root / "launcher-gate.json", proc) != 0:
            return 1
    if owner.get("state") != "finished":
        pid, token = owner.get("pid"), owner.get("birth")
        if alive(pid, token):
            os.kill(pid, signal.SIGTERM)
        else:
            # A surviving child is still ours even when its owner died.
            child, birth = owner.get("child_pid"), owner.get("child_birth")
            if alive(child, birth):
                os.kill(child, signal.SIGTERM)
        identity = root / "launcher-gate.json"
        if same_boot and identity.exists() and stop_gate(identity, proc) != 0:
            return 1
        deadline = time.monotonic() + 130
        while (alive(pid, token) or alive(owner.get("child_pid"), owner.get("child_birth"))) and time.monotonic() < deadline:
            time.sleep(.2)
        if alive(pid, token) or alive(owner.get("child_pid"), owner.get("child_birth")):
            return 1
        # Preserve the worker's terminal verdict if it finished during cancellation.
        owner = json.loads(record.read_text())
        if owner.get("state") != "finished":
            owner.update(state="finished", exit_code=76)
            record.write_text(json.dumps(owner))
    # gate-tree's evidence guard must also see the abandoned lane as finished.
    lane = root / "run-owner.json"
    if lane.exists():
        value = json.loads(lane.read_text())
        value["state"] = "finished"
        lane.write_text(json.dumps(value))
    return 0

if __name__ == "__main__":
    sys.exit(cancel(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path("/proc")))
"""#

    public static func stopGateRequest(paths: BeastPaths) -> ProcessRequest {
        let encoded = Data(gateStopper.utf8).base64EncodedString()
        var request = sshRequest(paths: paths, remote: "sudo -n incus exec \(paths.container) -- python3 -c 'import base64;exec(base64.b64decode(\"\(encoded)\"))' \(paths.gateTmpPath) \(paths.ownerPath)")
        request.timeoutSecs = 300
        return request
    }

    // No `bash -lc` needed: a single command, no env sourcing, no shell
    // features. `combineOutput: false` is load-bearing here - stdout carries
    // raw gzip bytes and must never be interleaved with stderr text (see the
    // pure builder's call to `sshRequest` below).
    public static func pullEvidenceRequest(paths: BeastPaths) -> ProcessRequest {
        let remote = "sudo -n incus exec \(paths.container) -- tar -czf - -C \(paths.clonePath) gate-tmp"
        return sshRequest(paths: paths, remote: remote, combineOutput: false)
    }

    public static func removeCloneRequest(paths: BeastPaths) -> ProcessRequest {
        let remote: String
        if paths.laneKey != nil {
            remote = "sudo -n incus exec \(paths.container) -- python3 \(paths.gateTmpPath)/gate-tree.py remove-evidence \(paths.clonePath)"
        } else {
            remote = "sudo -n incus exec \(paths.container) -- rm -rf \(paths.clonePath)"
        }
        return sshRequest(paths: paths, remote: remote)
    }

    // Beast's own host-facts sample - DESIGN.md 5.3's concept applied to the
    // ACTUAL execution host for an x86 run, which is beast, not this Mac:
    // loadavg/mem/CPU model from /proc on the Linux execution environment;
    // qemu peer counts via `pgrep -c -f` (plain `-x`/`-c`
    // without `-f` silently matches nothing - the name is >15 chars,
    // verified against beast). No single quote appears anywhere in this
    // script: it runs inside `bash -lc '<script>'`, so a literal `'` would
    // terminate that quoting early. Awk field references are escaped
    // (`\$1`) so the INNER bash's double-quote parsing does not expand them
    // as its own positional parameters before awk ever sees them. Verified
    // byte-for-byte against a real Foundation.Process invocation of this
    // exact string on 2026-09-06 (exit 0, all six fields parsed correctly).
    public static func hostFactsRequest(paths: BeastPaths) -> ProcessRequest {
        let script = #"read la1 la2 la3 _ < /proc/loadavg; memkb=$(awk "/MemTotal/{print \$2}" /proc/meminfo); qpeers86=$(pgrep -c -f qemu-system-x86_64 || echo 0); qpeersarm=$(pgrep -c -f qemu-system-aarch64 || echo 0); qver=$(qemu-system-x86_64 --version 2>/dev/null | head -1); cpumodel=$(awk -F: "/model name/{print \$2; exit}" /proc/cpuinfo | sed "s/^ *//"); echo "loadavg=$la1 $la2 $la3"; echo "qemu_peers_x86=$qpeers86"; echo "qemu_peers_aarch64=$qpeersarm"; echo "mem_total_kb=$memkb"; echo "qemu_version=$qver"; echo "cpu_model=$cpumodel""#
        return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: script))
    }

    /// Parses `hostFactsRequest`'s stdout into a `HostFactsSample`. Pure -
    /// no I/O - so it is testable directly against a fixture string with no
    /// process involved. Any line without a recognized `key=` prefix
    /// (including beast's trailing terminal-reset escape bytes) is ignored,
    /// not treated as an error. A field whose key is entirely absent from the
    /// text is `nil`/`0`, never fabricated.
    public static func parseHostFacts(_ text: String, wallTime: Date) -> HostFactsSample {
        var fields: [String: String] = [:]
        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            guard let eq = line.firstIndex(of: "=") else { continue }
            let key = String(line[line.startIndex..<eq])
            let value = String(line[line.index(after: eq)...])
            fields[key] = value
        }

        let loadParts = (fields["loadavg"] ?? "").split(separator: " ").compactMap { Double($0) }

        return HostFactsSample(
            wallTime: wallTime,
            qemuPeersAarch64: fields["qemu_peers_aarch64"].flatMap(Int.init) ?? 0,
            qemuPeersX86_64: fields["qemu_peers_x86"].flatMap(Int.init) ?? 0,
            loadavg1: loadParts.count > 0 ? loadParts[0] : nil,
            loadavg5: loadParts.count > 1 ? loadParts[1] : nil,
            loadavg15: loadParts.count > 2 ? loadParts[2] : nil,
            qemuCPUSeconds: nil,
            thermalPressure: nil,
            hostModel: fields["cpu_model"].map { $0.trimmingCharacters(in: .whitespaces) },
            physMem: fields["mem_total_kb"].flatMap(UInt64.init).map { $0 * 1024 },
            qemuVersion: fields["qemu_version"].map { $0.trimmingCharacters(in: .whitespaces) },
            gitSHA: nil,
            gitDirty: nil,
            clockRatio: nil
        )
    }

    private static func incusBashLC(paths: BeastPaths, script: String) -> String {
        "sudo -n incus exec \(paths.container) -- bash -lc '\(script)'"
    }

    private static func sshRequest(paths: BeastPaths, remote: String, combineOutput: Bool = true, liveStream: Bool = false) -> ProcessRequest {
        ProcessRequest(
            executable: "/usr/bin/ssh",
            arguments: ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=\(sshTimeoutSecs)", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2"] + [paths.host, remote],
            combineOutput: combineOutput
        )
    }
}
