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
/// five positional strings. Defaults match the `breenix-x86` Incus container
/// as verified directly against beast (2026-09-06): repo at `/root/breenix`
/// (root user - this container has no `wrb` account; CLAUDE.md's generic
/// `sudo -iu wrb` beast pattern is for a different container and does not
/// apply here), rust-fork repoint target at `/root/breenix/rust-fork-real`
/// (gitignored, not part of any clone - see `run-x86-gate.sh`'s own
/// `BREENIX_RUST_FORK` repoint logic), cargo on PATH via `/root/.cargo/env`.
public struct BeastPaths: Equatable, Sendable {
    public var host: String
    public var container: String
    public var canonicalRepoDir: String
    public var clonePath: String
    public var rustForkPath: String
    public var cargoEnvPath: String

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

    public static func plan(sha: String, boots: Int, mode: RemoteGateMode, timeoutSecs: Int, paths: BeastPaths) -> Plan {
        Plan(
            sha: sha,
            boots: boots,
            mode: mode,
            timeoutSecs: timeoutSecs,
            paths: paths,
            prepareClone: prepareCloneRequest(sha: sha, paths: paths),
            runGate: runGateRequest(boots: boots, mode: mode, timeoutSecs: timeoutSecs, paths: paths),
            pullEvidence: pullEvidenceRequest(paths: paths),
            removeClone: removeCloneRequest(paths: paths)
        )
    }

    // Refreshes the canonical checkout from origin, then makes a private
    // --shared clone (object storage shared via alternates, no second network
    // fetch needed - verified live 2026-09-06) and checks out the exact sha
    // under test. This is the private-clone-per-run DESIGN.md 5.2 requires
    // ([[workflow-worktree-isolation]] R83; #797 is concurrent lanes
    // clobbering a shared /tmp path). `rm -rf` before clone is defensive
    // against a stale directory reusing the same id, not expected to fire.
    public static func prepareCloneRequest(sha: String, paths: BeastPaths) -> ProcessRequest {
        let script = "git -C \(paths.canonicalRepoDir) fetch origin"
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
    public static func runGateRequest(boots: Int, mode: RemoteGateMode, timeoutSecs: Int, paths: BeastPaths) -> ProcessRequest {
        let script = "mkdir -p \(paths.gateTmpPath)"
            + " && source \(paths.cargoEnvPath)"
            + " && env BREENIX_GATE_TMP=\(paths.gateTmpPath)"
            + " BREENIX_REPO_DIR=\(paths.clonePath)"
            + " BREENIX_RUST_FORK=\(paths.rustForkPath)"
            + " BREENIX_GATE_TIMEOUT=\(timeoutSecs)"
            + " \(paths.clonePath)/docker/qemu/run-x86-gate.sh \(boots) \(mode.rawValue)"
        return sshRequest(paths: paths, remote: incusBashLC(paths: paths, script: script))
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
        let remote = "sudo -n incus exec \(paths.container) -- rm -rf \(paths.clonePath)"
        return sshRequest(paths: paths, remote: remote)
    }

    // Beast's own host-facts sample - DESIGN.md 5.3's concept applied to the
    // ACTUAL execution host for an x86 run, which is beast, not this Mac:
    // loadavg/mem/CPU model from /proc rather than sysctl (breenix-x86 is
    // Ubuntu, not macOS), qemu peer counts via `pgrep -c -f` (plain `-x`/`-c`
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

    private static func sshRequest(paths: BeastPaths, remote: String, combineOutput: Bool = true) -> ProcessRequest {
        ProcessRequest(
            executable: "/usr/bin/ssh",
            arguments: ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=\(sshTimeoutSecs)", paths.host, remote],
            combineOutput: combineOutput
        )
    }
}
