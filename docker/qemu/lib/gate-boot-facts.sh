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
#
# #865: `|| true` on the sysctl assignment is load-bearing on Linux, not
# decorative. `vm.loadavg` is a valid sysctl key on macOS (the only host
# this file's 2 aarch64 callers run on); on the beast Linux host #865's 2
# x86 callers added, it is not, so `sysctl -n vm.loadavg` exits nonzero.
# The 2 x86 callers (unlike the 2 aarch64 ones) run under `set -o
# pipefail`, which makes that failing first pipeline stage's nonzero
# status the WHOLE PIPELINE's exit status even with stderr redirected
# away and even though the last stage (`awk`) succeeds -- this is why the
# hazard is latent on the 2 aarch64 callers regardless of host (no
# pipefail to propagate the failure) and live only where both conditions
# hold: pipefail AND a host where the sysctl key does not exist. The 4
# callers run under `set -e`, under which a plain `v="$(pipeline)"`
# assignment with no `|| true` is a simple command whose failure is fatal
# -- not merely "$v ends up empty, fall through to the /proc/loadavg
# branch below" the way the rest of this function's own logic intends.
# Caught live on beast running run-x86-boot-tests.sh #865 added: the gate
# aborted at this exact line before the /proc/loadavg fallback ran, with
# no GATE_BOOT_FACTS line printed for that boot.
gbf_load_1m() {
    local v
    v="$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}')" || true
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
    # #865: `|| true` is load-bearing, the same hazard gbf_load_1m's own
    # comment documents in detail: on the 2 x86 callers (which run under
    # `set -o pipefail`), `ps` exiting nonzero for an already-gone PID
    # makes the whole pipeline's status nonzero even though `tr`
    # (downstream of it) succeeds, and `set -e` (the 4 callers of this
    # file run under it) treats this bare assignment's failure as fatal.
    # This function's own callers can legitimately reach it with an
    # already-gone `wrapper_pid` (a boot whose QEMU process died on its
    # own before the caller's poll loop's own bound was reached), so this
    # is not a hypothetical for the 2 callers it can bite.
    comm="$(ps -o comm= -p "$wrapper_pid" 2>/dev/null | tr -d ' ')" || true
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
# gbf_resolve_qemu_pid first if the caller launched via `timeout`. Intended
# to be sampled before the caller's own `kill $QEMU_PID` for a live CPU-time
# reading, but a PID that already exited on its own (before the caller's
# poll loop's own bound was reached) is exactly what the "NA" branch below
# is for, not merely a boot killed too late to sample -- ps has no output
# for a PID that is already gone, which is this function's cue to report
# "NA" rather than the misleading 0 (0 would claim QEMU burned no CPU at
# all). #865: `|| true` on the assignment is what lets that branch be
# reached at all -- see gbf_resolve_qemu_pid's own comment on the identical
# `set -o pipefail` + `set -e` hazard, caught live on beast the same way.
gbf_qemu_cpu_seconds() {
    local pid="$1"
    local raw
    raw="$(ps -o time= -p "$pid" 2>/dev/null | tr -d ' ')" || true
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
# line (a boot that died before userspace, has not written a serial file
# at all, or -- #865's own x86 callers -- an arch/profile whose init
# service does not print this literal marker).
#
# #865: `|| true` on the assignment is the same `set -o pipefail` + `set
# -e` hazard gbf_load_1m and gbf_resolve_qemu_pid's own comments document,
# in its "no match at all" shape rather than their "command not found"
# shape: a zero-match first grep exits nonzero, and unlike the pgrep-into-
# head chains elsewhere in this file, the SECOND grep here also sees empty
# input and ALSO exits nonzero (`tail`/`cut` downstream of it exit 0 on
# empty input either way, which does not save the pipeline under
# pipefail's own "last command to exit nonzero" rule). Caught live on
# beast running one of #865's x86 callers, where this literal marker did
# not appear in the captured serial -- confirming the zero-match case is
# real on x86, not only a theoretical one this fix guards preemptively.
gbf_last_heartbeat_uptime_ms() {
    local serial_file="$1"
    local val
    if [ ! -f "$serial_file" ]; then
        printf 'NA'
        return
    fi
    val="$(grep -aoE '\[heartbeat\][^]]*uptime_ms=[0-9]+' "$serial_file" 2>/dev/null \
        | grep -aoE 'uptime_ms=[0-9]+' | tail -1 | cut -d= -f2)" || true
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
