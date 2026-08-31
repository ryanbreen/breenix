#!/bin/bash
# x86_64 PRODUCTION-PROFILE boot and teardown-census gate (#540).
#
# WHAT THIS GATE IS FOR
#
# Every x86 counter, census and teardown claim this project has ever made was
# measured by docker/qemu/run-x86-boot-tests.sh, which hard-codes
# `--features boot_tests,testing,external_test_bins`. aarch64 has had a
# zero-feature counterpart (run-aarch64-prod-profile-boot-test.sh) for a while;
# x86 had none, so no x86 boot had ever executed the kernel that actually ships.
# That gap is #540. This script closes it: it builds `qemu-uefi` with NO
# `--features` flag at all and asserts against the shipped profile.
#
# The absence of --features is the point. Neither Cargo.toml nor kernel/Cargo.toml
# declares a `default` feature and every test feature is additive, so omitting the
# flag is a real production build rather than a differently-configured test build.
# The profile-fidelity block below is the negative control that proves it: a dozen
# markers that only a boot_tests/testing kernel can emit are asserted absent, so an
# accidental rebuild of the test kernel reddens this gate instead of passing it.
#
# WHAT THE SHIPPED x86 PROFILE ACTUALLY DOES (measured, not assumed)
#
# A zero-feature x86 boot initialises memory/GDT/IDT/PCI/VirtIO, mounts the ext2
# root, emits the root-custody and tombstone censuses once from
# kernel/src/main.rs (that call site is OUTSIDE the `boot_tests` cfg block above
# it, which is why no kernel change is needed for this gate), finishes kernel
# init, and drops into the async executor running the keyboard and serial-command
# tasks. That is its steady state, and this gate polls for it.
#
# WHAT IT NOW DOES (#673, fixed)
#
# The shipped x86_64 kernel now launches a real init process. kernel_main_continue()
# (kernel/src/main.rs) gained a third block, mutually exclusive with the existing
# `testing`/`interactive` blocks: `#[cfg(not(any(feature = "testing", feature =
# "interactive", feature = "disable_x86_prod_init")))]`. It reads `/sbin/init` from
# the already-mounted ext2 root and drives it through the exact same arch-neutral
# ProcessManager transaction aarch64's `launch_init_from_elf()` uses --
# `create_init_process` -> `designate_init` -> `publish_init` (all in
# kernel/src/process/manager.rs, no target_arch cfg) -- then hands the published
# thread to `task::scheduler::spawn()`, the ordinary x86_64 dispatch entry point
# every other process on this arch already uses. That single call was the gap:
# `publish_init()` already left the thread scheduler-ready, but nothing on x86 had
# ever called `spawn()` on it before #673.
#
# `disable_x86_prod_init` is an anti-vacuity knob only (not meant to ship enabled):
# building with `--features disable_x86_prod_init` and neither `testing` nor
# `interactive` compiles ONLY the init-launch block back out (#673 review, M3 --
# not "byte-for-byte" the pre-fix kernel: the rest of this branch's fixes stay
# in place, including the scheduler blocked-state fix, the timer TOCTOU fix, and
# the console-executor kthread). It reproduces the one property this gate's
# anti-vacuity leg needs: a kernel that never constructs a userspace process, so
# this same gate can be run against it to prove the assertions below actually
# discriminate the fix rather than passing either way. Set
# X86_PROD_PROFILE_EXTRA_FEATURES=disable_x86_prod_init before invoking this
# script to run that leg.
#
# CENSUS SCOPE, UNCHANGED BY THE FIX
#
# `emit_root_custody_summary()`/`emit_tombstone_census()` (kernel/src/main.rs:676-677)
# still run exactly where they always did: inside `kernel_main`, strictly BEFORE
# `kernel_main_continue()` is ever called, and therefore strictly before the new
# init-launch block above. The pinned all-zero census literals below are therefore
# still a true "before any user process exists" baseline -- the fix does not move
# that emission point and does not require re-deriving those literals. What remains
# true, and was true before #673 too, is that this gate does not drive init through
# a reap/retire cycle, so it proves the census is at rest at its own emission point,
# not a return-to-zero after a teardown: nothing here asserts anything about
# teardown behavior once init is actually running its own workload.
#
# NEW EVIDENCE THIS GATE REQUIRES (#673)
#
# Construction is not dispatch, dispatch is not execution, and execution is not
# survival, so four independent signals are asserted, each proving a strictly
# stronger claim than the last:
#   1. `[INIT_DESIGNATION:x86_64:designated_pid=1:...]` (emitted from the new call
#      site) -- proves the ProcessManager transaction ran and named PID 1 as init.
#   2. PRECONDITION 5 ("Scheduler has runnable threads") flips from its old,
#      attributed FAIL to PASS -- proves the thread reached the scheduler's
#      runnable set, checked live via `task::scheduler::with_scheduler` at
#      kernel/src/main.rs:1970 (was main.rs:1798-1807 pre-fix, per #673's
#      original RCA -- the fix's own new block shifted every line after it; re-
#      derived again for the #673 review's M4 finding).
#   3. `RING3_SYSCALL: First syscall from userspace` present exactly once --
#      proves init's userspace code actually ran and reached the syscall
#      handler. This is a pre-existing, one-time marker
#      (kernel/src/syscall/handler.rs's emit_ring3_syscall_marker(), raw
#      serial output, no locks) that already exists for the test framework's
#      own stage-advance bookkeeping; #673 does not add it, only asserts on
#      it in a profile that had never reached Ring 3 before.
#   4. init's own first line, followed by the bsshd-launch warning it prints
#      immediately afterward -- proves init ran past its first print into
#      start_bsshd()'s spawn attempt and handled the result (#673 review,
#      M6/MA4). This is deliberately NOT proof init reached its steady-state
#      reap loop: SPAWN is unconditionally ENOSYS on x86 today (#713), so the
#      warning fires on the very next lines of init's own code, not after
#      further progress. The signal this pin replaced -- "init was never
#      reported killed by signal" -- could never fire either way (it is
#      init.rs's reaped-CHILD message; PID 1 never reaps itself) and is
#      removed rather than kept as an unfalsifiable pin. See "INIT SURVIVAL
#      EVIDENCE" below for exactly what is and is not proven, and why this
#      does NOT also pin bsshd reaching its listening state, unlike the
#      aarch64 production gate.
#
# WHY INIT CANNOT RUN UNTIL BOOT'S OWN WORK IS DONE (#673, the real fight)
#
# The straightforward version of this fix -- spawn init, let it compete --
# reliably stalled the boot thread forever partway through its own remaining
# work. Root cause: main.rs's init_with_current() makes the boot thread
# BECOME the scheduler's idle thread ("Linux where the boot thread becomes
# the idle task"). The moment any thread's first syscall is confirmed,
# syscall::handler::is_ring3_confirmed() latches true, and
# interrupts/context_switch.rs permanently stops restoring idle's saved
# boot context whenever idle is next selected -- by design, to avoid
# resuming stale boot-time RIPs. Separately, schedule() never re-enqueues
# the outgoing thread when it IS the idle thread (correct for genuine idle).
# Combined: once init exists as a thread that keeps re-readying itself (its
# infinite reap loop nanosleep()s between waitpid attempts), the ready queue
# is never truly empty, idle is never selected as the last-resort fallback,
# and boot's own remaining PRECONDITION-checking/timer/census work -- tagged
# as "idle" -- can never resume. The fix (kernel/src/main.rs) is a scheduling
# brake taken unconditionally before init is even read from disk and released
# unconditionally at a single matching site, once every line of boot's own
# sequential work is behind it. This gate's evidence signals 1-3 above are
# what proves that brake is correctly placed: construction, then dispatch,
# then execution, in that order, with the shipped profile still reaching
# steady state afterward.
#
# INTERRUPT BOOT-ORDER CHANGE (#673 review, m1/MA5)
#
# The brake above is taken alongside a new `interrupts::enable()` call that
# moves hardware interrupt-enable to before this block, in every profile that
# takes it (including the ext2-read-failure path, since the enable is
# unconditional and outside the `match` arms). Before #673, the shipped
# production profile's first hardware interrupt-enable was several hundred
# lines later, immediately before the executor starts; every PRECONDITION
# check, `int3`, and the timer/clock_gettime test between here and there now
# run with interrupts genuinely enabled for the first time in production,
# matching what the `testing` profile has always done -- this is what
# surfaced time_test.rs's TOCTOU (m5).
#
# WHY THE CONSOLE SURVIVES INIT (#673 review, B1 -- a second, independent fight)
#
# The brake above only protects BOOT's own remaining work; it says nothing
# about what happens to the async executor (keyboard + serial console) once
# that brake lifts. The straightforward version of THAT -- run the executor
# from the boot thread's own tail, exactly like every other x86 profile --
# reliably loses the console within about a second of init starting: the
# instant init's first syscall lands, idle's saved context (which is what
# `executor.run()` would have to resume from) becomes exactly the kind of
# stale boot-time context the paragraph above says is permanently abandoned,
# and nothing else ever polls the executor again -- Waker::wake() only
# re-queues a task id, and Executor::run()'s own loop is the only thing that
# ever drains that queue. The fix is a SEPARATE mechanism from the brake
# above: the console executor runs in its own dedicated kernel thread
# (`task::kthread::kthread_run`, kernel/src/main.rs), spawned before the
# brake lifts. A kthread is never the scheduler's idle thread, so it never
# falls into the "abandon this context" rule at all -- it is preserved by the
# same ordinary dispatch path every other kernel thread uses, indefinitely.
# STEADY_STATE_LITERAL and the liveness check further below are what prove
# this: they can only pass if the console is still being serviced well after
# init has been running (see the LIVENESS section for exactly how).
#
# DISK LAYOUT
#
# kernel/src/fs/ext2/mod.rs's init_root_fs() looks for VirtIO block device index
# 2 (falling back to index 0), so the ext2 root only mounts when it is the third
# virtio-blk device -- the layout src/bin/qemu-uefi.rs builds. Production carries
# no test-binaries disk, so index 1 here is a zero-filled placeholder rather than
# target/test_binaries.img: that keeps the layout faithful while proving the
# shipped kernel needs no test artifacts to reach its steady state.
#
# VERDICT DISCIPLINE (#668)
#
# Every assertion below is a `test` under `set -e`, and `set -E` + the ERR trap
# make each of them loud: a silent `set -e` abort prints nothing of its own, so a
# genuine red used to die with no verdict text and no serial pointer. The trap
# fires on every uncaught nonzero exit, names the failing command and line, tails
# the serial, preserves it in a timestamped directory, and re-raises the same
# nonzero status.

set -euo pipefail
# errtrace: without this the ERR trap is not inherited into shell functions, and
# report_gate_failure is itself invoked from that trap.
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="/tmp/breenix_x86_prod_profile"
QEMU_PID=""
# #673 anti-vacuity knob. Empty (default) builds the real shipped profile;
# set to "disable_x86_prod_init" to build the pre-fix, zero-userspace kernel
# and confirm this same gate discriminates the fix (see the header).
X86_PROD_PROFILE_EXTRA_FEATURES="${X86_PROD_PROFILE_EXTRA_FEATURES:-}"

# ---------------------------------------------------------------------------
# Production milestones. Each must appear exactly once. `Kernel initialization
# complete!` and the executor/serial-task pair are the shipped profile's own
# progress markers, not test-runner markers -- deliberately so, because a marker
# only a test profile emits would make this gate impossible to satisfy in the
# profile it exists to measure.
# ---------------------------------------------------------------------------
EXT2_ROOT_LITERAL='ext2 root filesystem mounted'
KERNEL_INIT_LITERAL='Kernel initialization complete!'
EXECUTOR_LITERAL='Starting async executor...'
# Steady state: the last line the production boot prints before the executor
# parks on the keyboard and serial streams. The poll loop waits for this.
STEADY_STATE_LITERAL='Serial command task started'
CONSOLE_PROMPT_LITERAL='breenix> '

# ---------------------------------------------------------------------------
# The teardown census, read in the shipped profile. Both lines are emitted once
# from kernel/src/main.rs outside every test cfg. The *_PREFIX counts are what
# stop a drifted-value line from hiding: pinning only the all-zero literal would
# pass a boot that emitted a nonzero census as well, and pinning only the prefix
# would pass any values at all.
#
# The x86 serial console carries the scheduler's raw single-character trace
# stream on the same port, so these lines can carry a prefix ("[SW]<K>..."):
# every match here is a substring match and must stay one.
# ---------------------------------------------------------------------------
TOMBSTONE_CENSUS_PREFIX='[TOMBSTONE_CENSUS:'
TOMBSTONE_CENSUS_PROD_LITERAL='[TOMBSTONE_CENSUS:resident=0:removed=0:reap_second=0:retire_second=0:abandoned_unqueued=0]'
ROOT_CUSTODY_PREFIX='[PT_ROOT_CUSTODY:'
ROOT_CUSTODY_PROD_LITERAL='[PT_ROOT_CUSTODY:no_proof=0:no_arch=0:terminated=0:undecided=0:mid_retire=0:retired=0]'

# ---------------------------------------------------------------------------
# Profile fidelity. Every literal here can only be emitted by a kernel built
# with boot_tests/testing/external_test_bins. One of them appearing means this
# gate measured the wrong kernel, which is the single failure mode that would
# make everything else it asserts meaningless.
#
# RING3_SMOKE: creating (not bare RING3_SMOKE:, #673) -- context_switch.rs
# emits an unconditional, once-per-boot "[ OK ] RING3_SMOKE: userspace
# executed + syscall path verified" canary on the FIRST real transition to
# Ring 3 in ANY profile (raw_serial_str, no cfg at all -- it exists so CI can
# verify userspace ran regardless of build). Before #673 that canary could
# never fire in production because no process ever reached Ring 3, which
# masked the bare RING3_SMOKE: substring here being wrong: it was only ever
# a valid test-only signal by accident of the OTHER defect, not because it is
# actually behind a testing cfg. #673 is the first thing that makes a
# production boot exercise that canary, and it does -- correctly -- so this
# entry is narrowed to RING3_SMOKE: creating, the prefix every genuinely
# testing-gated RING3_SMOKE print in main.rs shares and the canary does not.
# ---------------------------------------------------------------------------
TEST_ONLY_MARKERS=(
    'TEST_TALLY:'
    '[TEST:'
    'TEST RUNNER:'
    '[BOOT_TESTS:'
    'RING3_SMOKE: creating'
    '[TOMBSTONE_QUIESCE:'
    '[RECLAIM_DRAIN:'
    '[TOMBSTONE_JOIN_ORACLE:'
    '[KSTACK_OWNER_ORACLE:'
    '[KSTACK_QUIESCE_LEAK:'
    '[PT_CUSTODY_COUNTERS:'
    '[FRAME_CUSTODY_COUNTERS:'
    '[PT_RETIRE_COHORT:'
    '[PT_EXEC_COHORT:'
    '[EXEC_DETACH_ORACLE:'
    '[CLONE_ADMISSION_ORACLE:'
    '[INIT_DESIGNATION_ORACLE:'
    '[SCHED_STRAND_ORACLE:'
    '[CENSUS_WIDEN_ORACLE:'
    'Testing features enabled'
)

# ---------------------------------------------------------------------------
# Fault markers the shipped kernel can emit from unconditional code. These are
# not test scaffolding: a production boot printing any of them is a real defect
# report, so they are asserted at zero.
# ---------------------------------------------------------------------------
FAULT_MARKERS=(
    '[PMGUARD]'
    '[CREATION_LOCK_ORDER:VIOLATION'
    'DISK LOADING FAILED'
)
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected'
# ---------------------------------------------------------------------------
# PRECONDITION pins. R52 forbids an unattributed failure, not a disclosed one
# left unfixed forever: both entries that used to live here (#672, #673) are
# now FIXED, and both pins are re-derived in the direction that proves it --
# the FAIL line must be ABSENT and the PASS line present exactly once. A
# kernel that simply stopped emitting the check would satisfy a bare absence
# assertion, which is why the PASS line is pinned too, not just the FAIL
# line's absence.
#
# PRECONDITION 5 (#673, fixed) -- "Scheduler has runnable threads". The
# shipped x86 kernel launched no userspace process at all; kernel_main_continue()
# now reads /sbin/init from ext2 and hands it to task::scheduler::spawn() before
# this check runs (see the header). Do not relax the FAIL-absent / PASS-present
# pair back to a bare presence check or delete it -- that is how a fixed defect
# regresses silently.
#
# PRECONDITION 7 (#672, fixed) -- "Preemption disabled". kernel_main_continue()
# used to call per_cpu::preempt_disable() only inside `#[cfg(all(feature =
# "testing", not(feature = "interactive")))]` while calling the matching
# preempt_enable() unconditionally, so the shipped profile decremented a
# preempt_count that was already zero and the bare `sub dword ptr gs:[..], 1`
# wrapped it to 0xFFFFFFFF. The disable is now unconditional, so the bracket is
# symmetric in every build profile.
# ---------------------------------------------------------------------------
PRECOND_RUNNABLE_FAIL_LITERAL='PRECONDITION 5: Scheduler has runnable threads ✗ FAIL'
PRECOND_RUNNABLE_PASS_LITERAL='PRECONDITION 5: Scheduler has runnable threads ✓ PASS'
PRECOND_PREEMPT_FAIL_LITERAL='PRECONDITION 7: Preemption disabled ✗ FAIL'
PRECOND_PREEMPT_PASS_LITERAL='PRECONDITION 7: Preemption disabled ✓ PASS'

# ---------------------------------------------------------------------------
# The preempt-bracket census (#672), emitted once per boot from
# kernel/src/main.rs on the way to the executor, outside every test cfg. A
# nonzero value means some path called preempt_enable() without a matching
# preempt_disable(); per_cpu::preempt_enable() saturates at zero and counts the
# violation instead of wrapping the count. Prefix and all-zero literal are both
# pinned for the same reason as the teardown censuses above: the prefix alone
# would pass a drifted value, the literal alone would pass a boot that also
# emitted a nonzero line.
# ---------------------------------------------------------------------------
PREEMPT_CENSUS_PREFIX='[PREEMPT_BRACKET_CENSUS:'
PREEMPT_CENSUS_PROD_LITERAL='[PREEMPT_BRACKET_CENSUS:underflow=0]'
# #673 review, mi5: the census above is emitted before the production
# block's OWN preempt_enable() (kernel/src/main.rs, B3's release) runs, so
# it cannot see an underflow caused by that specific release. This second
# marker is emitted immediately after it -- the only point in this profile
# reached after every preempt_enable() call in it has executed.
PROD_BRACKET_RELEASE_PREFIX='[PROD_BRACKET_RELEASE_CENSUS:'
PROD_BRACKET_RELEASE_PROD_LITERAL='[PROD_BRACKET_RELEASE_CENSUS:underflow=0]'

# ---------------------------------------------------------------------------
# #673 new evidence. Four independent signals, each a strictly stronger claim
# than the last (construction, dispatch, execution, survival) -- see the
# header's "NEW EVIDENCE THIS GATE REQUIRES" section for the full rationale.
# INIT_DESIGNATION is matched by prefix (not full literal) because
# reserved_collisions is a live counter, not a fixed value; designated_pid=1
# is the part that must never drift.
# ---------------------------------------------------------------------------
INIT_DESIGNATION_X86_PREFIX='[INIT_DESIGNATION:x86_64:designated_pid=1:'
RING3_SYSCALL_LITERAL='RING3_SYSCALL: First syscall from userspace'

# ---------------------------------------------------------------------------
# #673 review, M6/MA4: proves init did not just start but ran past its own
# startup sequence. INIT_FIRST_LINE is init.rs's own first print (the exact
# literal, not a prefix, since pid=1 is fixed for the singleton designated
# init this profile constructs).
#
# The pin this replaced (INIT_KILLED_PREFIX, absent) was structurally
# vacuous: it matched init.rs's waitpid(-1) reap-loop message for a REAPED
# CHILD, printed with the CHILD's own pid. PID 1 is init itself, and init
# never reaps itself via its own waitpid(-1) call, so that message could
# never be emitted whatever init did -- the assertion could not fail
# regardless of whether init actually survived. It is removed rather than
# kept as an unfalsifiable pin.
#
# INIT SURVIVAL EVIDENCE (why this does NOT also pin bsshd, unlike the
# aarch64 production gate): checked before pinning, per the #673 spec's own
# risk note about init.rs's spawn chain on a lean production disk. init's
# x86 main() calls start_bsshd() unconditionally right after its own
# startup print, exactly like aarch64 -- but on x86, both that call and the
# following run_boot_script() spawn fail cleanly with ENOSYS:
#   [init] Warning: failed to start bsshd
#   [init] Failed to spawn boot script: ENOSYS
# Root cause (pre-existing, NOT a #673 regression, filed as #713): the
# SPAWN syscall (nr 440) is unconditionally stubbed to ENOSYS in
# kernel/src/syscall/handler.rs on x86_64 -- it has a real implementation
# on aarch64 (sys_spawn_aarch64) but has never been ported to x86, because
# #673 is the first x86 build to ever run userspace code that calls it.
# The good news, and the reason init.rs's spawn chain was safe to run here
# at all: it degrades gracefully exactly as the spec required checking --
# init falls through cleanly into its reap loop rather than hanging. But
# no x86 child process can ever start today, so bsshd can never reach
# "listening" on this architecture, and pinning it would make this gate
# permanently unsatisfiable. Revisit once #713 is fixed.
#
# INIT_BSSHD_WARNING_LITERAL replaces INIT_KILLED_PREFIX as the actual
# survival pin (#673 review, MA4): it is the first line init prints AFTER
# its own startup print that it could only reach by running its own
# subsequent code (attempting the bsshd spawn and handling the Err arm), so
# unlike the marker it replaces, it CAN fail -- an init that hung, faulted,
# or never called start_bsshd() would not print it. It does not by itself
# prove init reached its reap loop (run_boot_script() and the loop both
# come later in init's own control flow) -- only that init progressed past
# its first line into code whose behavior depends on #713's real, still-
# open gap.
# ---------------------------------------------------------------------------
INIT_FIRST_LINE_LITERAL='[init] Breenix init starting (PID 1)'
INIT_BSSHD_WARNING_LITERAL='[init] Warning: failed to start bsshd'

# Measured on beast under TCG: steady state at 14s from QEMU launch. The bound is
# an order of magnitude above that so host contention cannot score a slow-but-
# healthy boot as a failure, and it is still far below run-x86-boot-tests.sh's
# 900s because this profile runs no oracle cohort.
POLL_BOUND_SECONDS=240
# Liveness. STIMULUS-RESPONSE, and it has to be, but not for the reason a
# pre-#673 header could give: post-fix, production has a real userspace init
# (unconditional prints) and every context switch emits an unconditional raw-
# serial marker (context_switch.rs's `[SW]`/`<K>`/`<U>`/`<I>`, outside every
# cfg) -- so a healthy shipped kernel at steady state is NOT silent, and a
# bare byte-growth check cannot fail for the reason it would need to: it was
# measured on this branch's own boot growing 54346 -> 63572 bytes (+9226) in
# 15s with NO console input at all, which is that switch/init traffic, not an
# echo -- exactly the signature "console dead, kernel still switching" would
# leave (#673 review, B1/B2). Total byte growth cannot distinguish that from
# a live console, so it cannot redden on B1's failure mode and does not try
# to here.
#
# So the gate asks a question only the console echo path can answer: it
# sends a bare newline and requires serial_command_task's `breenix> ` prompt
# COUNT to grow by exactly one. That prompt is printed only when the
# console-executor kthread (#673's B1 fix) is actually scheduled, polls its
# executor, and serial_command_task processes an RX-interrupt-delivered
# newline through its read-line loop -- switch-trace noise and init's own
# stdout cannot forge it. A kernel wedged in its halt loop, or one where the
# console-executor kthread is dead (B1's exact failure mode: the boot
# thread's saved executor.run() context abandoned with nothing left to poll
# it), answers zero delta either way -- this is the prompt-count check below,
# strengthened from a bare presence pin into a before/after delta.
LIVENESS_STIMULUS_BYTE=$'\n'
LIVENESS_WINDOW_SECONDS=15

report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    # Disarm before doing anything else: the diagnosis below must not be able to
    # re-enter this handler and bury the original failure.
    trap - ERR
    echo "x86 production-profile gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
        QEMU_PID=""
    fi
    if compgen -G "$OUTPUT_DIR/serial_*.txt" >/dev/null 2>&1; then
        local failure_dir
        failure_dir="/tmp/breenix_x86_prod_profile_failures/$(date -u +%Y%m%dT%H%M%SZ)_$$"
        mkdir -p "$failure_dir"
        cp "$OUTPUT_DIR"/serial_*.txt "$failure_dir/"
        echo "  preserved failing serial: $failure_dir"
        echo "--- observed values ---"
        print_observed_values
        echo "--- serial tail (last 60 lines per file) ---"
        tail -n 60 "$OUTPUT_DIR"/serial_*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

# Matching-LINE count across both serial files (#673 review, mi7 -- grep -c
# counts lines, not substring occurrences, so two matches on one line would
# still count once). The x86 console carries init's stdout on the same
# serial stream as the shell prompt, so a same-line collision between a
# pinned literal and the prompt could in principle misreport a 1->2 delta as
# 1->1 (false red) or hide a real increase; 25/25 production-profile boots
# (#673 fix round 3) observed no such collision, so this is a disclosed,
# currently-inert sharp edge, not a silently mislabeled one. grep exits 1
# when nothing matches, which under `set -e`/`pipefail` would abort before
# the assertion that wants to read the zero, so the status is swallowed
# inside the group and awk -- which always exits 0 -- produces the number.
marker_count() {
    local literal="$1"
    local total
    total=$( { grep -F -h -c -- "$literal" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

crash_count() {
    local total
    total=$( { grep -E -h -c -- "$CRASH_MARKERS_PATTERN" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

serial_bytes() {
    local total
    total=$( { cat "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } | wc -c | awk '{ print $1 + 0 }')
    printf '%s' "$total"
}


print_observed_values() {
    echo "  ext2 root mounted:            $(marker_count "$EXT2_ROOT_LITERAL")"
    echo "  kernel init complete:         $(marker_count "$KERNEL_INIT_LITERAL")"
    echo "  async executor started:       $(marker_count "$EXECUTOR_LITERAL")"
    echo "  steady state reached:         $(marker_count "$STEADY_STATE_LITERAL")"
    echo "  console prompt:               $(marker_count "$CONSOLE_PROMPT_LITERAL")"
    echo "  tombstone census lines:       $(marker_count "$TOMBSTONE_CENSUS_PREFIX")"
    echo "  tombstone census at rest:     $(marker_count "$TOMBSTONE_CENSUS_PROD_LITERAL")"
    echo "  root custody lines:           $(marker_count "$ROOT_CUSTODY_PREFIX")"
    echo "  root custody at rest:         $(marker_count "$ROOT_CUSTODY_PROD_LITERAL")"
    echo "  crash markers:                $(crash_count)"
    local marker
    for marker in "${TEST_ONLY_MARKERS[@]}"; do
        echo "  test-only marker '$marker': $(marker_count "$marker")"
    done
    for marker in "${FAULT_MARKERS[@]}"; do
        echo "  fault marker '$marker': $(marker_count "$marker")"
    done
    echo "  fixed PRECONDITION 5 fail (#673): $(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")"
    echo "  fixed PRECONDITION 5 pass (#673): $(marker_count "$PRECOND_RUNNABLE_PASS_LITERAL")"
    echo "  fixed PRECONDITION 7 fail (#672): $(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")"
    echo "  fixed PRECONDITION 7 pass (#672): $(marker_count "$PRECOND_PREEMPT_PASS_LITERAL")"
    echo "  preempt census lines:          $(marker_count "$PREEMPT_CENSUS_PREFIX")"
    echo "  preempt census at rest:        $(marker_count "$PREEMPT_CENSUS_PROD_LITERAL")"
    echo "  prod bracket release census:   $(marker_count "$PROD_BRACKET_RELEASE_PREFIX")"
    echo "  prod bracket release at rest:  $(marker_count "$PROD_BRACKET_RELEASE_PROD_LITERAL")"
    echo "  init designation (#673):      $(marker_count "$INIT_DESIGNATION_X86_PREFIX")"
    echo "  ring3 syscall confirmed (#673): $(marker_count "$RING3_SYSCALL_LITERAL")"
    echo "  init first line (#673 M6):    $(marker_count "$INIT_FIRST_LINE_LITERAL")"
    echo "  init bsshd-launch warning (#673 MA4): $(marker_count "$INIT_BSSHD_WARNING_LITERAL")"
    # bsshd is not pinned as LISTENING: SPAWN is unconditionally ENOSYS on
    # x86 (#713), a pre-existing gap #673 exposed but did not cause. See
    # INIT SURVIVAL EVIDENCE above.
    { grep -F -h -- "$TOMBSTONE_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$ROOT_CUSTODY_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$PREEMPT_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$PROD_BRACKET_RELEASE_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$INIT_DESIGNATION_X86_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
}

cd "$BREENIX_ROOT"

echo "Building the shipped x86_64 production kernel profile..."
# #673 review, B5: the ONLY two legal values are empty (the shipped profile)
# and the documented anti-vacuity knob below. "The absence of --features is
# the point" (see the header) is not just prose about the default -- any
# other value here would silently build and measure a DIFFERENT kernel than
# the one this gate exists to prove, with no assertion below able to catch
# it, and the trap above is already armed to make this loud (#668).
test -z "$X86_PROD_PROFILE_EXTRA_FEATURES" -o "$X86_PROD_PROFILE_EXTRA_FEATURES" = "disable_x86_prod_init"
FEATURE_ARGS=()
if [ -n "$X86_PROD_PROFILE_EXTRA_FEATURES" ]; then
    FEATURE_ARGS=(--features "$X86_PROD_PROFILE_EXTRA_FEATURES")
    echo "  (#673 anti-vacuity leg: extra features = $X86_PROD_PROFILE_EXTRA_FEATURES)"
fi
# No --features by default, and that omission is the assertion: silently
# adding one would make this gate measure a different kernel than the one
# the project ships. The one documented exception is the #673 anti-vacuity
# knob above, which is off by default and must be opted into explicitly.
# The existing image is removed first so a stale artifact from a differently
# featured build cannot be picked up by the newest-first selection below.
rm -f target/release/build/breenix-*/out/breenix-uefi.img
BUILD_LOG=/tmp/breenix_x86_prod_profile_build.log
cargo build --release ${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"} --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is swallowed in
# the group and awk -- which always exits 0 -- produces the number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release ${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"} --bin qemu-uefi >/dev/null
UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"

# The ext2 image carries the userspace binaries, so rebuild it every run: a
# cached image silently boots an old root filesystem.
rm -f target/ext2.img
./scripts/create_ext2_disk.sh
test -f target/ext2.img

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"
cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
# Zero-filled stand-in for the test-binaries disk production does not carry.
# Its only job is to occupy virtio-blk index 1 so the ext2 root lands on index 2,
# which is where init_root_fs() looks for it.
dd if=/dev/zero of="$OUTPUT_DIR/placeholder.img" bs=1M count=16 status=none

echo "Booting the x86_64 production profile..."
qemu-system-x86_64 \
    -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
    -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
    -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_DIR/placeholder.img" \
    -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
    -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
    -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
    -display none -no-reboot -no-shutdown \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -chardev "socket,id=console,path=$OUTPUT_DIR/console.sock,server=on,wait=off,logfile=$OUTPUT_DIR/serial_user.txt" \
    -serial chardev:console \
    -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
    >"$OUTPUT_DIR/qemu.log" 2>&1 &
QEMU_PID=$!

reached=false
for _ in $(seq 1 "$POLL_BOUND_SECONDS"); do
    if grep -qF -- "$STEADY_STATE_LITERAL" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
        reached=true
        break
    fi
    if grep -qE -- "$CRASH_MARKERS_PATTERN" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
        break
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done

# Explicit assertion, not a bare boolean variable: a bare boolean executed as a
# command is silent under set -e and leaves a red with no verdict text.
test "$reached" = true

# Liveness. Both samples are taken with QEMU still running, after steady state,
# with the stimulus written between them: a kernel that has wedged in its halt
# loop never services the UART interrupt and never echoes, a live one answers
# with a fresh `breenix> ` prompt. See the LIVENESS_STIMULUS_BYTE block above
# for why a bare byte-growth check is not available on a kernel with #672 and
# #673 both fixed, and why counting THIS specific marker's growth is.
PROMPT_BEFORE=$(marker_count "$CONSOLE_PROMPT_LITERAL")
python3 - "$OUTPUT_DIR/console.sock" "$LIVENESS_STIMULUS_BYTE" <<'STIMULUS'
import socket
import sys

console = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
console.connect(sys.argv[1])
console.sendall(sys.argv[2].encode())
console.close()
STIMULUS
sleep "$LIVENESS_WINDOW_SECONDS"
PROMPT_AFTER=$(marker_count "$CONSOLE_PROMPT_LITERAL")

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

test "$PROMPT_AFTER" -gt "$PROMPT_BEFORE"

# Production milestones.
test "$(marker_count "$EXT2_ROOT_LITERAL")" -eq 1
test "$(marker_count "$KERNEL_INIT_LITERAL")" -eq 1
test "$(marker_count "$EXECUTOR_LITERAL")" -eq 1
test "$(marker_count "$STEADY_STATE_LITERAL")" -eq 1
# Strengthened from a bare presence pin (#673 review, B2) into a before/after
# delta: PROMPT_BEFORE is the steady-state prompt printed once at start,
# PROMPT_AFTER is that plus exactly the one the liveness stimulus earned.
test "$PROMPT_BEFORE" -eq 1
test "$PROMPT_AFTER" -eq 2

# Teardown census, at rest, in the shipped profile.
test "$(marker_count "$TOMBSTONE_CENSUS_PREFIX")" -eq 1
test "$(marker_count "$TOMBSTONE_CENSUS_PROD_LITERAL")" -eq 1
test "$(marker_count "$ROOT_CUSTODY_PREFIX")" -eq 1
test "$(marker_count "$ROOT_CUSTODY_PROD_LITERAL")" -eq 1

# Profile fidelity: nothing a test kernel emits may appear.
for marker in "${TEST_ONLY_MARKERS[@]}"; do
    test "$(marker_count "$marker")" -eq 0
done

# No production fault report, no crash.
for marker in "${FAULT_MARKERS[@]}"; do
    test "$(marker_count "$marker")" -eq 0
done
test "$(crash_count)" -eq 0

# #673, fixed: the shipped kernel now launches init, so PRECONDITION 5 must
# now pass and must still be reported. See the block above.
test "$(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")" -eq 0
test "$(marker_count "$PRECOND_RUNNABLE_PASS_LITERAL")" -eq 1

# #672, fixed: the preempt bracket is symmetric in the shipped profile, so
# PRECONDITION 7 must now pass and must still be reported.
test "$(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")" -eq 0
test "$(marker_count "$PRECOND_PREEMPT_PASS_LITERAL")" -eq 1

# Preempt-bracket census, at rest, in the shipped profile.
test "$(marker_count "$PREEMPT_CENSUS_PREFIX")" -eq 1
test "$(marker_count "$PREEMPT_CENSUS_PROD_LITERAL")" -eq 1

# #673 review, mi5: the production bracket's own release, censused separately
# from the shared census above (see its declaration comment).
test "$(marker_count "$PROD_BRACKET_RELEASE_PREFIX")" -eq 1
test "$(marker_count "$PROD_BRACKET_RELEASE_PROD_LITERAL")" -eq 1

# #673: init designation and syscall-execution evidence -- construction,
# dispatch, and execution, each proven independently. See the header.
test "$(marker_count "$INIT_DESIGNATION_X86_PREFIX")" -eq 1
test "$(marker_count "$RING3_SYSCALL_LITERAL")" -eq 1

# #673 review, M6/MA4: init survival evidence -- init reached its own first
# line and printed the bsshd-launch warning that only its own subsequent
# code path can produce. bsshd is not pinned as listening here: SPAWN is
# unconditionally ENOSYS on x86 today (#713, pre-existing, not a #673
# regression) -- see "INIT SURVIVAL EVIDENCE" above for the full account.
test "$(marker_count "$INIT_FIRST_LINE_LITERAL")" -eq 1
test "$(marker_count "$INIT_BSSHD_WARNING_LITERAL")" -eq 1

trap - ERR
# #673 review, B5: an anti-vacuity leg must never print a bare production PASS
# -- the measured arm has to be the shipping arm, or the verdict has to say so.
if [ -n "$X86_PROD_PROFILE_EXTRA_FEATURES" ]; then
    echo "PASS (feature-mutated build, features=$X86_PROD_PROFILE_EXTRA_FEATURES; NOT the shipped profile)"
else
    echo "PASS: x86 production profile reached steady state with the teardown census at rest"
fi
print_observed_values
echo "  console prompt count over ${LIVENESS_WINDOW_SECONDS}s: $PROMPT_BEFORE -> $PROMPT_AFTER"
echo "  (informational) total serial bytes at exit: $(serial_bytes)"
