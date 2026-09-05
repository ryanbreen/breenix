#!/bin/bash
# #826/#827/R181: shared host-wide lock that serializes qemu-system-aarch64
# boots on this Mac.
#
# R181's own measurement: 4-6 concurrent qemu-system-aarch64 processes on
# this host ran the guest clock at 37-53% of wall-clock, which then falsely
# reds the strict gate's ~18s poll ceiling (#826) even though the guest was
# healthy and simply starved of host CPU. Each docker/qemu/*.sh script that
# launches qemu-system-aarch64 is expected to `source` this file and wrap
# each launch between qemu_host_lock_acquire and qemu_host_lock_release, so
# "at most one aarch64 QEMU boot alive on this host at a time" is mechanical
# rather than an operating discipline ("check pgrep, then launch") each
# gate author has to remember and re-derive by hand.
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

_QHL_LOCK_HELD=0
_QHL_LOCK_DIR=""
_QHL_DISABLED=0
_QHL_TRAP_INSTALLED=0

# The directory this lock's atomic mkdir acquires. Not the file the
# top-of-file comment's "flock on <path>" language names literally -- the
# configured/default value IS that path, used directly as the mkdir target,
# since mkdir (not a plain file) is this implementation's atomic primitive.
qemu_host_lock_dir() {
    if [ -n "${BREENIX_QEMU_LOCK:-}" ] && [ "$BREENIX_QEMU_LOCK" != "off" ]; then
        printf '%s\n' "$BREENIX_QEMU_LOCK"
    else
        printf '%s\n' "${HOME:-/tmp}/.cache/breenix/a64-qemu.lock"
    fi
}

# Host-wide count of qemu-system-aarch64 processes. Native launches (bare,
# under nice(1), or the exec'd child of timeout(1)) are counted via a
# process-name match (`pgrep -x`), not a full-command-line search: GNU
# coreutils timeout forks a monitoring parent that keeps
# `timeout N qemu-system-aarch64 ...` as its OWN argv for the child's whole
# life (found live: `pgrep -f qemu-system-aarch64` during a real
# timeout-wrapped boot returned that parent's PID and the child's PID both,
# double-counting one boot as two -- nice(1) execs in place and does not
# have this problem). Docker-wrapped launches (run-aarch64-test.sh,
# run-aarch64-userspace.sh, run-aarch64-interactive.sh) are counted
# separately: their actual qemu-system-aarch64 process runs inside
# Docker's own Linux VM, invisible to this host's process table, so the
# host-side `docker run ... qemu-system-aarch64` CLI invocation blocking
# for the container's life is the visible proxy, matched by a
# full-command-line pattern narrow enough not to pick up an unrelated
# process that merely mentions the token. Not used to kill anything --
# observational only, printed so a human or a launch log can see what this
# host was carrying at the moment of a lock acquisition.
qemu_host_lock_count() {
    # pgrep exits 1 (not an error) when no process matches, and with a caller
    # running `set -o pipefail` that would make either pipeline below "fail"
    # and, for the bare `count="$(qemu_host_lock_count)"` assignment this
    # function feeds, trip the caller's `set -e` -- an empty host is not a
    # failure, so pgrep's own exit status is swallowed in both legs.
    local native docker_wrapped
    native="$( { pgrep -x qemu-system-aarch64 2>/dev/null || true; } | wc -l | tr -d ' ')"
    docker_wrapped="$( { pgrep -f 'docker run.*qemu-system-aarch64' 2>/dev/null || true; } | wc -l | tr -d ' ')"
    echo $((native + docker_wrapped))
}

# Chains this file's own release (and, if the lock is disabled this run,
# the disabled-banner) onto whatever EXIT trap the calling script already
# had, rather than replacing it -- several callers (run-aarch64-arma609-arm.sh,
# run-aarch64-prod-profile-boot-test.sh, run-aarch64-stability-test.sh,
# run-aarch64-full-test.sh) already `trap cleanup EXIT` their own QEMU_PID
# kill+wait, and this must run in addition to that, not instead of it.
# Installed at most once per process (idempotent across a script's own
# per-boot acquire/release loop) so the chain does not grow one link per
# iteration.
_qhl_chain_exit_trap() {
    [ "$_QHL_TRAP_INSTALLED" = "1" ] && return 0
    local existing
    existing="$(trap -p EXIT 2>/dev/null | sed -E "s/^trap -- '(.*)' EXIT\$/\1/")"
    if [ -n "$existing" ]; then
        trap "$existing; qemu_host_lock_release; _qhl_verdict_banner" EXIT
    else
        trap "qemu_host_lock_release; _qhl_verdict_banner" EXIT
    fi
    _QHL_TRAP_INSTALLED=1
}

_qhl_verdict_banner() {
    if [ "$_QHL_DISABLED" = "1" ]; then
        echo "========================================="
        echo "QEMU HOST LOCK: DISABLED for this run (BREENIX_QEMU_LOCK=off)"
        echo "Concurrent aarch64 QEMU boots on this host were NOT serialized."
        echo "========================================="
    fi
}

# Blocking acquire. Prints the host aarch64 QEMU count on each call (locked
# or not). When locked and contended, prints a wait message roughly once per
# 30s span (poll granularity is 1s; the message is not itself the poll interval).
qemu_host_lock_acquire() {
    _qhl_chain_exit_trap
    local count
    count="$(qemu_host_lock_count)"
    if [ "${BREENIX_QEMU_LOCK:-}" = "off" ]; then
        _QHL_DISABLED=1
        echo "QEMU HOST LOCK: DISABLED (BREENIX_QEMU_LOCK=off) -- host aarch64 QEMU count now: $count -- NOT serializing" >&2
        return 0
    fi
    echo "QEMU HOST LOCK: host aarch64 QEMU count before acquire: $count" >&2

    local lock_dir waited=0 next_message=30
    lock_dir="$(qemu_host_lock_dir)"
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
            echo "QEMU HOST LOCK: waiting for $lock_dir (${waited}s elapsed, host aarch64 QEMU count=$(qemu_host_lock_count))..." >&2
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
