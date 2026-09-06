#!/bin/bash
# #826/#827/#834/#865/R181: shared host-wide lock that serializes QEMU boots,
# one lock domain per QEMU binary.
#
# R181's own measurement: 4-6 concurrent qemu-system-aarch64 processes on
# this host ran the guest clock at 37-53% of wall-clock, which then falsely
# reds the strict gate's ~18s poll ceiling (#826) even though the guest was
# healthy and simply starved of host CPU. #865 found the identical shape on
# the beast x86 host: several qemu-system-x86_64 TCG lanes running
# concurrently starve each other and hit the same kind of false timing
# ceiling (Run Inspector run 20260906T014135Z-x86_64-gate-864a: boot=900s
# total=1081s, failed on clock_gettime_test -- the #631/#766 timing
# signature -- while other lanes were booting). Each docker/qemu/*.sh or
# scripts/*.sh script that launches a QEMU binary is expected to `source`
# this file and wrap each launch between qemu_host_lock_acquire and
# qemu_host_lock_release, so
# "at most one <arch> QEMU boot alive on this host at a time" is mechanical
# rather than an operating discipline ("check pgrep, then launch") each
# gate author has to remember and re-derive by hand.
#
# ARCHITECTURE AWARENESS (#865): the three functions below that need to
# know which QEMU binary is in play -- qemu_host_lock_dir,
# qemu_host_lock_count, qemu_host_lock_acquire -- take the binary name
# (`qemu-system-aarch64` or `qemu-system-x86_64`) as their first argument,
# defaulting to `qemu-system-aarch64` when omitted so the ~30 aarch64 call
# sites that predate #865 and call e.g. `qemu_host_lock_acquire` bare keep
# working byte-for-byte. A non-empty first argument that is neither of
# those two literals (a typo, e.g. `qemu-system-x86`) is refused via
# `_qhl_resolve_bin` below -- exit 1 with a FAIL line on stderr -- rather
# than silently treated as `qemu-system-aarch64` by a `case ... *)`
# default arm, which would otherwise pick the wrong lock domain and the
# wrong `pgrep` target with no indication anything was wrong.
# qemu_host_lock_release and qemu_host_lock_track_pid
# need no such argument -- they operate on this process's own global lock
# state (`_QHL_LOCK_DIR`, `_QHL_TRACKED_PIDS`), already scoped to whichever
# binary the matching acquire call was made for. An x86 caller passes the
# binary name
# explicitly: `qemu_host_lock_acquire qemu-system-x86_64`. This is one lock
# PER BINARY NAME, not one global lock -- an aarch64 boot and an x86 boot on
# the same host do not contend with each other (they are different QEMU
# binaries competing for different CPU-emulation resources; #865's own
# report is x86 lanes starving x86 lanes on the beast host, whose own
# process table carries 0 qemu-system-aarch64 entries), so each binary gets
# its own lock directory
# (`a64-qemu.lock` / `x86-qemu.lock` under the same cache root) and its own
# `pgrep -x <binary>` census. A THIRD kind of exclusion ("only one QEMU of
# any arch, host-wide") is not what this file provides -- #865's own audit
# found 0 call sites in this repo asking for it (the beast x86 host's
# process table carries 0 aarch64 QEMU entries, and the Mac's native
# aarch64 gates do not also launch x86 QEMU while they run).
#
# LOCK IMPLEMENTATION: macOS ships no flock(1). This uses an atomic mkdir
# as the lock primitive (mkdir either succeeds and creates the directory,
# or fails with EEXIST -- there is no window where two racing callers can
# both see success), with a PID file inside for stale-lock reclaim, rather
# than a file-descriptor flock(2) acquired via e.g. `python3 -c
# 'fcntl.flock(...)'`. The reason is where these scripts need the lock held
# across: each caller acquires immediately before backgrounding
# qemu-system-aarch64 (`... &`; QEMU_PID=$!) and releases only after a
# later, separate poll-and-kill sequence -- often dozens of seconds and
# several external commands later, sometimes inside a per-boot loop. An
# flock(2) held via a helper subprocess does not survive past that
# subprocess's own exit unless the fd is threaded through an exec() chain
# (open the fd, flock it, exec into qemu-system-aarch64, let the kernel
# auto-release on that process's eventual exit) -- workable, but it forces
# each of the ~20 call sites this lock wraps into the same
# spawn-and-exec shape, several of which background qemu-system-aarch64
# directly in the CALLING shell (not via a wrapper exec) specifically so
# they can `kill $QEMU_PID` it early on a crash marker or a poll timeout.
# A shell-level mkdir/rmdir pair needs no fd bookkeeping and no exec chain:
# acquire is one function call, release is another, and they can sit on
# opposite sides of arbitrary intervening shell code in the same process,
# matching the shape each caller already has. Stale-lock recovery
# (a script killed with an untrappable SIGKILL, or a host crash, leaves the
# lock directory behind) is a `kill -0` liveness check on the PID recorded
# inside it, which is no weaker a check than relying on the kernel to
# auto-release an flock on process death -- and it is the same primitive
# either way, since flock's auto-release on SIGKILL is a kernel table
# cleanup, not something the acquiring process's own trap can perform.
#
# OPT-OUT: BREENIX_QEMU_LOCK=off turns locking off (the only opt-out).
# Otherwise BREENIX_QEMU_LOCK, if set, is the lock directory path
# to use instead of the default -- e.g. to give one deliberately-isolated
# test lane its own lock domain. A disabled run prints a loud banner both
# when locking is skipped and again, via the EXIT trap, immediately after
# the script's own PASS/FAIL output -- adjacent to the verdict without this
# file having to know each of its ~20 callers' own verdict-printing shape.
# BREENIX_QEMU_LOCK must be an absolute path when set to anything other
# than "off" -- qemu_host_lock_acquire refuses a relative value the same
# way callers already refuse a relative BREENIX_GATE_TMP (the F6 guard from
# PR #801/#797), since a relative lock directory resolves against whatever
# directory happens to be current when each caller runs and would silently
# split what is supposed to be one shared lock domain into several.
#
# PID TRACKING: qemu_host_lock_track_pid registers the PID a caller just
# launched (a backgrounded qemu-system-aarch64, or a backgrounded `docker
# run` client) so this lock's own EXIT trap can terminate it even on a path
# where the calling script's own poll-and-kill sequence does not run -- e.g. a
# SIGTERM/SIGINT delivered to just the script's own PID during the boot-poll
# window, which does not propagate to a foreground or backgrounded child on
# its own. Callers with their own working cleanup trap do not need this;
# it exists for the callers that had no cleanup trap of their own.

_QHL_LOCK_HELD=0
_QHL_LOCK_DIR=""
_QHL_DISABLED=0
_QHL_TRAP_INSTALLED=0
_QHL_TRACKED_PIDS=()
_QHL_RESOLVED_BIN=""

# Resolves $1 to a known QEMU binary name into the global
# _QHL_RESOLVED_BIN, defaulting to qemu-system-aarch64 when $1 is empty
# (the pre-#865 bare-call shape, see the ARCHITECTURE AWARENESS header
# comment) and returns 0. On a non-empty $1 that is neither
# qemu-system-aarch64 nor qemu-system-x86_64 -- a typo, e.g.
# qemu-system-x86 -- prints a FAIL line to stderr, clears
# _QHL_RESOLVED_BIN, and returns 1 (#865 F1: this used to be a silent
# `case ... *)` fallthrough to the aarch64 lock domain and pgrep target
# in both qemu_host_lock_dir and qemu_host_lock_count). 3/3 call sites
# below invoke it as a plain function call --
# `_qhl_resolve_bin "${1:-}" || exit 1` -- not via `$( )` command
# substitution, so the `exit 1` on an unrecognized name runs in the SAME
# shell as the caller (qemu_host_lock_dir/qemu_host_lock_count's own
# bodies already execute inside whatever subshell their own caller's
# command substitution created; qemu_host_lock_acquire's body runs
# directly in the calling script's shell) rather than being swallowed by
# an extra throwaway subshell this helper would otherwise add.
_qhl_resolve_bin() {
    _QHL_RESOLVED_BIN="${1:-qemu-system-aarch64}"
    case "$_QHL_RESOLVED_BIN" in
        qemu-system-aarch64|qemu-system-x86_64)
            return 0
            ;;
        *)
            echo "QEMU HOST LOCK: FAIL -- unrecognized QEMU binary name: '$_QHL_RESOLVED_BIN' (expected qemu-system-aarch64 or qemu-system-x86_64)" >&2
            _QHL_RESOLVED_BIN=""
            return 1
            ;;
    esac
}

# The directory this lock's atomic mkdir acquires, for the QEMU binary named
# in $1 (default qemu-system-aarch64, see the ARCHITECTURE AWARENESS header
# comment). Not the file the top-of-file comment's "flock on <path>"
# language names literally -- the configured/default value IS that path,
# used directly as the mkdir target, since mkdir (not a plain file) is this
# implementation's atomic primitive. BREENIX_QEMU_LOCK, when set to a real
# path, overrides the default for WHICHEVER binary the caller asked for --
# it is one override knob shared by both arches, matching its existing
# single-knob shape rather than growing an a64/x86-specific pair of
# variables no caller has ever needed.
qemu_host_lock_dir() {
    _qhl_resolve_bin "${1:-}" || exit 1
    local qemu_bin="$_QHL_RESOLVED_BIN"
    if [ -n "${BREENIX_QEMU_LOCK:-}" ] && [ "$BREENIX_QEMU_LOCK" != "off" ]; then
        printf '%s\n' "$BREENIX_QEMU_LOCK"
        return
    fi
    case "$qemu_bin" in
        qemu-system-x86_64)
            printf '%s\n' "${HOME:-/tmp}/.cache/breenix/x86-qemu.lock"
            ;;
        *)
            printf '%s\n' "${HOME:-/tmp}/.cache/breenix/a64-qemu.lock"
            ;;
    esac
}

# Host-wide count of the QEMU binary named in $1 (default
# qemu-system-aarch64). Native launches (bare, under nice(1), or the exec'd
# child of timeout(1)) are counted via a process-name match (`pgrep -x`),
# not a full-command-line search: GNU coreutils timeout forks a monitoring
# parent that keeps `timeout N <qemu_bin> ...` as its OWN argv for the
# child's whole life (found live: `pgrep -f qemu-system-aarch64` during a
# real timeout-wrapped boot returned that parent's PID and the child's PID
# both, double-counting one boot as two -- nice(1) execs in place and does
# not have this problem). Docker-wrapped launches (run-aarch64-test.sh,
# run-aarch64-userspace.sh, run-aarch64-interactive.sh, and their x86 twins)
# are counted separately: their actual QEMU process runs inside Docker's own
# Linux VM, invisible to this host's process table, so the host-side
# `docker run ... <qemu_bin>` CLI invocation blocking for the container's
# life is the visible proxy, matched by a full-command-line pattern narrow
# enough not to pick up an unrelated process that merely mentions the
# token. Not used to kill anything -- observational only, printed so a
# human or a launch log can see what this host was carrying at the moment
# of a lock acquisition.
qemu_host_lock_count() {
    _qhl_resolve_bin "${1:-}" || exit 1
    local qemu_bin="$_QHL_RESOLVED_BIN"
    # pgrep exits 1 (not an error) when no process matches, and with a caller
    # running `set -o pipefail` that would make either pipeline below "fail"
    # and, for the bare `count="$(qemu_host_lock_count)"` assignment this
    # function feeds, trip the caller's `set -e` -- an empty host is not a
    # failure, so pgrep's own exit status is swallowed in both legs.
    local native docker_wrapped
    native="$( { pgrep -x "$qemu_bin" 2>/dev/null || true; } | wc -l | tr -d ' ')"
    docker_wrapped="$( { pgrep -f "docker run.*$qemu_bin" 2>/dev/null || true; } | wc -l | tr -d ' ')"
    echo $((native + docker_wrapped))
}

# Chains this file's own cleanup (killing any PID registered via
# qemu_host_lock_track_pid, then releasing the lock, then -- if the lock is
# disabled this run -- the disabled-banner) onto whatever EXIT trap the
# calling script already had, rather than replacing it -- several callers
# (run-aarch64-arma609-arm.sh, run-aarch64-prod-profile-boot-test.sh,
# run-aarch64-stability-test.sh, run-aarch64-full-test.sh) already
# `trap cleanup EXIT` their own QEMU_PID kill+wait, and this must run in
# addition to that, not instead of it. The tracked-PID kill runs BEFORE the
# existing trap and the lock release, not after: it is what makes the
# release reachable on a caller with no cleanup of its own, and
# running it before the existing trap adds no cost when that trap does its
# own kill+wait on the same PID (a dead/reaped PID is a silent no-op).
# Installed at most once per process (idempotent across a script's own
# per-boot acquire/release loop) so the chain does not grow one link per
# iteration.
_qhl_chain_exit_trap() {
    [ "$_QHL_TRAP_INSTALLED" = "1" ] && return 0
    local existing
    existing="$(trap -p EXIT 2>/dev/null | sed -E "s/^trap -- '(.*)' EXIT\$/\1/")"
    if [ -n "$existing" ]; then
        trap "_qhl_kill_tracked_pids; $existing; qemu_host_lock_release; _qhl_verdict_banner" EXIT
    else
        trap "_qhl_kill_tracked_pids; qemu_host_lock_release; _qhl_verdict_banner" EXIT
    fi
    _QHL_TRAP_INSTALLED=1
}

# Registers a PID this script just launched (a backgrounded
# qemu-system-aarch64, or a backgrounded `docker run` client) so the EXIT
# trap above can terminate it even when this script has no cleanup of its
# own. Call immediately after capturing the PID (`... &`; PID=$!). Safe to
# call across a retry/boot loop -- earlier, already-dead entries are silent
# no-ops when the trap fires.
qemu_host_lock_track_pid() {
    [ -n "${1:-}" ] || return 0
    _QHL_TRACKED_PIDS+=("$1")
}

# Kills and reaps each PID registered via qemu_host_lock_track_pid. An
# already-dead or already-reaped PID is a harmless no-op here (both kill and
# wait fail silently) -- the normal case, since most exits reach this only
# after the calling script's own kill+wait already ran.
_qhl_kill_tracked_pids() {
    local pid
    for pid in "${_QHL_TRACKED_PIDS[@]:-}"; do
        [ -n "$pid" ] || continue
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
}

_qhl_verdict_banner() {
    if [ "$_QHL_DISABLED" = "1" ]; then
        echo "========================================="
        echo "QEMU HOST LOCK: DISABLED for this run (BREENIX_QEMU_LOCK=off)"
        echo "Concurrent aarch64 QEMU boots on this host were NOT serialized."
        echo "========================================="
    fi
}

# Blocking acquire for the QEMU binary named in $1 (default
# qemu-system-aarch64, see the ARCHITECTURE AWARENESS header comment).
# Prints the host count for that binary on each call (locked or not). When
# locked and contended, prints a wait message roughly once per 30s span
# (poll granularity is 1s; the message is not itself the poll interval).
qemu_host_lock_acquire() {
    _qhl_resolve_bin "${1:-}" || exit 1
    local qemu_bin="$_QHL_RESOLVED_BIN"
    _qhl_chain_exit_trap
    if [ -n "${BREENIX_QEMU_LOCK:-}" ] && [ "$BREENIX_QEMU_LOCK" != "off" ]; then
        case "$BREENIX_QEMU_LOCK" in
            /*) ;;
            *)
                echo "QEMU HOST LOCK: FAIL -- BREENIX_QEMU_LOCK must be an absolute path (or 'off'), got: $BREENIX_QEMU_LOCK" >&2
                exit 1
                ;;
        esac
    fi
    local count
    count="$(qemu_host_lock_count "$qemu_bin")"
    if [ "${BREENIX_QEMU_LOCK:-}" = "off" ]; then
        _QHL_DISABLED=1
        echo "QEMU HOST LOCK: DISABLED (BREENIX_QEMU_LOCK=off) -- host $qemu_bin count now: $count -- NOT serializing" >&2
        return 0
    fi
    echo "QEMU HOST LOCK: host $qemu_bin count before acquire: $count" >&2

    local lock_dir waited=0 next_message=30
    lock_dir="$(qemu_host_lock_dir "$qemu_bin")"
    mkdir -p "$(dirname "$lock_dir")" 2>/dev/null || true

    while ! mkdir "$lock_dir" 2>/dev/null; do
        if [ -f "$lock_dir/pid" ]; then
            local holder
            holder="$(cat "$lock_dir/pid" 2>/dev/null || true)"
            if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
                echo "QEMU HOST LOCK: reclaiming stale lock at $lock_dir (holder PID $holder is dead)" >&2
                rm -rf "$lock_dir" 2>/dev/null || true
                continue
            fi
        fi
        if [ "$waited" -ge "$next_message" ]; then
            echo "QEMU HOST LOCK: waiting for $lock_dir (${waited}s elapsed, host $qemu_bin count=$(qemu_host_lock_count "$qemu_bin"))..." >&2
            next_message=$((next_message + 30))
        fi
        sleep 1
        waited=$((waited + 1))
    done
    echo $$ > "$lock_dir/pid" 2>/dev/null || true
    _QHL_LOCK_HELD=1
    _QHL_LOCK_DIR="$lock_dir"
    return 0
}

# Releases the lock if this process holds it. Safe to call when not held
# (the EXIT trap's safety-net call after an explicit release is a no-op).
qemu_host_lock_release() {
    [ "$_QHL_LOCK_HELD" = "1" ] || return 0
    rm -rf "$_QHL_LOCK_DIR" 2>/dev/null || true
    _QHL_LOCK_HELD=0
    _QHL_LOCK_DIR=""
    return 0
}
