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
# WHAT IT DOES NOT DO, STATED PLAINLY
#
# It launches NO userspace process (#673). `/sbin/init` is read and launched only by
# kernel/src/main_aarch64.rs; x86's kernel_main_continue() creates user processes
# exclusively inside `#[cfg(feature = "testing")]` / `#[cfg(feature = "interactive")]`
# blocks, and the serial console's `test` handler bottoms out in
# userspace_test::test_multiple_processes(), whose body is also `#[cfg(feature =
# "testing")]`. So the shipped x86 kernel spawns nothing, reaps nothing, and
# retires no page-table root.
#
# Therefore the census assertions below are a ZERO-WORKLOAD BASELINE, not a
# return-to-zero after a teardown. This gate CANNOT prove that x86 production
# teardown drains, because x86 production has no teardown to drain. What it does
# prove, and what nothing proved before, is that the census counters are live and
# at rest in the shipped kernel: any kernel-internal path -- a kthread, a
# boot-path process row, a page-table root taken and abandoned before userspace
# ever exists -- that leaked a tombstone or abandoned a root would move these
# fields off zero and redden this gate. Read the pinned literals as a
# conservation law over the whole shipped boot, not as evidence about process
# exit.
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
# ---------------------------------------------------------------------------
TEST_ONLY_MARKERS=(
    'TEST_TALLY:'
    '[TEST:'
    'TEST RUNNER:'
    '[BOOT_TESTS:'
    'RING3_SMOKE:'
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
# ATTRIBUTED pre-existing production-profile defects, pinned so they cannot move
# silently. R52 forbids an unattributed failure; it does not require pretending
# a disclosed one is absent. Both of these are printed by the shipped x86 kernel
# on every boot and were invisible until this gate existed:
#
#   PRECONDITION 5 (#673) -- "No runnable threads in scheduler". The shipped x86
#   kernel launches no userspace process at all (see the header). Symptom of that
#   gap, not an independent defect.
#
#   PRECONDITION 7 (#672) -- "Preemption is not disabled". kernel_main_continue() calls
#   per_cpu::preempt_disable() only inside `#[cfg(all(feature = "testing", not(feature
#   = "interactive")))]` but calls the matching preempt_enable() unconditionally,
#   so the shipped profile decrements a preempt_count that is already zero. The
#   x86 HAL decrement is a bare `sub dword ptr gs:[..], 1`, so the count wraps to
#   0xFFFFFFFF and preemption stays disabled for the rest of the boot.
#
# Both are pinned present-exactly-once ON PURPOSE: the day either is fixed this
# gate goes red and the pin must be re-derived to 0 in the same change. Do not
# relax these to >= or delete them -- that is how a disclosed defect becomes an
# undisclosed one.
# ---------------------------------------------------------------------------
PRECOND_RUNNABLE_FAIL_LITERAL='PRECONDITION 5: Scheduler has runnable threads ✗ FAIL'
PRECOND_PREEMPT_FAIL_LITERAL='PRECONDITION 7: Preemption disabled ✗ FAIL'

# Measured on beast under TCG: steady state at 14s from QEMU launch. The bound is
# an order of magnitude above that so host contention cannot score a slow-but-
# healthy boot as a failure, and it is still far below run-x86-boot-tests.sh's
# 900s because this profile runs no oracle cohort.
POLL_BOUND_SECONDS=240
# Liveness window. The production idle path emits its raw scheduler trace token
# roughly once every few seconds under TCG (measured: 6 bytes per 5s), so a
# window this size makes strict growth robust while still failing a kernel that
# has wedged in its halt loop.
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
    echo "  attributed PRECONDITION 5 fail: $(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")"
    echo "  attributed PRECONDITION 7 fail: $(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")"
    { grep -F -h -- "$TOMBSTONE_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$ROOT_CUSTODY_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
}

cd "$BREENIX_ROOT"

echo "Building the shipped x86_64 production kernel profile..."
# No --features, and that omission is the assertion: adding one here would make
# this gate measure a different kernel than the one the project ships.
# The existing image is removed first so a stale artifact from a differently
# featured build cannot be picked up by the newest-first selection below.
rm -f target/release/build/breenix-*/out/breenix-uefi.img
BUILD_LOG=/tmp/breenix_x86_prod_profile_build.log
cargo build --release --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is swallowed in
# the group and awk -- which always exits 0 -- produces the number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release --bin qemu-uefi >/dev/null
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
    -serial "file:$OUTPUT_DIR/serial_user.txt" \
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

# Liveness. Both samples are taken with QEMU still running, after steady state:
# a kernel that has wedged in its halt loop emits nothing more, a live one keeps
# emitting its raw scheduler trace token. This is the shipped profile's only
# periodic output -- it has no heartbeat process, because it has no userspace.
BYTES_BEFORE=$(serial_bytes)
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

# Attributed pre-existing defects: pinned, not tolerated. See the block above.
test "$(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")" -eq 1
test "$(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")" -eq 1

trap - ERR
echo "PASS: x86 production profile reached steady state with the teardown census at rest"
print_observed_values
echo "  serial bytes over ${LIVENESS_WINDOW_SECONDS}s: $BYTES_BEFORE -> $BYTES_AFTER"
