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
# `interactive` compiles the new block back out and reproduces the pre-fix,
# zero-userspace shipped kernel byte-for-byte, so this same gate can be run against
# it to prove the assertions below actually discriminate the fix rather than
# passing either way. Set X86_PROD_PROFILE_EXTRA_FEATURES=disable_x86_prod_init
# before invoking this script to run that leg.
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
# Construction is not dispatch, and dispatch is not execution, so three independent
# signals are asserted, each proving a strictly stronger claim than the last:
#   1. `[INIT_DESIGNATION:x86_64:designated_pid=1:...]` (emitted from the new call
#      site) -- proves the ProcessManager transaction ran and named PID 1 as init.
#   2. PRECONDITION 5 ("Scheduler has runnable threads") flips from its old,
#      attributed FAIL to PASS -- proves the thread reached the scheduler's
#      runnable set, checked live via `task::scheduler::with_scheduler` at
#      kernel/src/main.rs:1909-1915 (was main.rs:1798-1807 pre-fix, per #673's
#      original RCA -- the fix's own new block shifted every line after it).
#   3. `RING3_SYSCALL: First syscall from userspace` present exactly once --
#      proves init's userspace code actually ran and reached the syscall
#      handler. This is a pre-existing, one-time marker
#      (kernel/src/syscall/handler.rs's emit_ring3_syscall_marker(), raw
#      serial output, no locks) that already exists for the test framework's
#      own stage-advance bookkeeping; #673 does not add it, only asserts on
#      it in a profile that had never reached Ring 3 before.
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
# as "idle" -- can never resume. The fix (kernel/src/main.rs) is a second,
# dedicated preempt_disable()/preempt_enable() bracket scoped to exactly the
# production build: taken immediately after scheduler::spawn() (before init
# can be dispatched) and released only as the last thing before the async
# executor starts, once every line of boot's own sequential work is behind
# it. This gate's three evidence signals above are what proves that bracket
# is correctly placed: construction, then dispatch, then execution, in that
# order, with the shipped profile still reaching steady state afterward.
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

# ---------------------------------------------------------------------------
# #673 new evidence. Three independent signals, each a strictly stronger claim
# than the last (construction, then dispatch, then execution) -- see the
# header's "NEW EVIDENCE THIS GATE REQUIRES" section for the full rationale.
# INIT_DESIGNATION is matched by prefix (not full literal) because
# reserved_collisions is a live counter, not a fixed value; designated_pid=1
# is the part that must never drift.
# ---------------------------------------------------------------------------
INIT_DESIGNATION_X86_PREFIX='[INIT_DESIGNATION:x86_64:designated_pid=1:'
RING3_SYSCALL_LITERAL='RING3_SYSCALL: First syscall from userspace'

# Measured on beast under TCG: steady state at 14s from QEMU launch. The bound is
# an order of magnitude above that so host contention cannot score a slow-but-
# healthy boot as a failure, and it is still far below run-x86-boot-tests.sh's
# 900s because this profile runs no oracle cohort.
POLL_BOUND_SECONDS=240
# Liveness. STIMULUS-RESPONSE, and it has to be: until #672 was fixed, the
# shipped kernel's only periodic output was `pFFr1 ` - per_cpu::can_schedule()'s
# every-1000th-refusal debug trace (per_cpu.rs), printing the low byte of the
# preempt_count #672 had wrapped to 0xFFFFFFFF next to a set need_resched. That
# is the defect's own symptom, so the old "serial keeps growing on its own"
# check was measuring the bug rather than the kernel's health, and it goes
# silent - correctly - the moment the count is sane and can_schedule() stops
# refusing. A healthy shipped kernel at steady state emits NOTHING spontaneously:
# it has no userspace, no heartbeat process, and an async executor parked on the
# keyboard and serial streams.
#
# So the gate pokes it. Serial 0 is a socket chardev instead of a plain file
# (same logfile, so every marker count above is unchanged), one byte is written
# to the console after steady state, and the console's echo is what makes the
# byte count grow. That exercises UART RX interrupt delivery, the executor, and
# serial_command_task's echo path - strictly more than the old check, and it
# fails a kernel wedged in its halt loop for the same reason the old one did.
#
# Anti-vacuity: the chardev logfile records guest output only, not what the host
# writes into the socket. Measured on the fixed kernel: one byte in, exactly one
# byte logged. If it echoed the host's write the growth would be two.
#
# The byte is a printable character rather than a newline on purpose: a newline
# would make serial_command_task print a second `breenix> ` prompt and break the
# prompt pin above.
LIVENESS_STIMULUS_BYTE='x'
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

# Substring count across both serial files. grep exits 1 when nothing matches,
# which under `set -e`/`pipefail` would abort before the assertion that wants to
# read the zero, so the status is swallowed inside the group and awk -- which
# always exits 0 -- produces the number.
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
    echo "  init designation (#673):      $(marker_count "$INIT_DESIGNATION_X86_PREFIX")"
    echo "  ring3 syscall confirmed (#673): $(marker_count "$RING3_SYSCALL_LITERAL")"
    { grep -F -h -- "$TOMBSTONE_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$ROOT_CUSTODY_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$PREEMPT_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$INIT_DESIGNATION_X86_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
}

cd "$BREENIX_ROOT"

echo "Building the shipped x86_64 production kernel profile..."
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
cargo build --release "${FEATURE_ARGS[@]}" --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is swallowed in
# the group and awk -- which always exits 0 -- produces the number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release "${FEATURE_ARGS[@]}" --bin qemu-uefi >/dev/null
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
# loop never services the UART interrupt and never echoes, a live one answers.
# See the LIVENESS_STIMULUS_BYTE block above for why the old free-running
# byte-growth check is not available on a kernel with #672 fixed.
BYTES_BEFORE=$(serial_bytes)
python3 - "$OUTPUT_DIR/console.sock" "$LIVENESS_STIMULUS_BYTE" <<'STIMULUS'
import socket
import sys

console = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
console.connect(sys.argv[1])
console.sendall(sys.argv[2].encode())
console.close()
STIMULUS
sleep "$LIVENESS_WINDOW_SECONDS"
BYTES_AFTER=$(serial_bytes)

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

test "$BYTES_AFTER" -gt "$BYTES_BEFORE"

# Production milestones.
test "$(marker_count "$EXT2_ROOT_LITERAL")" -eq 1
test "$(marker_count "$KERNEL_INIT_LITERAL")" -eq 1
test "$(marker_count "$EXECUTOR_LITERAL")" -eq 1
test "$(marker_count "$STEADY_STATE_LITERAL")" -eq 1
test "$(marker_count "$CONSOLE_PROMPT_LITERAL")" -eq 1

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

# #673: init designation and syscall-execution evidence -- construction,
# dispatch, and execution, each proven independently. See the header.
test "$(marker_count "$INIT_DESIGNATION_X86_PREFIX")" -eq 1
test "$(marker_count "$RING3_SYSCALL_LITERAL")" -eq 1

trap - ERR
echo "PASS: x86 production profile reached steady state with the teardown census at rest"
print_observed_values
echo "  console echo over ${LIVENESS_WINDOW_SECONDS}s: $BYTES_BEFORE -> $BYTES_AFTER bytes"
