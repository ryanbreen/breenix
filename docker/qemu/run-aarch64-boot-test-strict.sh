#!/bin/bash
# Strict ARM64 boot test - runs multiple iterations and requires ALL to pass
# Used for CI to catch regressions. Does NOT retry failed boots.
#
# Unlike run-aarch64-boot-test-native.sh which uses retries (masking failures),
# this test counts every boot attempt. A single failure means the test fails.
# A boot is accepted only after both userspace liveness and exec smoke completion.
# Serial output from every failed iteration is preserved in a never-cleared directory.
#
# Usage: ./run-aarch64-boot-test-strict.sh [iterations]
#        Default: 20 iterations
#
# Exit codes:
#   0 - All iterations passed
#   1 - One or more iterations failed

set -e

ITERATIONS=${1:-20}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/R181: this gate's qemu-system-aarch64 boot(s) run behind the
# host-wide lock in lib/qemu-host-lock.sh, so at most one aarch64 QEMU is
# active on this host for the boot's duration.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #827: per-boot host-side facts (wall-clock window, host QEMU count and
# load average at start/kill, QEMU's own CPU time, the guest's last
# heartbeat, and which bound ended the boot) -- see that file's own header
# for why a starved boot and a wedged boot could not be told apart before.
# shellcheck source=lib/gate-boot-facts.sh
source "$SCRIPT_DIR/lib/gate-boot-facts.sh"
# #825: two concurrent runs of this gate (e.g. two worktrees on the same host,
# both native QEMU rather than the shared beast container #797/#801 covered)
# each hardcoded the identical /tmp/breenix_aarch64_strict_$iteration and
# /tmp/breenix_aarch64_strict_failures paths, so one run's rm -rf/mkdir could
# delete and rewrite the serial another run's poll loop was mid-boot scoring,
# and the first run then reported the second run's kernel as its own result --
# the false 18/20 red this issue reports. Defaulting to /tmp keeps every
# existing caller byte-identical; a concurrent-lane launcher sets this to a
# per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever directory
# happens to be current when each function below runs (the same F6 guard PR
# #801 gave the x86 gate scripts for #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "GATE: FAIL (BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP)"; exit 1 ;;
esac
# construct_residual is the counted frame residue of the two construction-failure arms read off a measured green run, and it is architecture-specific (4 on x86, 2 on aarch64) because the two page-table constructors record different table-frame counts.
INIT_DESIGNATION_ORACLE_LITERAL='[INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=2:construct_residual=2:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]'
INIT_GROUP_REFUSAL_ORACLE_LITERAL='[INIT_GROUP_REFUSAL_ORACLE:aarch64:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]'
# driven=2 proves both handoff seams ran; stage1/2 return, wake, and park fields
# expose D1/D2. stage3_elapsed_ok=1 proves the interval the oracle measured
# reached the full requested duration -- since #627 that interval is anchored
# to the same clock read the kernel used to compute the deadline, not to a
# later oracle-internal read, so this bit can no longer read 0 on a wait that
# was never actually short. arm_delay_us is that retired gap, kept visible.
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms is the measured duration; residual/balance prove cleanup.
# claim-lint:ok: #627 -- provable by construction from program order (futex.rs
# reads base_ns before its deadline check; record_arm's own clock read comes
# after), not by boot sampling: see kernel/src/syscall/futex_oracle.rs::record_arm
# and validate_futex_oracle_record_arm_anchor in tests/teardown_structure.rs.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:arm_delay_us=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
# resolved_production may be zero once #605's early-slot-consumption defect is fixed; deterministic resolved_exercised proves the resolver ran.
SCHED_STRAND_ORACLE_PATTERN='\[SCHED_STRAND_ORACLE:aarch64:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[1-9][0-9]*:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+:nonprogress=[0-9]+:queued_on_nondispatching_cpu=[0-9]+:worst_queued_nondispatch_ms=[0-9]+:worst_cpu_scheduler_silence_ms=[0-9]+:worst_silence_cpu=[0-9]+\]'
STRAND_INJECT_ORACLE_PATTERN='\[STRAND_INJECT_ORACLE:aarch64:legA_exercised=1:legA_recovered=1:legB_exercised=1:legB_recovered=1:stranded=0\]'
# P6a PR-2 gate extras (b)/(f)/(g). Every field is a delta the oracle drives
# itself inside one run, so the whole line is a literal: two fixture rows, one
# joined by retirement (retire_second) and one by the reap (reap_second), the
# gauge back at its entry value (resident_delta=0) and no tombstone left behind
# (tombstone_rows=0 is absolute, not a delta). Observed on this gate's own
# profile before it was pinned: 20/20 boots of the 2026-08-25 run printed it
# exactly once. Without this pin, deleting the oracle's registry entry left this
# gate, [BOOT_TESTS:PASS] and every structural suite green.
TOMBSTONE_JOIN_ORACLE_LITERAL='[TOMBSTONE_JOIN_ORACLE:aarch64:retire_second=1:reap_second=1:removed=2:resident_delta=0:tombstone_rows=0:PASS]'
# #796. The 12 fields are driven inside one run. armed=1 and pm_busy_probe=1 are
# the anti-vacuity pair: the peer CPU really held PROCESS_MANAGER, and an
# independent try-lock read confirmed it busy at the instant the measured fcntl
# was issued.
# first_wait_us is pinned HERE to at least four digits -- i.e. >= 1000 us of the
# oracle's 8000 us overlap window -- so a call that sailed through an
# uncontended lock cannot score even if the oracle's own
# >= FCNTL_PM_MIN_WAIT_US conjunct is deleted. R157/F3: the previous [0-9]+
# accepted first_wait_us=0, which left the in-kernel conjunct as the sole
# authority and this gate green after deleting it.
# first_errno=9 is EBADF: the driving thread is a kthread with no process row, so
# the repaired syscall reaches the lookup and fails there instead of reporting
# EAGAIN from the lock. eagain=0 is the property under test; on origin/main the
# same oracle prints eagain=64:first_errno=11 and a FAIL verdict.
# The shape changed with the arming rendezvous: `attempts=[1-3]` is gone -- there
# is no retry loop to count -- and arm_wait_us (how long the driver waited for
# the peer to publish its hold), acquired and hold_safety took its place.
# hold_safety=0 is pinned because a hold released on the holder's own safety
# deadline is a hold the driver did not use, whatever the other fields read.
FCNTL_PM_CONTENTION_ORACLE_PATTERN='\[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=[0-9]+:armed=1:acquired=1:holder_cpu=[0-9]+:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=[1-9][0-9]{3,}:hold_safety=0:hold_done=1:joined=1:PASS\]'
# FCNTL_PM_WAIT_SELFCHECK (R157/F3). The pattern above is this gate's sole
# gate-side reading that the measured fcntl really waited for a held lock, so
# check that the pattern separates the two cases BEFORE it is used to score any
# boot. The review's finding was that a first_wait_us=[0-9]+ pin accepts
# first_wait_us=0, which left the oracle's own in-kernel floor as the sole
# authority; that pin fails the first check below instead of scoring a boot
# green.
# claim-lint:ok: 3 of 3 mutation runs, recorded in the #796 doc's STEP 3
# anti-vacuity block -- shipped pattern exits 0, [0-9]+ exits 1 on the zero leg,
# [0-9]{9,} exits 1 on the 8032 leg.
fcntl_pm_oracle_sample() {
    printf '[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=4021:armed=1:acquired=1:holder_cpu=1:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=%s:hold_safety=0:hold_done=1:joined=1:PASS]\n' "$1"
}
if fcntl_pm_oracle_sample 0 | grep -qE "$FCNTL_PM_CONTENTION_ORACLE_PATTERN"; then
    echo "FAIL: FCNTL_PM_CONTENTION_ORACLE_PATTERN accepts first_wait_us=0, so this gate would score green on a call that never waited for the process-manager lock"
    exit 1
fi
if ! fcntl_pm_oracle_sample 8032 | grep -qE "$FCNTL_PM_CONTENTION_ORACLE_PATTERN"; then
    echo "FAIL: FCNTL_PM_CONTENTION_ORACLE_PATTERN rejects first_wait_us=8032, a wait the repaired oracle really records, so this gate can never pass"
    exit 1
fi
# #812. The holder takes the same non-blocking PROCESS_MANAGER acquisition a
# syscall body takes, with a preempt-disable and NetRx softirq work pending on
# its own CPU, and stays there for longer than two timer ticks. Two of the
# eleven fields are the anti-vacuity pair: masked_in_hold=1 is the reading that
# the acquisition really masked this CPU, and netrx_pending_at_release=1 is the
# reading that the softirq that would have deadlocked it really was pending for
# the whole window. irqs_enabled_before=1 pins the precondition -- the holder
# entered with interrupts live, the way an aarch64 syscall body does -- so a
# window that was already masked by its caller cannot score. On origin/main the
# holder's own IRQ exit runs that softirq inside the hold and wedges the CPU on
# a lock it already owns, so no verdict line is printed at all.
IRQ_HOLD_ORACLE_PATTERN='\[IRQ_HOLD_ORACLE:aarch64:attempts=[1-3]:armed=1:holder_cpu=[0-9]+:irqs_enabled_before=1:masked_in_hold=1:sends=[1-9][0-9]*:hold_us=[1-9][0-9]{3,}:netrx_pending_at_release=1:received=[1-9][0-9]*:stalled=0:hold_done=1:joined=1:PASS\]'
# IRQ_HOLD_SELFCHECK. hold_us is this gate's only gate-side reading that the
# window was actually wide enough for a timer tick to land in it, so check that
# the pattern separates the two cases BEFORE it scores any boot -- the same
# failure the #796 review found on first_wait_us, where a [0-9]+ pin accepted a
# hold of 0 us and left the oracle's own floor as the sole authority.
irq_hold_oracle_sample() {
    printf '[IRQ_HOLD_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=%s:netrx_pending_at_release=1:received=1:stalled=0:hold_done=1:joined=1:PASS]\n' "$1"
}
if irq_hold_oracle_sample 0 | grep -qE "$IRQ_HOLD_ORACLE_PATTERN"; then
    echo "FAIL: IRQ_HOLD_ORACLE_PATTERN accepts hold_us=0, so this gate would score green on a hold no timer tick could land in"
    exit 1
fi
if ! irq_hold_oracle_sample 12034 | grep -qE "$IRQ_HOLD_ORACLE_PATTERN"; then
    echo "FAIL: IRQ_HOLD_ORACLE_PATTERN rejects hold_us=12034, a window the repaired oracle really records, so this gate can never pass"
    exit 1
fi
# #821. The console TTY has its foreground process group cleared, a peer CPU
# takes PROCESS_MANAGER through the ordinary blocking accessor -- which masks
# that CPU on this architecture -- and one byte is pushed through the input IRQ
# entry while the remote hold is open. What the entry must NOT do is wait for
# that lock: on the unrepaired kernel it does, and `entry_us` reads out the
# peer's whole 20 ms window.
#
# Four of the sixteen fields are the anti-vacuity set. `fg_unset_before=1` is
# the precondition -- the deferring branch is only live while no foreground
# pgrp is set, so a console that already had one cannot score.
# `pm_busy_probe=1` is an independent try-lock reading that the peer really
# owned the lock at the instant of the injection. `hold_us` is the peer's own
# measurement of that window, pinned to at least five digits so a hold that
# collapsed cannot pass. `pm_blocking_acquires=0` is the property itself: a
# production counter that each of the 3 blocking PROCESS_MANAGER accessors
# bumps while the entry's no-blocking scope is open.
#
# `entry_us` is pinned to at most three digits -- under 1 ms against a 20 ms
# hold -- and the selfcheck below runs that pin against both readings before it
# is used to score a boot.
# claim-lint:ok: the 13 scoring legs behind this pin are recorded in
# docs/planning/green-program/irq-locks/serials/821/05-a64-gate-mutations.txt
TTY_IRQ_PM_ORACLE_PATTERN='\[TTY_IRQ_PM_ORACLE:aarch64:fg_unset_before=1:pm_blocking_acquires=0:deferred=2:pgrp_set_by_entry=0:processed=2:buffered=2:irqs_enabled_before=1:holder_cpu=[0-9]+:pm_busy_probe=1:hold_us=[2-9][0-9]{4,}:entry_us=[0-9]{1,3}:joined=1:adopted=1:adopted_pgrp=821:restored=1:PASS:peer_hold\]'
# TTY_IRQ_PM_SELFCHECK, for the reason IRQ_HOLD_SELFCHECK below gives: entry_us
# is this gate's only gate-side reading that the entry did not sit out the
# remote hold, so check that the pattern separates the two cases BEFORE it
# scores any boot. The two samples are the values this branch actually
# recorded: 2 us repaired, 20022 us with the blocking call restored
# (docs/planning/green-program/irq-locks/serials/821/).
tty_irq_pm_oracle_sample() {
    printf '[TTY_IRQ_PM_ORACLE:aarch64:fg_unset_before=1:pm_blocking_acquires=0:deferred=2:pgrp_set_by_entry=0:processed=2:buffered=2:irqs_enabled_before=1:holder_cpu=1:pm_busy_probe=1:hold_us=20000:entry_us=%s:joined=1:adopted=1:adopted_pgrp=821:restored=1:PASS:peer_hold]\n' "$1"
}
if tty_irq_pm_oracle_sample 20022 | grep -qE "$TTY_IRQ_PM_ORACLE_PATTERN"; then
    echo "FAIL: TTY_IRQ_PM_ORACLE_PATTERN accepts entry_us=20022, so this gate would score green on an input IRQ entry that waited out the whole remote hold"
    exit 1
fi
if ! tty_irq_pm_oracle_sample 2 | grep -qE "$TTY_IRQ_PM_ORACLE_PATTERN"; then
    echo "FAIL: TTY_IRQ_PM_ORACLE_PATTERN rejects entry_us=2, the reading the repaired entry really records, so this gate can never pass"
    exit 1
fi
CENSUS_WIDEN_ORACLE_PATTERN='\[CENSUS_WIDEN_ORACLE:aarch64:arm_target=[0-9]+:baseline_reported=0:armed_reported=1:tid=[1-9][0-9]*:shape=ready_queued_nondispatching:queued_nondispatching=[1-9][0-9]*:queued_nondispatch_ms=[1-9][0-9]*:cpu_silence_ms=[1-9][0-9]*:joined=1:retired=[01]:PASS\]'
# #786 follow-on: the TTBR0 ASID census, emitted before userspace and at every
# process exit. `untagged` counts publishes into `saved_process_cr3`/`next_cr3`
# of a process root whose ASID field is not the userspace ASID -- the word the
# `.Lrestore_saved_ttbr` arm of `syscall_entry.S` installs verbatim. Presence,
# absence-of-untagged and evidence-that-anything-was-counted are pinned
# separately: a census that reached no process-root publish reports untagged=0
# for the same reason a dead counter does.
# claim-lint:ok: 3 of 3 boots of this gate at this head print untagged=0 with
# tagged above 17000, and the raw-operand mutation reddens the sibling gate --
# docs/planning/green-program/aarch64-testing/serials/asid-ratchet/03-strict-x3.txt
# and 02-runtime-anti-vacuity-prod-gate.txt
ASID_CENSUS_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[0-9]+:tagged=[0-9]+:kernel=[0-9]+:cleared=[0-9]+\]'
ASID_CENSUS_UNTAGGED_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[1-9][0-9]*:'
ASID_CENSUS_PUBLISHED_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[0-9]+:tagged=[1-9][0-9]*:'
# Slice 3d: the pinned-placement census. Three assertions rather than one, for
# the reason the ASID block above gives: the line must be present, no line may
# report a field above zero, and the one-shot first-hold marker must be absent
# -- the census is emitted on a period, so a hold after the last emission would
# otherwise be invisible while the marker fires whenever the first one happens.
# A census line is scored by comparing it against the all-zero literal rather
# than by matching each field, so a field added to the line later is gated on
# the day it appears rather than on the day someone remembers to widen a regex.
# claim-lint:ok: 3 of 3 strict boots and 3 of 3 production boots at this head
# read the all-zero literal, and the forced-hold leg reddens this gate --
# docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-x3.txt,
# 02-prod-boot1.txt and its 2 siblings, 05-runtime-anti-vacuity-strict-gate.txt
PINNED_CENSUS_PATTERN='\[PINNED_HOME_CPU_UNAVAILABLE:count=[0-9]+:publish_discarded=[0-9]+:hold_pen_migrated=[0-9]+:delivered=[0-9]+:migration_refused=[0-9]+:stack_home_conflict=[0-9]+\]'
PINNED_CENSUS_ZERO_LITERAL='[PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0:migration_refused=0:stack_home_conflict=0]'
PINNED_FIRST_HOLD_LITERAL='[PINNED_HOME_CPU_UNAVAILABLE:first:'

# Slice 3e's pin-guard oracle. The boot drives 3 of the 11 migration sites the
# slice 3d census enumerated against a thread that carries a per-CPU worker pin
# and reports the CPU each site put it on. Three assertions, for the same reason
# the census has three: a verdict of FAIL is a red, a missing line is a red (a
# kernel that stopped running the oracle must not read as a kernel that passed
# it), and a SKIP is a red too -- the probe declines only on a machine shape
# this gate does not boot, so a SKIP here means the shape changed and the
# oracle stopped measuring.
# The P5b pin census counts this script's occurrences of the INIT_GROUP_WALK
# failing-verdict literal, so this comment deliberately does not spell that
# string: the failing reading of the oracle below is the PASS suffix absent.
# claim-lint:ok: 20 of 20 strict boots at this head print a passing verdict, and
# the unconditional-move mutation of the guard prints a failing one --
# docs/planning/green-program/aarch64-testing/serials/slice3e/
PIN_GUARD_ORACLE_PATTERN='\[PIN_GUARD_ORACLE:aarch64:'
PIN_GUARD_ORACLE_PASS_LITERAL=':verdict=PASS]'

# #766's wake-to-dispatch latency leg, aarch64 arm. The same leg runs on both
# architectures so the two can be read against each other; this arm is a
# regression guard rather than evidence about the x86 mechanism, because
# aarch64 has MAX_CPUS ready queues and a 1 ms tick, and it already passed
# before the #766 fix. What it protects is the fix itself: a change to
# `wake_expired_timers` that regresses dispatch of an already-late wake fails
# here as well as on x86. `bound_ms=100` is a kernel constant; the mechanism
# bound on this arch is 10 ticks * 1 ms + 1 ms = 11 ms, and `quantum_ms` and
# `round_ms` are on the line so the arithmetic can be redone from it.
# `peer_max_gap_ms` is the second reading on the same line: the worst interval a
# CPU-bound peer spent off the CPU while the four re-armers were being promoted
# ahead of it. `peer_gap_bound_ms=560` is a STARVATION ceiling derived in the
# oracle from this arch's quantum ((8 + 4 + 2) * 10 * 4), not a latency
# certification, and on this arch the peers are spread over MAX_CPUS queues.
TIMER_WAKE_LATENCY_ORACLE_PATTERN='\[TIMER_WAKE_LATENCY_ORACLE:aarch64:sleep_ms=10:peers=8:rearmers=4:rearms=32:overrun_ms=[0-9]+:bound_ms=100:quantum_ms=10:round_ms=80:peer_max_gap_ms=[0-9]+:peer_gap_bound_ms=560:wake_enqueues=[1-9][0-9]*:peers_started=8:peers_spinning=8:peers_exited=8:backstops=0:setup_ms=[0-9]+:window_ms=[0-9]+:measured=1:PASS\]'
timer_wake_oracle_sample() {
    printf '[TIMER_WAKE_LATENCY_ORACLE:aarch64:sleep_ms=10:peers=8:rearmers=4:rearms=%s:overrun_ms=%s:bound_ms=100:quantum_ms=10:round_ms=80:peer_max_gap_ms=%s:peer_gap_bound_ms=560:wake_enqueues=32:peers_started=8:peers_spinning=8:peers_exited=8:backstops=0:setup_ms=120:window_ms=430:measured=1:%s]\n' "$1" "$2" "$3" "$4"
}
if timer_wake_oracle_sample 32 2592 40 FAIL | grep -qE "$TIMER_WAKE_LATENCY_ORACLE_PATTERN"; then
    echo "FAIL: TIMER_WAKE_LATENCY_ORACLE_PATTERN accepts a failing verdict at overrun_ms=2592, so this gate would score green on the wake latency it exists to catch"
    exit 1
fi
if ! timer_wake_oracle_sample 32 9 40 PASS | grep -qE "$TIMER_WAKE_LATENCY_ORACLE_PATTERN"; then
    echo "FAIL: TIMER_WAKE_LATENCY_ORACLE_PATTERN rejects overrun_ms=9 at rearms=32, the reading this arm really records, so this gate can never pass"
    exit 1
fi
if timer_wake_oracle_sample 4 9 40 PASS | grep -qE "$TIMER_WAKE_LATENCY_ORACLE_PATTERN"; then
    echo "FAIL: TIMER_WAKE_LATENCY_ORACLE_PATTERN accepts rearms=4, so a leg whose re-armers gave up after one sleep each would score the same as one that completed all 32"
    exit 1
fi

# R157/ASID-01: the scoring-only entry point further down scores a serial that
# was captured earlier, so it needs no kernel, no disk and no preflight. Those
# checks are guarded on this being empty; the scoring rules themselves are not
# guarded at all, which is the point of running them from a test.
SCORE_ONLY_SERIAL="${BREENIX_STRICT_SCORE_ONLY:-}"

if [ -z "$SCORE_ONLY_SERIAL" ]; then

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    # This gate pins boot_tests-only markers (the oracle counters and both strand
    # oracles); a kernel built without the feature fails it spuriously.
    echo "  cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Durable #528 guard: the kernel MUST be soft-float. Fail fast if it was built
# with the NEON hardfloat target (aarch64-breenix.json) — that re-arms #528.
# (set -e aborts the gate if the guard trips.)
#
# This gate is the kernel-merge gate and it was the ONLY aarch64 gate without
# this guard: the full-system, production-profile and service-sequence gates all
# carried it, so a NEON-target kernel could be merged through the one gate that
# decides merges while the others would have caught it. Found while pinning the
# feature-profile guard below, which asserts the two preflights run in order.
"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

# Durable feature-profile guard. This gate pins markers that ONLY a
# `--features boot_tests` kernel emits, so a kernel built in any other profile
# fails every boot on "marker missing" and the run reads as a kernel regression.
#
# `cargo` keeps one cached artifact per feature set and hardlinks the requested
# one into this single output path in about 0.06 s, with no recompilation and no
# output worth reading. ANY `cargo test` in the same session therefore replaces
# this binary silently — `cargo test --test kernel_no_neon_guard` builds the
# kernel with NO features by design — and the next gate boots the wrong kernel.
# Measured: an acceptance battery that ran the structural suites and then this
# gate scored 0/6, every boot on "Futex handoff oracle marker missing or failed",
# against a production kernel that was never asked to emit it. Refuse instead.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of marker literals rather than one sentinel: a single marker
    # changing profile must not be able to disarm this guard quietly.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[CENSUS_WIDEN_ORACLE:' '[FCNTL_PM_CONTENTION_ORACLE:' '[IRQ_HOLD_ORACLE:' '[TTY_IRQ_PM_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[TOMBSTONE_JOIN_ORACLE:' '[TIMER_WAKE_LATENCY_ORACLE:' '[BOOT_TESTS:'; do
        if ! grep -aqF "$marker" "$kernel" 2>/dev/null; then
            missing="$missing $marker"
        fi
    done

    if [ -n "$missing" ]; then
        echo "Error: $kernel was not built with --features boot_tests."
        echo "  Missing boot_tests-only marker literal(s):$missing"
        echo "  This gate pins those markers, so every boot would fail on 'marker missing'."
        echo "  Rebuild with:"
        echo "    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
        echo "  NOTE: any 'cargo test' in this session rebuilds the kernel WITHOUT boot_tests and"
        echo "  silently swaps this binary in a fraction of a second. Build after testing, not before."
        exit 1
    fi
}

require_boot_tests_kernel "$KERNEL"

# Find ext2 disk (required for userspace)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

fi

# Track results
SUCCESSES=0
FAILURES=0
FAILED_ITERATIONS=""

# Check serial output for crash markers. Prints the crash type and returns 0
# if a crash is found, 1 if clean.
check_crash_markers() {
    local serial_file="$1"
    [ -f "$serial_file" ] || return 1
    if grep -qiE "(KERNEL PANIC|panic!)" "$serial_file" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$serial_file" 2>/dev/null; then
        echo "CPU exception"
        return 0
    fi
    if grep -qiE "soft lockup detected" "$serial_file" 2>/dev/null; then
        echo "Soft lockup"
        return 0
    fi
    if grep -qE "\[EXEC_LOCK_ORDER:VIOLATION" "$serial_file" 2>/dev/null; then
        echo "Exec lock-order violation"
        return 0
    fi
    # This profile does not run the boot-test oracle that emits the injected
    # marker, so it pins only the forbidden [CREATION_LOCK_ORDER:VIOLATION:PM_HELD].
    if grep -qE "\[CREATION_LOCK_ORDER:VIOLATION" "$serial_file" 2>/dev/null; then
        echo "Creation lock-order violation"
        return 0
    fi
    if grep -qE "\[EXEC_SMOKE:(EXEC_FAILED|TARGET_ARGV_FAIL|SPAWN_FAILED)" "$serial_file" 2>/dev/null; then
        echo "Exec smoke failure"
        return 0
    fi
    return 1
}

# Score a finished boot ENTIRELY from the serial file it produced.
#
# The poll loop in run_single_test only decides WHEN TO STOP WAITING; it must
# never decide the verdict. Its booleans latch on a grep that runs at most once
# every 1.5s, so a marker that lands between the last grep and the kill is
# present in the file while the boolean is still false — and the old code scored
# that latched false as a boot failure. Everything the gate rejects is rejected
# here, from the file, after QEMU is gone; nothing is loosened.
#
# Prints the failure reason and returns 1 when the boot is unacceptable; prints
# nothing and returns 0 when it is acceptable.
score_serial() {
    local serial_file="$1"
    local boot_test_fail_line
    local crash_type

    if [ ! -f "$serial_file" ]; then
        echo "Userspace not detected"
        return 1
    fi
    if crash_type=$(check_crash_markers "$serial_file"); then
        echo "$crash_type"
        return 1
    fi
    if grep -qF "[BOOT_TESTS:FAIL" "$serial_file" 2>/dev/null \
        || grep -qE '\[TESTS_COMPLETE:[^]]*:FAILED:[1-9][0-9]*\]' "$serial_file" 2>/dev/null; then
        boot_test_fail_line=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' \
            "$serial_file" 2>/dev/null | head -1 || true)
        echo "Boot test failure: ${boot_test_fail_line:-[TEST:<missing>:FAIL:<missing>]}"
        return 1
    fi
    if ! grep -qE "(breenix>|bsh |\[bwm\] Display:|\[bcheck\] Complete:|\[heartbeat\])" \
        "$serial_file" 2>/dev/null; then
        echo "Userspace not detected"
        return 1
    fi
    if ! grep -qF "[EXEC_SMOKE:TARGET_OK]" "$serial_file" 2>/dev/null; then
        echo "Exec smoke did not complete"
        return 1
    fi
    if ! grep -qF "[EXEC_LOCK_ORDER:FIRST_COMMIT]" "$serial_file" 2>/dev/null; then
        echo "Exec commit marker missing"
        return 1
    fi
    if ! grep -qF "[BLOCK_EINTR_ORACLE:" "$serial_file" 2>/dev/null; then
        echo "Block EINTR oracle marker missing"
        return 1
    fi
    if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        echo "Block EINTR oracle reported failure"
        return 1
    fi
    # #568: the blocking-poll-on-connected-TCP oracle. Pinned in the same pair
    # shape as its #575 peer above -- presence, then absence-of-FAIL -- because
    # a marker check alone passes a boot where the program never ran, and a
    # FAIL check alone passes a boot where it was never started. Without both,
    # the oracle's own non-zero exit rides a green gate unnoticed, which is
    # exactly what happened on the first pass at this fix.
    if ! grep -qF "[POLL_TCP_ORACLE:" "$serial_file" 2>/dev/null; then
        echo "Poll TCP oracle marker missing"
        return 1
    fi
    if grep -qF "[POLL_TCP_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        echo "Poll TCP oracle reported failure ($(grep -aoF -m1 "[POLL_TCP_ORACLE:FAIL" "$serial_file"))"
        return 1
    fi
    # #693: the kernel's own contradiction check. `[POLL_TCP_READY_LOST]` is
    # emitted from `sys_poll` when a blocking poll hands back a fd without
    # POLLIN although bytes were published into that connection inside the
    # poll's own window and are still buffered. It does not depend on any
    # userspace program's opinion, so it is pinned separately from the oracle
    # verdict above. `[POLL_TCP_TIMEOUT]` comes out of the same function on
    # each ordinary boot (the oracle's stage 1 and stage 4 are both built to
    # time out) and is required so that "no lost-wake marker" is a reading
    # rather than an assumption about a reporting path that might be dead.
    if ! grep -qF "[POLL_TCP_TIMEOUT]" "$serial_file" 2>/dev/null; then
        echo "Kernel poll timeout report (#693) never emitted"
        return 1
    fi
    if grep -qF "[POLL_TCP_READY_LOST]" "$serial_file" 2>/dev/null; then
        echo "Kernel reported a lost TCP readiness publication (#693): $(grep -aF -m1 "[POLL_TCP_READY_LOST]" "$serial_file")"
        return 1
    fi
    if ! grep -qE "$FUTEX_HANDOFF_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Futex handoff oracle marker missing or failed"
        return 1
    fi
    # FORBIDDEN PATTERNS, scanned over the WHOLE serial, before the presence
    # checks below. The census is cumulative and emitted on a fixed cadence from
    # t≈3s, so every boot that survives three seconds always contains a clean
    # `stranded=0` line; a presence check alone therefore cannot fail on a strand
    # that first appears at t=10s. These two greps are what make ruling (b)'s
    # "hard-fails on stranded>0" true on the kernel-merge gate. They add failure
    # conditions and remove none.
    if grep -qE '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$serial_file" 2>/dev/null; then
        echo "Scheduler strand census reported stranded work ($(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$serial_file" | tail -1))"
        return 1
    fi
    if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" 2>/dev/null; then
        echo "Scheduler strand injection oracle reported stranded work ($(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$SCHED_STRAND_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Scheduler strand census oracle marker missing or failed"
        return 1
    fi
    if ! grep -qE "$STRAND_INJECT_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Scheduler strand injection oracle marker missing or failed"
        return 1
    fi
    if ! grep -qE "$CENSUS_WIDEN_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Census widening mutation oracle marker missing or failed"
        return 1
    fi
    # #766, pinned as a pair for the reason the oracles above are: the FAIL
    # scan names what went wrong even where the pattern check would already
    # have rejected the boot, and it also catches a verdict line whose fields
    # drift out of the pattern for some other reason.
    if grep -qE '\[TIMER_WAKE_LATENCY_ORACLE:[^]]*:FAIL' "$serial_file" 2>/dev/null; then
        echo "Timer wake latency oracle reported failure ($(grep -aoE '\[TIMER_WAKE_LATENCY_ORACLE:[^]]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$TIMER_WAKE_LATENCY_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Timer wake latency oracle marker missing or failed"
        return 1
    fi
    # #796, pinned as a pair: the FAIL scan names what went wrong even when the
    # pattern check would already have rejected the boot, and it also catches a
    # verdict line whose fields drift out of the pattern for some other reason.
    if grep -qF "[FCNTL_PM_CONTENTION_ORACLE:aarch64:" "$serial_file" 2>/dev/null \
        && grep -qE "FCNTL_PM_CONTENTION_ORACLE.*:FAIL(:[a-z_]+)?\]" "$serial_file" 2>/dev/null; then
        echo "fcntl process-manager contention oracle reported failure ($(grep -aoE '\[FCNTL_PM_CONTENTION_ORACLE:[^]]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$FCNTL_PM_CONTENTION_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "fcntl process-manager contention oracle marker missing or failed"
        return 1
    fi
    # #812, pinned as the same pair for the same reason.
    if grep -qF "[IRQ_HOLD_ORACLE:aarch64:" "$serial_file" 2>/dev/null \
        && grep -q "IRQ_HOLD_ORACLE.*:FAIL\]" "$serial_file" 2>/dev/null; then
        echo "IRQ-hold oracle reported failure ($(grep -aoE '\[IRQ_HOLD_ORACLE:[^]]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$IRQ_HOLD_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "IRQ-hold oracle marker missing or failed"
        return 1
    fi
    # #821, pinned as the same pair for the same reason.
    if grep -qF "[TTY_IRQ_PM_ORACLE:aarch64:" "$serial_file" 2>/dev/null \
        && grep -qE "TTY_IRQ_PM_ORACLE.*:FAIL(:[a-z_]+)?\]" "$serial_file" 2>/dev/null; then
        echo "TTY input IRQ process-manager oracle reported failure ($(grep -aoE '\[TTY_IRQ_PM_ORACLE:[^]]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$TTY_IRQ_PM_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "TTY input IRQ process-manager oracle marker missing or failed"
        return 1
    fi
    if ! grep -qF "[INIT_DESIGNATION:aarch64:designated_pid=1:reserved_collisions=0]" "$serial_file" 2>/dev/null; then
        echo "Init designation marker missing"
        return 1
    fi
    if ! grep -qF -x "$INIT_DESIGNATION_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Init designation oracle counter marker missing"
        return 1
    fi
    if ! grep -qF -x "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Init-group refusal oracle counter marker missing"
        return 1
    fi
    if ! grep -qF -x "$TOMBSTONE_JOIN_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Tombstone join oracle marker missing"
        return 1
    fi
    # This gate kills QEMU shortly after exec smoke, so it pins the early probe
    # pair only; the full-system and service-sequence gates pin the quiesce pair.
    if ! grep -qF "[INIT_GROUP_REFUSAL:aarch64:phase=early:probe1=-22:probe2=-22:expected=-22]" "$serial_file" 2>/dev/null; then
        echo "Init-group early refusal marker missing"
        return 1
    fi
    if ! grep -qE '^\[INIT_GROUP_WALK:aarch64:rows=[0-9]+:init_tgid_rows=1:foreign_tgid_rows=0:refused=2:verdict=PASS\]$' "$serial_file" 2>/dev/null; then
        echo "Init-group early walk marker missing"
        return 1
    fi
    if grep -qE '\[INIT_GROUP_WALK:.*verdict=FAIL' "$serial_file" 2>/dev/null; then
        echo "Init-group walk reported failure"
        return 1
    fi
    if grep -qF "[INIT_GROUP_CHILD_RAN]" "$serial_file" 2>/dev/null; then
        echo "Refused init-group child ran"
        return 1
    fi
    if grep -qaE "$ASID_CENSUS_UNTAGGED_PATTERN" "$serial_file" 2>/dev/null; then
        echo "TTBR0 ASID census reported an untagged process-root publish ($(grep -aoE "$ASID_CENSUS_PATTERN" "$serial_file" | grep -E ':untagged=[1-9]' | tail -1))"
        return 1
    fi
    if ! grep -qaE "$ASID_CENSUS_PATTERN" "$serial_file" 2>/dev/null; then
        echo "TTBR0 ASID census marker missing"
        return 1
    fi
    if ! grep -qaE "$ASID_CENSUS_PUBLISHED_PATTERN" "$serial_file" 2>/dev/null; then
        echo "TTBR0 ASID census never counted a process-root publish"
        return 1
    fi
    if grep -aoE "$PINNED_CENSUS_PATTERN" "$serial_file" 2>/dev/null | grep -qvxF "$PINNED_CENSUS_ZERO_LITERAL"; then
        echo "Pinned-placement census reported a field above zero ($(grep -aoE "$PINNED_CENSUS_PATTERN" "$serial_file" | grep -vxF "$PINNED_CENSUS_ZERO_LITERAL" | tail -1))"
        return 1
    fi
    if grep -qaF "$PINNED_FIRST_HOLD_LITERAL" "$serial_file" 2>/dev/null; then
        echo "A pinned worker's wake was held for want of a dispatching home CPU ($(grep -aF -m1 "$PINNED_FIRST_HOLD_LITERAL" "$serial_file"))"
        return 1
    fi
    if ! grep -qaE "$PINNED_CENSUS_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Pinned-placement census marker missing"
        return 1
    fi
    # failure-trace-capture PR-2: TIMER_TICK ring-depth self-check
    # (kernel/src/tracing/providers/irq.rs), fires once ~1s into boot.
    #
    # PR-2 fix round (2026-09-05, PR-2-2026-09-05.md section 9): this used to
    # gate purely on span_ms (the wall-clock reach of CPU 0's live
    # TIMER_TICK entries) against a fixed floor. That floor was wall-clock
    # jitter sensitive rather than a proof the sampling guard was actually
    # gating: widening the sample beyond this PR's original 2-3 boots per
    # configuration measured unsampled (TICK_SAMPLE=1) readings from 0ms to
    # 1120ms and sampled (the fix) readings from 1270ms to 1903ms -- a real,
    # measured ~8% false-pass rate at the old 1100 floor across the combined
    # sample, not the "clean, reproducible gap" originally claimed. The
    # underlying cause: at the boundary where the ring's own wrap timing
    # decides span_ms, ordinary boot-to-boot host jitter can put either
    # configuration on either side, and no amount of floor-tuning removes
    # that -- pushing the checkpoint later shrinks the margin against
    # userspace bring-up traffic swamping the ring instead (see section 2.2).
    #
    # `ticks_total` / `tick_events` (added this round) does not depend on
    # wall-clock timing: `ticks_total` is `TIMER_TICK_TOTAL`'s aggregate,
    # incremented unconditionally on each tick with no sampling applied;
    # `tick_events` is how many TIMER_TICK-typed entries the ring currently
    # holds. Their ratio is a pure count relationship -- it does not care
    # when in the boot this fires or whether the ring has wrapped. Measured
    # on this branch, this configuration (aarch64, -smp 4): sampled
    # (`TICK_SAMPLE=16`, the fix) reads ratios of 34.0-64.4 across 8 boots;
    # unsampled (`TICK_SAMPLE=1`, the mutation) reads ratios of 5.39-5.94
    # across 9 boots -- 8 of 8 fixed-config samples clear
    # RING_SPAN_RATIO_FLOOR below, 9 of 9 mutated-config samples fall below
    # it (PR-2-2026-09-05.md section 9.4).
    #
    # span_ms is kept as a basic liveness sanity check (nonzero, i.e. the
    # ring actually held more than one TIMER_TICK entry when this fired) --
    # informational only, not a threshold; the ratio above is the pass/fail
    # signal, per the 8-of-8 / 9-of-9 result just cited.
    RING_SPAN_RATIO_FLOOR=10
    ring_span_line=$(grep -aoE '\[RING_SPAN:cpu=0:span_ms=[0-9]+:writes=[0-9]+:dropped=[0-9]+:ticks_total=[0-9]+:tick_events=[0-9]+\]' "$serial_file" 2>/dev/null | tail -1 || true)
    if [ -z "$ring_span_line" ]; then
        echo "Ring-span self-check marker missing"
        return 1
    fi
    ring_span_ms=$(echo "$ring_span_line" | sed -n 's/^\[RING_SPAN:cpu=0:span_ms=\([0-9][0-9]*\):.*/\1/p')
    ticks_total=$(echo "$ring_span_line" | sed -n 's/.*:ticks_total=\([0-9][0-9]*\):.*/\1/p')
    tick_events=$(echo "$ring_span_line" | sed -n 's/.*:tick_events=\([0-9][0-9]*\)\].*/\1/p')
    if [ -z "$ring_span_ms" ] || [ "$ring_span_ms" -le 0 ]; then
        echo "Ring-span self-check span_ms ${ring_span_ms:-<unparsed>} is not a positive span ($ring_span_line)"
        return 1
    fi
    if [ -z "$ticks_total" ] || [ -z "$tick_events" ] || [ "$tick_events" -le 0 ]; then
        echo "Ring-span self-check ticks_total/tick_events unparsed or zero ($ring_span_line)"
        return 1
    fi
    if [ "$ticks_total" -lt "$((tick_events * RING_SPAN_RATIO_FLOOR))" ]; then
        echo "Ring-span self-check sampling ratio ticks_total=$ticks_total / tick_events=$tick_events below floor $RING_SPAN_RATIO_FLOOR ($ring_span_line)"
        return 1
    fi
    if ! grep -qaE "$PIN_GUARD_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Pin-guard oracle line missing"
        return 1
    fi
    if ! grep -aE "$PIN_GUARD_ORACLE_PATTERN" "$serial_file" 2>/dev/null | grep -qF "$PIN_GUARD_ORACLE_PASS_LITERAL"; then
        echo "Pin-guard oracle did not pass ($(grep -aE "$PIN_GUARD_ORACLE_PATTERN" "$serial_file" | tail -1))"
        return 1
    fi
    return 0
}

# Scoring-only entry point: score an already-captured serial log and exit. This
# exists so the scoring rules can be exercised against a preserved serial without
# booting, which is how the "a serial containing every success marker scores as a
# success" property is proven.
if [ -n "$SCORE_ONLY_SERIAL" ]; then
    if [ ! -f "$SCORE_ONLY_SERIAL" ]; then
        echo "SCORE: FAIL - BREENIX_STRICT_SCORE_ONLY names no readable serial ($SCORE_ONLY_SERIAL)"
        exit 1
    fi
    if SCORE_REASON=$(score_serial "$SCORE_ONLY_SERIAL"); then
        echo "SCORE: PASS - $SCORE_ONLY_SERIAL"
        exit 0
    else
        echo "SCORE: FAIL - $SCORE_REASON ($SCORE_ONLY_SERIAL)"
        exit 1
    fi
fi

report_failure() {
    local iteration="$1"
    local reason="$2"
    local serial_file="$3"
    local facts_line="$4"
    local failure_dir="$BREENIX_GATE_TMP/breenix_aarch64_strict_failures"
    local timestamp
    local preserved_serial
    local lines

    mkdir -p "$failure_dir"
    timestamp=$(date -u +%Y%m%dT%H%M%SZ)
    preserved_serial="$failure_dir/${timestamp}-boot${iteration}.txt"
    if [ -f "$serial_file" ]; then
        cp "$serial_file" "$preserved_serial"
        lines=$(wc -l < "$serial_file" 2>/dev/null | tr -d ' ' || echo 0)
    else
        # QEMU never opened the serial file: preserve the empty artifact anyway so
        # "zero serial bytes" (the #569 silent-hang signature) is on the record.
        : > "$preserved_serial"
        lines=0
    fi
    # #827: the facts line is preserved alongside the serial it describes,
    # not only echoed to the console, so a failed boot's host-side reading
    # is as durable as its serial capture.
    printf '%s\n' "$facts_line" > "$failure_dir/${timestamp}-boot${iteration}.facts.txt"
    echo "  [FAIL] Boot $iteration: $reason ($lines lines); serial: $preserved_serial"
    echo "  $facts_line"
}

run_single_test() {
    local iteration=$1
    local OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_strict_$iteration"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 20s timeout.
    # Breenix ARM64 expects a GICv3 CPU interface, matching Parallels.
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
    qemu_host_lock_acquire
    # #827: "start" is sampled here, right after the lock is held and right
    # before QEMU is launched -- this is when THIS boot's own wall-clock
    # window actually begins, so time spent blocked on the host lock is not
    # folded into the window the guest-uptime ratio is measured against.
    local HOST_MS_START QEMU_AT_START LOAD_AT_START
    HOST_MS_START="$(gbf_host_ms_now)"
    QEMU_AT_START="$(qemu_host_lock_count)"
    LOAD_AT_START="$(gbf_load_1m)"
    timeout 20 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" &
    local QEMU_PID=$!
    # F2: registers this PID with the lock's own EXIT trap so a
    # SIGTERM/SIGINT delivered to just this process during the poll below
    # still kills QEMU instead of orphaning it with the lock free.
    qemu_host_lock_track_pid "$QEMU_PID"

    # Wait for userspace liveness AND exec smoke completion (18s max, checking every 1.5s)
    # Accept any of these as the liveness condition:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    #   "[bcheck] Complete:" - bcheck self-test suite finished (headless/no-VirGL mode)
    #   "[heartbeat]" - the default ARM64 init service executed in userspace
    # Also require "[EXEC_SMOKE:TARGET_OK]" as the exec completion condition.
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local CRASH_TYPE=""
    # Named POLL, not i: the caller's loop variable is also i, and an unscoped
    # inner i made the summary report the poll counter instead of the boot number.
    local POLL
    #
    # THE STOP CONDITION IS score_serial ITSELF, not a narrower pair of liveness
    # patterns.
    #
    # This loop used to break as soon as a userspace liveness pattern and
    # [EXEC_SMOKE:TARGET_OK] were both present, kill QEMU, and only then score the
    # serial. Those two land at roughly 0.5 s and 4.4 s of uptime; every other
    # marker score_serial requires — the futex handoff oracle (~5.8 s), the block
    # EINTR oracle (~5.8 s), the strand census and the strand injection oracle —
    # is emitted afterwards. The gate therefore killed the VM before the evidence
    # it scores could exist and failed every boot on "marker missing", including
    # on main: a stop condition narrower than the scoring criteria is a gate that
    # cannot pass. It also made the forbidden-pattern scans below unreachable — a
    # late strand cannot appear in a serial that was truncated at 4.4 s.
    #
    # Polling score_serial keeps the two in sync by construction: whatever the
    # scoring criteria grow to require, the loop waits for it. This only ever
    # extends the capture window. It accepts nothing score_serial would reject —
    # the verdict below is still a fresh score of the serial QEMU left behind —
    # and the crash-marker break and the wall-clock bound are unchanged.
    for POLL in $(seq 1 12); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
            if score_serial "$OUTPUT_DIR/serial.txt" >/dev/null 2>&1; then
                break
            fi
        fi
        sleep 1.5
    done

    # #827: reclassifies which of the loop's own two guarded breaks
    # explains why polling stopped, using the SAME two predicates the loop
    # above uses (check_crash_markers, score_serial), evaluated once more
    # against the file's state right as the loop exits. This is not a new
    # stop condition -- the loop body above is byte-for-byte what it was
    # before this file existed -- it is a read taken immediately after the
    # loop already stopped, naming which guarded branch a moment ago is
    # still true right now. Left empty when neither is true, i.e. the loop
    # simply ran out its 12 iterations without either break firing.
    local ENDED_BY_LOOP=""
    if [ -f "$OUTPUT_DIR/serial.txt" ] && check_crash_markers "$OUTPUT_DIR/serial.txt" >/dev/null 2>&1; then
        ENDED_BY_LOOP="crash"
    elif [ -f "$OUTPUT_DIR/serial.txt" ] && score_serial "$OUTPUT_DIR/serial.txt" >/dev/null 2>&1; then
        ENDED_BY_LOOP="early_pass"
    fi

    # #827: sampled together, immediately before this boot's own kill --
    # ps has no output for a PID already gone, so qemu_cpu_seconds and the
    # aliveness check below must both run before the kill line, not after.
    local HOST_MS_END QEMU_AT_END LOAD_AT_END QEMU_CPU_S
    HOST_MS_END="$(gbf_host_ms_now)"
    QEMU_AT_END="$(qemu_host_lock_count)"
    LOAD_AT_END="$(gbf_load_1m)"
    # #827: $QEMU_PID is `timeout`'s own pid (see gate-boot-facts.sh's
    # own header for why); resolve to the actual qemu-system-aarch64
    # child before reading its CPU time.
    local QEMU_ACTUAL_PID
    QEMU_ACTUAL_PID="$(gbf_resolve_qemu_pid "$QEMU_PID")"
    QEMU_CPU_S="$(gbf_qemu_cpu_seconds "$QEMU_ACTUAL_PID")"
    local QEMU_STILL_ALIVE=1
    kill -0 "$QEMU_PID" 2>/dev/null || QEMU_STILL_ALIVE=0

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true
    qemu_host_lock_release

    # The poll booleans above are a stop condition, not a verdict. Score the boot
    # from the serial file QEMU actually left behind.
    local FAIL_DETAIL
    local SCORE_PASS=0
    if FAIL_DETAIL=$(score_serial "$OUTPUT_DIR/serial.txt"); then
        SCORE_PASS=1
    fi

    # #827: ended_by names which bound in the loop above actually ended this
    # boot, derived from the same control flow the scoring above already
    # ran -- no new stop condition, no changed deadline.
    #   crash_marker    -- the crash-marker break fired (ENDED_BY_LOOP=crash)
    #   scored_pass     -- either the score_serial break fired
    #                      (ENDED_BY_LOOP=early_pass) and the post-kill
    #                      rescore still agrees, OR neither break fired
    #                      (ENDED_BY_LOOP empty, so the loop's own 12
    #                      iterations ran out or QEMU died on its own) but
    #                      the guest's last required marker landed in the
    #                      gap between this function's pre-kill sampling
    #                      calls and the kill+rescore, so the post-kill
    #                      rescore is a PASS anyway -- the mirror of the
    #                      scored_fail race below, resolved the same way:
    #                      the post-kill rescore, not the loop's now-stale
    #                      classification, wins: the case block right below
    #                      checks SCORE_PASS before it falls through to
    #                      poll_exhausted/hard_timeout, so a PASS rescore
    #                      lands on scored_pass, matching the SUCCESS
    #                      verdict printed a few lines down
    #   scored_fail     -- the score_serial break fired, but content written
    #                      between that grep and the kill (e.g. a late strand)
    #                      flipped the post-kill rescore to FAIL
    #   poll_exhausted  -- neither break fired, the post-kill rescore is
    #                      also not a PASS, and QEMU was still alive when
    #                      the loop's own 12 iterations ran out (this
    #                      script's own kill ends it)
    #   hard_timeout    -- neither break fired, the post-kill rescore is
    #                      also not a PASS, and QEMU was already dead when
    #                      this point was reached -- the `timeout 20`
    #                      wrapping the launch line above fired first
    local ENDED_BY
    case "$ENDED_BY_LOOP" in
        crash)
            ENDED_BY="crash_marker"
            ;;
        early_pass)
            if [ "$SCORE_PASS" = "1" ]; then
                ENDED_BY="scored_pass"
            else
                ENDED_BY="scored_fail"
            fi
            ;;
        *)
            if [ "$SCORE_PASS" = "1" ]; then
                ENDED_BY="scored_pass"
            elif [ "$QEMU_STILL_ALIVE" = "1" ]; then
                ENDED_BY="poll_exhausted"
            else
                ENDED_BY="hard_timeout"
            fi
            ;;
    esac

    local GUEST_UPTIME_MS
    GUEST_UPTIME_MS="$(gbf_last_heartbeat_uptime_ms "$OUTPUT_DIR/serial.txt")"

    local FACTS_LINE
    FACTS_LINE="$(gbf_emit_line "$iteration" "$HOST_MS_START" "$HOST_MS_END" \
        "$QEMU_AT_START" "$LOAD_AT_START" "$QEMU_AT_END" "$LOAD_AT_END" \
        "$QEMU_CPU_S" "$GUEST_UPTIME_MS" "$ENDED_BY")"
    # Recorded into this boot's own evidence directory unconditionally --
    # not only on failure, so a passing boot's host-side reading is on the
    # record too.
    printf '%s\n' "$FACTS_LINE" > "$OUTPUT_DIR/gate_boot_facts.txt"

    if [ "$SCORE_PASS" = "1" ]; then
        echo "  [OK] Boot $iteration: SUCCESS"
        echo "  $FACTS_LINE"
        return 0
    fi

    report_failure "$iteration" "$FAIL_DETAIL" "$OUTPUT_DIR/serial.txt" "$FACTS_LINE"
    return 1
}

echo "========================================="
echo "ARM64 Strict Boot Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Iterations: $ITERATIONS"
echo "Requirement: 100% success rate (all $ITERATIONS must pass)"
echo ""
echo "Running tests..."
echo ""

START_TIME=$(date +%s)

for i in $(seq 1 $ITERATIONS); do
    if run_single_test $i; then
        SUCCESSES=$((SUCCESSES + 1))
    else
        FAILURES=$((FAILURES + 1))
        FAILED_ITERATIONS="$FAILED_ITERATIONS $i"
    fi
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "========================================="
echo "RESULTS"
echo "========================================="
echo "Total iterations: $ITERATIONS"
echo "Successes: $SUCCESSES"
echo "Failures: $FAILURES"
echo "Success rate: $(( (SUCCESSES * 100) / ITERATIONS ))%"
echo "Duration: ${DURATION}s"

if [ $FAILURES -eq 0 ]; then
    echo ""
    echo "========================================="
    echo "PASS: $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    exit 0
else
    echo ""
    echo "Failed iterations:$FAILED_ITERATIONS"
    echo ""
    echo "========================================="
    echo "FAIL: Only $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    echo ""
    echo "This indicates a regression or timing bug that needs investigation."
    echo "Serial output from failed boots can be found in $BREENIX_GATE_TMP/breenix_aarch64_strict_N/"
    exit 1
fi
