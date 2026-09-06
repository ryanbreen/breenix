#!/bin/bash
# #827: the strict and production-profile aarch64 gates score a boot purely
# from serial content, so a boot that ran out of its host-side wall-clock
# budget and a boot that genuinely wedged score identically. This file
# gives both gates one shared way to print a per-boot line of host-side
# facts -- wall-clock start/end, host aarch64 QEMU count and 1-minute load
# average sampled at boot start and again right before the kill, the QEMU
# process's own accumulated CPU time at that same "before the kill" moment,
# the guest's last observed heartbeat uptime, and an explicit `ended_by`
# field naming which bound in the caller's own poll loop actually stopped
# the boot -- so the two cases in #826 (a starved guest vs. a wedged one)
# can be told apart mechanically instead of by re-deriving an assumed
# wall-clock window after the fact.
#
# This file does not decide `ended_by` itself -- each caller's own poll
# loop is what knows which of its break conditions fired, so each caller
# sets its own `ended_by` value from its own control flow and passes it to
# gbf_emit_line below. This file only formats the resulting line
# identically for both gates and supplies the host-side measurements
# neither gate previously took.
#
# Depends on qemu_host_lock_count() from lib/qemu-host-lock.sh for the QEMU
# count fields -- callers must source that file first.
#
# #865: arch-portable. gbf_resolve_qemu_pid takes the QEMU binary name as
# its second argument (default qemu-system-aarch64, matching
# qemu_host_lock_acquire's own default) so an x86 caller can pass
# qemu-system-x86_64 and get the same "walk past a timeout(1) wrapper to
# the real QEMU child" behavior the aarch64 gates already rely on.
# gbf_qemu_cpu_seconds, gbf_last_heartbeat_uptime_ms and gbf_emit_line take
# no binary name -- they operate on a PID or a serial file already resolved
# to the right process by the caller, and carry no arch-specific text.

# Host wall clock in whole milliseconds since the epoch. macOS `date` has no
# %N (that is GNU-only), so this uses the same `python3 -c
# 'import time; ...'` millisecond-timestamp pattern
# scripts/parallels/launcher-smoke.sh already uses on this same host. Falls
# back to 0 (not a failure) if python3 is unavailable, so a caller running
# under `set -e` does not abort on this call.
gbf_host_ms_now() {
    python3 -c 'import time; print(int(time.time() * 1000))' 2>/dev/null || echo 0
}

# 1-minute host load average. macOS has no /proc/loadavg; `sysctl -n
# vm.loadavg` prints "{ 1m 5m 15m }" as one line, so the 1-minute figure is
# the second field. Falls back to /proc/loadavg's own first field on a
# Linux host, and to the literal string "NA" if neither source is
# readable -- not a failure under the caller's `set -e`.
gbf_load_1m() {
    local v
    v="$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}')"
    if [ -n "$v" ]; then
        printf '%s' "$v"
        return
    fi
    if [ -r /proc/loadavg ]; then
        awk '{print $1}' /proc/loadavg
        return
    fi
    printf 'NA'
}

# The PID `ps -o time=` should actually be pointed at: each caller of this
# file that runs under `timeout` launches via `timeout N qemu-system-aarch64
# ... &`, and `$!` after
# that line is coreutils `timeout`'s OWN pid, not the qemu-system-aarch64
# child it execs -- the exact "timeout forks a monitor that keeps its own
# pid alive for the child's whole life" shape
# docker/qemu/lib/qemu-host-lock.sh's own header documents finding for
# `pgrep -f` double-counting a single boot. Measured live while gathering
# this file's own boot proofs: the `timeout` wrapper's own `ps -o time=`
# read 0:00.00 for an entire boot while its qemu-system-aarch64 child read
# 0:14.86 over the same window -- reporting the wrapper's own idle-monitor
# CPU time as "QEMU's CPU time" would silently misreport a boot at
# ~0.00s regardless of how busy QEMU actually was. This walks one level
# down via `pgrep -P` to find that child; a
# caller that instead backgrounds the QEMU binary directly (no
# `timeout` in front of it) has $QEMU_PID already pointing at the right
# process, which the `comm=` check below detects and returns unchanged.
# `$2` names which QEMU binary to look for (default qemu-system-aarch64,
# matching qemu_host_lock_acquire's own default) -- an x86 caller passes
# qemu-system-x86_64 so this resolves the right child under a `timeout`
# wrapper there too (e.g. the fs-fault gate's x86 leg).
gbf_resolve_qemu_pid() {
    local wrapper_pid="$1"
    local qemu_bin="${2:-qemu-system-aarch64}"
    local comm child
    comm="$(ps -o comm= -p "$wrapper_pid" 2>/dev/null | tr -d ' ')"
    case "$comm" in
        *"$qemu_bin") printf '%s' "$wrapper_pid"; return ;;
    esac
    child="$(pgrep -P "$wrapper_pid" -x "$qemu_bin" 2>/dev/null | head -1)"
    if [ -n "$child" ]; then
        printf '%s' "$child"
    else
        printf '%s' "$wrapper_pid"
    fi
}

# QEMU's own accumulated CPU time (user+system), converted from `ps -o
# time=`'s own [[dd-]hh:]mm:ss[.ss] display to whole (fractional) seconds.
# `pid` MUST already be qemu-system-aarch64's own pid -- resolve it with
# gbf_resolve_qemu_pid first if the caller launched via `timeout`. MUST be
# sampled before the caller's own `kill $QEMU_PID` -- ps has no output
# for a PID that is already gone, which this function reports as "NA"
# rather than 0 (0 would misreport a boot killed too late to sample, as if
# QEMU had burned no CPU at all).
gbf_qemu_cpu_seconds() {
    local pid="$1"
    local raw
    raw="$(ps -o time= -p "$pid" 2>/dev/null | tr -d ' ')"
    if [ -z "$raw" ]; then
        printf 'NA'
        return
    fi
    awk -v t="$raw" 'BEGIN {
        days = 0; rest = t
        if (split(t, dp, "-") == 2) { days = dp[1]; rest = dp[2] }
        n = split(rest, p, ":")
        if (n == 3) { secs = p[1] * 3600 + p[2] * 60 + p[3] }
        else if (n == 2) { secs = p[1] * 60 + p[2] }
        else { secs = p[1] }
        secs += days * 86400
        printf "%.2f", secs
    }'
}

# The guest's own last `[heartbeat] ... uptime_ms=N` field in the serial
# file -- the guest-side clock reading the host-side wall-clock facts this
# file gathers are compared against. "NA" when the file has no heartbeat
# line (a boot that died before userspace, or has not written a serial
# file at all).
gbf_last_heartbeat_uptime_ms() {
    local serial_file="$1"
    local val
    if [ ! -f "$serial_file" ]; then
        printf 'NA'
        return
    fi
    val="$(grep -aoE '\[heartbeat\][^]]*uptime_ms=[0-9]+' "$serial_file" 2>/dev/null \
        | grep -aoE 'uptime_ms=[0-9]+' | tail -1 | cut -d= -f2)"
    if [ -z "$val" ]; then
        printf 'NA'
    else
        printf '%s' "$val"
    fi
}

# Prints the one GATE_BOOT_FACTS line both gates share, given the values
# named in #827: boot number, host wall-clock start/end (ms), host aarch64
# QEMU count and 1-minute load average at start and at the pre-kill sample,
# QEMU's own accumulated CPU seconds at that same pre-kill sample, the
# guest's last heartbeat uptime, and the caller-derived ended_by label.
# Shared here (rather than composed inline at each of the 2 call sites) so
# the two gates cannot drift into two different field orders or labels --
# the structural ratchet's field census checks the literal format string
# this one function owns.
gbf_emit_line() {
    local boot="$1" host_ms_start="$2" host_ms_end="$3" \
        qemu_at_start="$4" load_at_start="$5" \
        qemu_at_end="$6" load_at_end="$7" \
        qemu_cpu_s="$8" guest_uptime_ms="$9" ended_by="${10}"
    printf '[GATE_BOOT_FACTS:boot=%s:host_ms=%s-%s:qemu_at_start=%s:load_at_start=%s:qemu_at_end=%s:load_at_end=%s:qemu_cpu_s=%s:guest_uptime_ms=%s:ended_by=%s]\n' \
        "$boot" "$host_ms_start" "$host_ms_end" \
        "$qemu_at_start" "$load_at_start" \
        "$qemu_at_end" "$load_at_end" \
        "$qemu_cpu_s" "$guest_uptime_ms" "$ended_by"
}
