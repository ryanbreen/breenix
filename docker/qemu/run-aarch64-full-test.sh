#!/bin/bash
# ARM64 Full System Test (Native QEMU)
#
# This test matches the manual workflow of running ./run.sh:
#   Phase 1: Boot and run all 85 subsystem tests (wait for [BOOT_TESTS:PASS])
#   Phase 2: Verify BWM shell is up and services launched
#   Phase 3: Wait for bounce demo under GPU load (10+ seconds)
#   Phase 4: Verify kernel is still alive — no crashes during sustained operation
#
# This is the REAL test. Unlike boot-test-native.sh which exits at the shell
# prompt, this test waits for the full 85-test suite to complete and then
# monitors sustained operation under GPU load.
#
# Usage: ./run-aarch64-full-test.sh [--rebuild] [--boot-tests-only]
#
# Options:
#   --rebuild          Force rebuild of the kernel before testing
#   --boot-tests-only  Stop after the registered boot-test suite passes

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #825: two concurrent runs of this gate on the same host each hardcoded the
# identical /tmp/breenix_aarch64_full_test path, so one run's rm -rf/mkdir
# could delete and rewrite another run's in-flight boot output. Defaulting
# to /tmp keeps a caller that leaves it unset byte-identical; a concurrent-lane
# launcher sets this to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when it is read (the same F6 guard PR
# #801 gave the x86 gate scripts for #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac
# Creation/fork/slot/refusal/classifier/balance fields are oracle-driven and exact.
# aarch64 frames_mapped_delta and frames_released_delta are legitimately 0 because its HHDM-preallocated kernel stacks map no frames, which the oracle asserts in-kernel.
# live_checks is nonzero because every allocation evaluates the guard; pub_pooled and pub_sched_owned are nonzero boot-wide totals whose exact values depend on process creation, while the oracle asserts they are equal and both publication residuals are zero.
# sched_publications is a nonzero boot-wide driver for sched_pm_held_production=0. frame_used_delta varies with heap growth, while the oracle asserts it is strictly less than one 128-frame kernel stack.
KSTACK_OWNER_ORACLE_PATTERN='^\[KSTACK_OWNER_ORACLE:aarch64:creation_rows=1000:creation_owned=1000:one_owner=1000:two_owner=0:zero_owner=0:fork_rows=1:fork_owned=1:slot_returns_exact_one=1:slot_alloc_delta=[1-9][0-9]*:slot_free_delta=[1-9][0-9]*:slot_balance=-?[0-9]+:cohort_enrolled=1000:cohort_returned=1000:cohort_double_return=0:foreign_alloc_delta=[0-9]+:foreign_returned=[0-9]+:frames_mapped_delta=0:frames_released_delta=0:frame_balance=0:frame_used_delta=[0-9]+:frame_used_bounded=1:live_checks=[1-9][0-9]*:live_refusals_production=0:live_refusals_injected=1:drop_refused_live=0:pte_overwrite_refusals=0:pub_pooled=[1-9][0-9]*:pub_sched_owned=[1-9][0-9]*:pub_row_residual=0:pub_unowned=0:classifier_sched_owned=1:classifier_row_residual=1:classifier_unowned=1:classifier_not_pooled=1:sched_publications=[1-9][0-9]*:sched_pm_held_production=0:sched_pm_held_injected=1:reconciliation_diff=-?[0-9]+:reconciliation_skew_bound=[0-9]+:balance=0\]$'
# driven=2 proves both waiter-owned wake seams ran; stage1/2 return, wake, and
# park fields expose D1/D2. stage3_elapsed_ok=1 proves the interval the oracle
# measured reached the full requested duration -- since #627 that interval is
# anchored to the same clock read the kernel used to compute the deadline, not
# to a later oracle-internal read, so this bit can no longer read 0 on a wait
# that was never actually short. arm_delay_us is that retired gap, kept visible.
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms reports the measured duration, and residual/balance prove cleanup.
# claim-lint:ok: #627 -- provable by construction from program order (futex.rs
# reads base_ns before its deadline check; record_arm's own clock read comes
# after), not by boot sampling: see kernel/src/syscall/futex_oracle.rs::record_arm
# and validate_futex_oracle_record_arm_anchor in tests/teardown_structure.rs.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:arm_delay_us=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
# The boot-test oracle drives this injection exactly once; its forbidden detector output is pinned absent below.
CREATION_LOCK_ORDER_INJECTED_LITERAL='[CREATION_LOCK_ORDER:INJECTED:PM_HELD]'
CREATION_LOCK_ORDER_VIOLATION_LITERAL='[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]'
# resolved_production may be zero once #605's early-slot-consumption defect is fixed; deterministic resolved_exercised proves the resolver ran.
SCHED_STRAND_ORACLE_PATTERN='\[SCHED_STRAND_ORACLE:aarch64:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[1-9][0-9]*:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+:nonprogress=[0-9]+:queued_on_nondispatching_cpu=[0-9]+:worst_queued_nondispatch_ms=[0-9]+:worst_cpu_scheduler_silence_ms=[0-9]+:worst_silence_cpu=[0-9]+\]'
STRAND_INJECT_ORACLE_PATTERN='\[STRAND_INJECT_ORACLE:aarch64:legA_exercised=1:legA_recovered=1:legB_exercised=1:legB_recovered=1:stranded=0\]'

# Parse args
REBUILD=false
BOOT_TESTS_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=true ;;
        --boot-tests-only) BOOT_TESTS_ONLY=true ;;
    esac
done

# Optionally rebuild
if $REBUILD; then
    echo "Building ARM64 kernel with boot_tests feature..."
    (cd "$BREENIX_ROOT" && cargo build --release --features boot_tests \
        --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64 2>&1)
    echo "Build complete."
    echo ""
fi

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found at $KERNEL"
    echo "Build with: cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Durable #528 guard: the kernel MUST be soft-float. Fail fast if it was built
# with the NEON hardfloat target (aarch64-breenix.json) — that re-arms #528.
# (set -e aborts the test if the guard trips.)
"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

# Durable feature-profile guard, the twin of the #528 guard above. This test pins
# markers that ONLY a `--features boot_tests` kernel emits, so without --rebuild
# a kernel left behind in another profile fails every phase on "marker missing"
# and reads as a kernel regression.
#
# `cargo` keeps one cached artifact per feature set and hardlinks the requested
# one into this single output path in about 0.06 s, with no recompilation and no
# output worth reading. ANY `cargo test` in the same session therefore replaces
# this binary silently — `cargo test --test kernel_no_neon_guard` builds the
# kernel with NO features by design — and the next gate boots the wrong kernel.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of marker literals rather than one sentinel: a single marker
    # changing profile must not be able to disarm this guard quietly.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[CENSUS_WIDEN_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[TOMBSTONE_JOIN_ORACLE:' '[BOOT_TESTS:'; do
        if ! grep -aqF "$marker" "$kernel" 2>/dev/null; then
            missing="$missing $marker"
        fi
    done

    if [ -n "$missing" ]; then
        echo "Error: $kernel was not built with --features boot_tests."
        echo "  Missing boot_tests-only marker literal(s):$missing"
        echo "  This test pins those markers, so every phase would fail on 'marker missing'."
        echo "  Re-run with --rebuild, or build with:"
        echo "    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
        echo "  NOTE: any 'cargo test' in this session rebuilds the kernel WITHOUT boot_tests and"
        echo "  silently swaps this binary in a fraction of a second. Build after testing, not before."
        exit 1
    fi
}

require_boot_tests_kernel "$KERNEL"

# Find ext2 disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_full_test"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Create writable copy of ext2 disk
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

cleanup() {
    if [ -n "${QEMU_PID:-}" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "========================================="
echo "ARM64 Full System Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Start QEMU in background (180s total timeout — 84 tests, the exec smoke, and the soak)
timeout 180 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
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
QEMU_PID=$!

FAIL_REASON=""
TESTS_PASSED=0
TESTS_TOTAL=0
TEST_FAILURES=""

# Helper: check for fatal markers
check_fatal() {
    local serial="$OUTPUT_DIR/serial.txt"
    if grep -qiE "soft lockup detected" "$serial" 2>/dev/null; then
        echo "Soft lockup detected"
        return 0
    fi
    if grep -qiE "(KERNEL PANIC|panic!)" "$serial" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "DATA_ABORT.*FAR=" "$serial" 2>/dev/null; then
        echo "DATA_ABORT"
        return 0
    fi
    if grep -qiE "INSTRUCTION_ABORT" "$serial" 2>/dev/null; then
        echo "INSTRUCTION_ABORT"
        return 0
    fi
    if grep -qiE "Unhandled sync exception" "$serial" 2>/dev/null; then
        echo "Unhandled exception"
        return 0
    fi
    if grep -qE "\[EXEC_LOCK_ORDER:VIOLATION" "$serial" 2>/dev/null; then
        echo "Exec lock-order violation"
        return 0
    fi
    if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' "$serial" 2>/dev/null; then
        echo "Creation lock-order violation"
        return 0
    fi
    return 1
}

# --- Phase 1: Run all 85 subsystem tests (up to 90s) ---
echo "Phase 1: Running 85 subsystem tests..."
echo "  (Waiting for [BOOT_TESTS:PASS] or [BOOT_TESTS:FAIL])"
PHASE1_OK=false
for i in $(seq 1 45); do  # 45 * 2s = 90s timeout
    if ! kill -0 $QEMU_PID 2>/dev/null; then
        FAIL_REASON="Phase 1: QEMU exited before tests completed"
        break
    fi

    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        # Check for test suite completion
        if grep -q "\[BOOT_TESTS:PASS\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            PHASE1_OK=true
            TESTS_PASSED=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:\([0-9]*\).*/\1/' | tail -1)
            TESTS_TOTAL=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:[0-9]*\/\([0-9]*\).*/\1/' | tail -1)
            echo "  All tests passed: ${TESTS_PASSED:-?}/${TESTS_TOTAL:-?}"
            break
        fi

        # Check for test suite failure (tests ran but some failed)
        if grep -q "\[BOOT_TESTS:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            TEST_FAILURES=$(grep "\[TEST:.*:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)
            TESTS_PASSED=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:\([0-9]*\).*/\1/' | tail -1)
            TESTS_TOTAL=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:[0-9]*\/\([0-9]*\).*/\1/' | tail -1)
            FAIL_REASON="Phase 1: Test suite completed with failures (${TESTS_PASSED:-?}/${TESTS_TOTAL:-?})"
            break
        fi

        # Check for crash during tests
        if FATAL=$(check_fatal); then
            # Report which test was running when crash happened
            LAST_TEST=$(grep "\[TEST:.*:START\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1: $FATAL during test execution. Last test: $LAST_TEST"
            break
        fi

        # Progress indicator every 10 seconds
        if (( i % 5 == 0 )); then
            COMPLETED=$(grep -c "\[TEST:.*:PASS\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            FAILED=$(grep -c "\[TEST:.*:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            echo "  Progress: ${COMPLETED} passed, ${FAILED} failed (${i}*2s elapsed)"
        fi
    fi
    sleep 2
done

if ! $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    FAIL_REASON="Phase 1 timeout: tests did not complete within 90s"
fi

# Device-enumeration census leg (green arc 5, bus+NIC blended). The expected
# VirtIO MMIO device total is self-counted from this script's OWN -device
# flags above, not a hand-pinned literal, so a future edit to the QEMU
# invocation cannot silently desync the assertion below from what actually
# boots (the #549/#551/[[gate-target-fidelity-528]] census-not-literal
# lesson). This converts "enumerate_devices() returned" into "the declared
# device set, by type, was actually found" -- the precise gap #702 exposed on
# x86 (a boot can die silently right after device detection, and every
# marker-grep gate reads that only as an undifferentiated timeout).
EXPECTED_MMIO_DEVICES=$(grep -cE -- '^[[:space:]]*-device virtio-[a-z]*-device' "${BASH_SOURCE[0]}")
MMIO_CENSUS_LINE=$(grep -h -E '\[drivers\] Found [0-9]+ VirtIO MMIO devices' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
MMIO_CENSUS_TOTAL=$(printf '%s\n' "$MMIO_CENSUS_LINE" | sed -n 's/.*Found \([0-9]*\) VirtIO MMIO devices.*/\1/p')
MMIO_NETWORK_COUNT=$(grep -h -c -F '[drivers] Found VirtIO MMIO device: network' "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)
MMIO_BLOCK_COUNT=$(grep -h -c -F '[drivers] Found VirtIO MMIO device: block' "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)

if $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    # The scheduler publication seam emits this prefix if publication happens
    # while the process-manager lock is held on the same CPU. Its absence is
    # driven by the nonzero sched_publications field and the injected marker.
    if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: creation lock-order violation detected (forbidden $CREATION_LOCK_ORDER_VIOLATION_LITERAL)"
    elif [ "$(grep -Fxc "$CREATION_LOCK_ORDER_INJECTED_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)" -ne 1 ]; then
        FAIL_REASON="Phase 1: creation lock-order detector injection marker count is not exactly one"
    elif ! grep -Eq "$KSTACK_OWNER_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing or malformed aarch64 kernel-stack ownership oracle marker"
    elif ! grep -Fq '[CREATING_DISPATCH_ORACLE:aarch64:injected=1:refused_via_dispatch=1:requeue_retried=1:dispatched_after_publish=1:balance=0:leaf_residual=16:user_stack_residual=16]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing creating-dispatch refusal oracle marker"
    elif ! grep -Fq '[TEST:process:init_designation_oracle:PASS]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing init designation oracle PASS marker"
    elif ! grep -Fq '[INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=2:construct_residual=2:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing init designation oracle counter marker"
    elif ! grep -Fq '[INIT_GROUP_REFUSAL_ORACLE:aarch64:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing init-group refusal oracle counter marker"
    elif ! grep -Eq '\[BLOCK_WEDGE_ORACLE:locked=1:wedged=1:refused=1:parked=0:refuse_ms=[0-9]+\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing block wedge oracle counter marker"
    # P6a PR-2 gate extras (b)/(f)/(g). Every field is a delta the oracle drives
    # itself inside one run: two fixture rows, one joined by retirement and one
    # by the reap, the gauge back at its entry value and no tombstone left. The
    # tally alone cannot pin this — deleting the oracle's registry entry lowers
    # TESTS_TOTAL and TESTS_PASSED together and stays green.
    elif [ "$(grep -Fxc '[TOMBSTONE_JOIN_ORACLE:aarch64:retire_second=1:reap_second=1:removed=2:resident_delta=0:tombstone_rows=0:PASS]' "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)" -ne 1 ]; then
        FAIL_REASON="Phase 1: tombstone join oracle marker count is not exactly one"
    elif [ -z "$MMIO_CENSUS_LINE" ]; then
        FAIL_REASON="Phase 1: device-enumeration census absent -- see kernel/src/drivers/{mod.rs,virtio/mmio.rs}"
    elif [ -z "$MMIO_CENSUS_TOTAL" ]; then
        FAIL_REASON="Phase 1: device-enumeration census line malformed: $MMIO_CENSUS_LINE"
    elif [ "$MMIO_CENSUS_TOTAL" -ne "$EXPECTED_MMIO_DEVICES" ]; then
        FAIL_REASON="Phase 1: device-enumeration census reports $MMIO_CENSUS_TOTAL VirtIO MMIO device(s), self-counted expected $EXPECTED_MMIO_DEVICES from this script's own -device flags"
    elif [ "${MMIO_NETWORK_COUNT:-0}" -lt 1 ]; then
        FAIL_REASON="Phase 1: device-enumeration census found no VirtIO MMIO network device, though -device virtio-net-device is attached"
    elif [ "${MMIO_BLOCK_COUNT:-0}" -lt 1 ]; then
        FAIL_REASON="Phase 1: device-enumeration census found no VirtIO MMIO block device, though -device virtio-blk-device is attached"
    fi
fi

# The strand detector deliberately emits its first census about three seconds
# after it starts, independently of boot-test completion. Give that fixed
# cadence time to produce both required markers before scoring the boot.
if $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1a3: Waiting for scheduler strand oracles..."
    STRAND_ORACLES_OK=false
    for i in $(seq 1 40); do
        if grep -qF "[INSTRUCTION_ABORT]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            INSTRUCTION_ABORT_LINE=$(grep -F "[INSTRUCTION_ABORT]" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a3: INSTRUCTION_ABORT reported ($INSTRUCTION_ABORT_LINE)"
            break
        fi
        if grep -qF "[DATA_ABORT]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            DATA_ABORT_LINE=$(grep -F "[DATA_ABORT]" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a3: DATA_ABORT reported ($DATA_ABORT_LINE)"
            break
        fi
        if grep -qE '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            STRAND_LINE=$(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a3: scheduler strand census reported stranded work ($STRAND_LINE)"
            break
        fi
        if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            STRAND_LINE=$(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a3: scheduler strand injection oracle reported stranded work ($STRAND_LINE)"
            break
        fi
        if grep -qE "$SCHED_STRAND_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null \
            && grep -qE "$STRAND_INJECT_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            STRAND_ORACLES_OK=true
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1a3: scheduler strand oracles never completed (QEMU exited)"
            break
        fi
        sleep 1
    done

    if ! $STRAND_ORACLES_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1a3: scheduler strand oracle marker absent (40s timeout)"
    fi
    if $STRAND_ORACLES_OK && [ -z "$FAIL_REASON" ]; then
        echo "  Observed: $(grep -E "$SCHED_STRAND_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" | tail -1)"
        echo "  Observed: $(grep -E "$STRAND_INJECT_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" | tail -1)"
        echo "Phase 1a3: PASS"
    fi
fi

# --- Phase 1a: Pin the init-driven block request lifetime oracle (up to 30s) ---
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1a: Waiting for block EINTR oracle..."
    BLOCK_EINTR_ORACLE_OK=false
    for i in $(seq 1 15); do
        if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BLOCK_EINTR_ORACLE_LINE=$(grep -F "[BLOCK_EINTR_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a: block EINTR oracle failed ($BLOCK_EINTR_ORACLE_LINE)"
            break
        fi
        if grep -qF "[BLOCK_EINTR_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BLOCK_EINTR_ORACLE_OK=true
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1a: block EINTR oracle never completed ($FATAL)"
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1a: block EINTR oracle never completed (QEMU exited)"
            break
        fi
        sleep 2
    done

    if ! $BLOCK_EINTR_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1a: block EINTR oracle marker absent (30s timeout)"
    fi

    if $BLOCK_EINTR_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        BLOCK_EINTR_ORACLE_LINE=$(grep -F "[BLOCK_EINTR_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
        if echo "$BLOCK_EINTR_ORACLE_LINE" | grep -qF "[BLOCK_EINTR_ORACLE:FAIL"; then
            FAIL_REASON="Phase 1a: block EINTR oracle failed ($BLOCK_EINTR_ORACLE_LINE)"
        else
            echo "  Observed: $BLOCK_EINTR_ORACLE_LINE"
            echo "Phase 1a: PASS"
        fi
    fi
fi

# --- Phase 1a1: Pin the #568 blocking-poll-on-connected-TCP oracle ---
# init runs this immediately after the block EINTR oracle above, so by the time
# Phase 1a has passed it is either already in the serial or about to be. Both
# halves are pinned -- the marker must appear, and it must not be a FAIL --
# because a marker check alone passes a boot whose verdict was FAIL, and a FAIL
# check alone passes a boot where the program never started. Before this phase
# existed, a POLL_TCP_ORACLE FAIL was invisible to every aarch64 gate: the
# oracle exited non-zero and the run still reported GREEN.
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1a1: Waiting for poll TCP oracle (#568)..."
    POLL_TCP_ORACLE_OK=false
    for i in $(seq 1 15); do
        if grep -qF "[POLL_TCP_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            POLL_TCP_ORACLE_LINE=$(grep -F "[POLL_TCP_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a1: poll TCP oracle failed ($POLL_TCP_ORACLE_LINE)"
            break
        fi
        if grep -qF "[POLL_TCP_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            POLL_TCP_ORACLE_OK=true
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1a1: poll TCP oracle never completed ($FATAL)"
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1a1: poll TCP oracle never completed (QEMU exited)"
            break
        fi
        sleep 2
    done

    if ! $POLL_TCP_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1a1: poll TCP oracle marker absent (30s timeout)"
    fi

    if $POLL_TCP_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        POLL_TCP_ORACLE_LINE=$(grep -F "[POLL_TCP_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
        # #693: the kernel's own contradiction check, pinned independently of
        # the oracle's verdict because it is a statement about kernel state
        # rather than a userspace program's opinion: a blocking poll handed back
        # a fd without POLLIN although bytes were published into that connection
        # inside the poll's window and are still buffered.
        if grep -qF "[POLL_TCP_READY_LOST]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1a1: kernel reported a lost TCP readiness publication (#693): $(grep -aF -m1 "[POLL_TCP_READY_LOST]" "$OUTPUT_DIR/serial.txt")"
        elif echo "$POLL_TCP_ORACLE_LINE" | grep -qF "[POLL_TCP_ORACLE:FAIL"; then
            FAIL_REASON="Phase 1a1: poll TCP oracle failed ($POLL_TCP_ORACLE_LINE)"
        else
            echo "  Observed: $POLL_TCP_ORACLE_LINE"
            echo "Phase 1a1: PASS"
        fi
    fi
fi

# --- Phase 1a2: Pin the deterministic futex handoff oracle ---
# The pattern keeps every kernel-behaviour field exact while allowing only the
# measured stage3_elapsed_ms field to vary with emulator wall-clock speed.
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1a2: Waiting for futex handoff oracle..."
    FUTEX_HANDOFF_ORACLE_OK=false
    for i in $(seq 1 15); do
        if grep -qE "$FUTEX_HANDOFF_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FUTEX_HANDOFF_ORACLE_OK=true
            FUTEX_HANDOFF_ORACLE_LINE=$(grep -E "$FUTEX_HANDOFF_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            break
        fi
        if grep -qF '[FUTEX_HANDOFF_ORACLE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            # The prefix can match a line still being flushed to the serial
            # file before the anchored pattern above sees it complete — that
            # is an adjudication race, not a real failure (#R3-B2). Settle
            # briefly and re-check the anchored pattern once before treating
            # the prefix match as a genuine failure.
            sleep 0.3
            if grep -qE "$FUTEX_HANDOFF_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                FUTEX_HANDOFF_ORACLE_OK=true
                FUTEX_HANDOFF_ORACLE_LINE=$(grep -E "$FUTEX_HANDOFF_ORACLE_PATTERN" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
                break
            fi
            FUTEX_HANDOFF_ORACLE_LINE=$(grep -F '[FUTEX_HANDOFF_ORACLE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1a2: futex handoff oracle failed ($FUTEX_HANDOFF_ORACLE_LINE)"
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1a2: futex handoff oracle never completed ($FATAL)"
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1a2: futex handoff oracle never completed (QEMU exited)"
            break
        fi
        sleep 2
    done

    if ! $FUTEX_HANDOFF_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1a2: futex handoff oracle marker absent (30s timeout)"
    fi

    if $FUTEX_HANDOFF_ORACLE_OK && [ -z "$FAIL_REASON" ]; then
        echo "  Observed: $FUTEX_HANDOFF_ORACLE_LINE"
        echo "Phase 1a2: PASS"
    fi
fi

# --- Phase 1b: Exercise the init-driven exec path (up to 30s) ---
if [ -z "$FAIL_REASON" ]; then
    echo "Phase 1: PASS (${TESTS_PASSED}/${TESTS_TOTAL} tests)"
    echo ""
    echo "Phase 1b: Running exec smoke..."
    EXEC_SMOKE_OK=false
    for i in $(seq 1 15); do
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1b: exec smoke never completed (QEMU exited)"
            break
        fi

        EXEC_SMOKE_FAILURE=$(grep -E "\[EXEC_SMOKE:(EXEC_FAILED|TARGET_ARGV_FAIL|SPAWN_FAILED)" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        if [ -n "$EXEC_SMOKE_FAILURE" ]; then
            FAIL_REASON="Phase 1b: exec smoke never completed ($EXEC_SMOKE_FAILURE)"
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1b: exec smoke never completed ($FATAL)"
            break
        fi
        if grep -q "\[EXEC_SMOKE:TARGET_OK\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            EXEC_SMOKE_OK=true
            break
        fi
        sleep 2
    done

    if ! $EXEC_SMOKE_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1b: exec smoke never completed (30s timeout)"
    fi

    if $EXEC_SMOKE_OK && [ -z "$FAIL_REASON" ]; then
        if ! grep -q "\[EXEC_LOCK_ORDER:FIRST_COMMIT\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1b: exec smoke never completed (missing [EXEC_LOCK_ORDER:FIRST_COMMIT])"
        else
            EXEC_COUNTER_LINE=$(grep -E "\[EXEC_LOCK_ORDER:commits=[0-9]+:pm_held=[0-9]+:unpinned=[0-9]+:missing=[0-9]+\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
            EXEC_COMMITS=$(echo "$EXEC_COUNTER_LINE" | sed -n 's/.*commits=\([0-9][0-9]*\).*/\1/p')
            EXEC_PM_HELD=$(echo "$EXEC_COUNTER_LINE" | sed -n 's/.*pm_held=\([0-9][0-9]*\).*/\1/p')
            EXEC_UNPINNED=$(echo "$EXEC_COUNTER_LINE" | sed -n 's/.*unpinned=\([0-9][0-9]*\).*/\1/p')
            EXEC_MISSING=$(echo "$EXEC_COUNTER_LINE" | sed -n 's/.*missing=\([0-9][0-9]*\).*/\1/p')
            if [ -z "$EXEC_COMMITS" ] || [ -z "$EXEC_PM_HELD" ] || [ -z "$EXEC_UNPINNED" ] || [ -z "$EXEC_MISSING" ] || \
               [ "$EXEC_COMMITS" -lt 1 ] || [ "$EXEC_PM_HELD" -ne 0 ] || \
               [ "$EXEC_UNPINNED" -ne 0 ] || [ "$EXEC_MISSING" -ne 0 ]; then
                FAIL_REASON="Phase 1b: exec smoke counters invalid (observed: ${EXEC_COUNTER_LINE:-none})"
            else
                echo "  Exec smoke: $EXEC_COUNTER_LINE"
                echo "Phase 1b: PASS"
            fi
        fi
    fi
fi

# --- Phase 1c: The exec-detach runtime proof must actually run (up to 30s) ---
# clonevm_exec_test is launched by /sbin/init immediately after the exec smoke.
# The kernel's ext2 test-binary loader is #[cfg(feature = "testing")] and this
# gate builds boot_tests, so init is the only launch path in this profile; before
# it existed the program was built and launched nowhere, and its absence was
# invisible because no gate pinned its markers. Pin them: a silent skip (the
# program missing from the image, or init not launching it) is now a hard failure.
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1c: Running clonevm_exec_test (exec detach proof)..."
    CLONEVM_OK=false
    for i in $(seq 1 15); do
        if grep -qF "CLONEVM_EXEC_TEST: ERROR" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            CLONEVM_ERROR=$(grep -F "CLONEVM_EXEC_TEST: ERROR" "$OUTPUT_DIR/serial.txt" | tail -1)
            FAIL_REASON="Phase 1c: clonevm_exec_test reported an error ($CLONEVM_ERROR)"
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1c: clonevm_exec_test never completed ($FATAL)"
            break
        fi
        if grep -qF "CLONEVM_EXEC_TEST: PASS" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            CLONEVM_OK=true
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1c: clonevm_exec_test never completed (QEMU exited)"
            break
        fi
        sleep 2
    done

    if ! $CLONEVM_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1c: clonevm_exec_test never completed (30s timeout)"
    fi

    if $CLONEVM_OK && [ -z "$FAIL_REASON" ]; then
        # The aarch64-only live-sibling arm must fire; on this arch a SKIP marker
        # would mean the guard probe was compiled for the wrong target.
        if ! grep -qF "CLONEVM_EXEC_TEST: live sibling refused exec" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1c: live-sibling refusal probe did not run"
        elif ! grep -qF "CLONEVM_EXEC_TEST: post-exec rendezvous complete" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1c: post-exec rendezvous did not complete"
        else
            echo "Phase 1c: PASS"
        fi
    fi
fi

# --- Phase 1d: init identity ---
# AC-8 retry evidence: oracle arms A1 and A2 fail construction at the reserved PID before
# production launch_init_from_elf succeeds and designates that PID in the same aarch64 boot.
# Their serial ordering makes the Phase 1d construction a retry after both reserved-PID failures.
# Phase 1d then proves designated PID == userspace-observed PID == 1 with no reserved collision.
# The x86 oracle exercises only the ticket path over synthetic rows; x86 has no constructor retry.
if [ -z "$FAIL_REASON" ]; then
    INIT_DESIGNATION_LINE=$(grep -E '\[INIT_DESIGNATION:aarch64:designated_pid=[0-9]+:reserved_collisions=[0-9]+\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
    if [ -z "$INIT_DESIGNATION_LINE" ]; then
        FAIL_REASON="Phase 1d: init designation marker is absent"
    else
        INIT_DESIGNATED_PID=$(echo "$INIT_DESIGNATION_LINE" | sed -n 's/.*designated_pid=\([0-9][0-9]*\):reserved_collisions=.*/\1/p')
        INIT_RESERVED_COLLISIONS=$(echo "$INIT_DESIGNATION_LINE" | sed -n 's/.*reserved_collisions=\([0-9][0-9]*\).*/\1/p')
    fi

    INIT_USERSPACE_LINE=$(grep -E '\[init\] Breenix init starting \(PID [0-9]+\)' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
    if [ -z "$INIT_USERSPACE_LINE" ] && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1d: userspace init PID marker is absent"
    elif [ -n "$INIT_USERSPACE_LINE" ]; then
        INIT_USERSPACE_PID=$(echo "$INIT_USERSPACE_LINE" | sed -n 's/.*\[init\] Breenix init starting (PID \([0-9][0-9]*\)).*/\1/p')
    fi

    if [ -z "$FAIL_REASON" ]; then
        if [ "$INIT_DESIGNATED_PID" -ne "$INIT_USERSPACE_PID" ]; then
            FAIL_REASON="Phase 1d: designated init PID $INIT_DESIGNATED_PID does not match userspace PID $INIT_USERSPACE_PID"
        elif [ "$INIT_DESIGNATED_PID" -ne 1 ]; then
            FAIL_REASON="Phase 1d: designated init PID is $INIT_DESIGNATED_PID, expected 1"
        elif [ "$INIT_RESERVED_COLLISIONS" -ne 0 ]; then
            FAIL_REASON="Phase 1d: reserved init PID collision count is $INIT_RESERVED_COLLISIONS, expected 0"
        else
            echo "Phase 1d: PASS (init designated and observed as PID $INIT_DESIGNATED_PID)"
        fi
    fi
fi

# --- Phase 1e: init-group refusal whole-boot assertion (up to 60s) ---
# The process map is walked with init's full service set live. `foreign_tgid_rows=0`
# is the acceptance quantity, while `init_tgid_rows=1` forbids a vacuous pass over
# an empty map.
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 1e: Waiting for init-group refusal quiesce proof..."
    INIT_GROUP_QUIESCE_OK=false
    for i in $(seq 1 30); do
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1e: init-group refusal quiesce proof never completed ($FATAL)"
            break
        fi
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 1e: init-group refusal quiesce proof never completed (QEMU exited)"
            break
        fi
        if grep -qF "[INIT_GROUP_REFUSAL:aarch64:phase=quiesce:probe1=-22:probe2=-22:expected=-22]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            INIT_GROUP_QUIESCE_OK=true
            break
        fi
        sleep 2
    done

    if ! $INIT_GROUP_QUIESCE_OK && [ -z "$FAIL_REASON" ]; then
        FAIL_REASON="Phase 1e: init-group refusal quiesce marker absent (60s timeout)"
    fi

    if $INIT_GROUP_QUIESCE_OK && [ -z "$FAIL_REASON" ]; then
        INIT_GROUP_WALK_LINE=$(grep -E '^\[INIT_GROUP_WALK:aarch64:rows=[0-9]+:init_tgid_rows=1:foreign_tgid_rows=0:refused=4:verdict=PASS\]$' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        INIT_GROUP_WALK_ROWS=$(echo "$INIT_GROUP_WALK_LINE" | sed -n 's/^\[INIT_GROUP_WALK:aarch64:rows=\([0-9][0-9]*\):.*/\1/p')
        # The green Phase 1e full-test run observed rows=11.  A floor of 8
        # leaves three rows of headroom for a legitimately shorter service set
        # while making the vacuous rows=1 case a hard failure.
        INIT_GROUP_WALK_ROWS_FLOOR=8
        if [ -z "$INIT_GROUP_WALK_LINE" ]; then
            FAIL_REASON="Phase 1e: init-group refusal quiesce walk marker absent"
        elif [ -z "$INIT_GROUP_WALK_ROWS" ] || [ "$INIT_GROUP_WALK_ROWS" -lt "$INIT_GROUP_WALK_ROWS_FLOOR" ]; then
            FAIL_REASON="Phase 1e: init-group refusal quiesce walk rows ${INIT_GROUP_WALK_ROWS:-<missing>} below floor $INIT_GROUP_WALK_ROWS_FLOOR"
        elif grep -qE '\[INIT_GROUP_WALK:.*verdict=FAIL' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1e: init-group walk reported failure"
        elif grep -qF "[INIT_GROUP_CHILD_RAN]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FAIL_REASON="Phase 1e: refused init-group child ran"
        else
            echo "  Observed: $INIT_GROUP_WALK_LINE"
            echo "Phase 1e: PASS"
        fi
    fi
fi

# --- Phase 2: Verify services (10s) ---
if [ -z "$FAIL_REASON" ] && ! $BOOT_TESTS_ONLY; then
    echo ""
    echo "Phase 2: Checking services..."
    SHELL_OK=false
    BWM_OK=false
    for i in $(seq 1 5); do
        if grep -qE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
        # A bare PTY unlock is NOT evidence of a shell. It used to be accepted
        # here as a proxy, on the assumption that only a shell-spawning service
        # ever unlocks a PTY during boot. The green-program TTY leg
        # (/bin/tty_oracle) broke that assumption: it calls unlockpt() on every
        # boot, which flipped this phase from its honest #593 red to reporting
        # "PASS (shell spawned)" on a boot with zero shell markers. The proxy is
        # gone; Phase 2 now requires actual shell output, and stays red until
        # #593 (init's aarch64 arm spawns no shell) is fixed.
        if grep -qE "\[bwm\] Display:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BWM_OK=true
        fi
        if $SHELL_OK; then break; fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 2: $FATAL during service startup"
            break
        fi
        sleep 2
    done

    if [ -z "$FAIL_REASON" ]; then
        if $BWM_OK; then echo "  BWM: running"; fi
        if $SHELL_OK; then
            echo "Phase 2: PASS (shell spawned)"
        else
            FAIL_REASON="Phase 2 timeout: shell not detected"
        fi
    fi
fi

# --- Phase 3: Sustained operation under GPU load (15s) ---
# This catches the crashes that only manifest under sustained GPU rendering
# (e.g., bounce demo, btop updating, BWM rendering test progress).
if [ -z "$FAIL_REASON" ] && ! $BOOT_TESTS_ONLY; then
    echo ""
    echo "Phase 3: Sustained operation soak (15 seconds)..."
    for check in $(seq 1 5); do
        sleep 3

        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 3: $FATAL during sustained operation (check $check/5)"
            break
        fi

        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 3: QEMU exited unexpectedly (check $check/5)"
            break
        fi

        CURR_LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null | tr -d ' ')
        echo "  Check $check/5: OK (${CURR_LINES:-0} lines, QEMU alive)"
    done

    if [ -z "$FAIL_REASON" ]; then
        echo "Phase 3: PASS (stable for 15s)"
    fi
fi

# --- Cleanup QEMU ---
kill $QEMU_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true
unset QEMU_PID

# --- Post-run forbidden-pattern scan over the WHOLE serial ---
#
# Phase 1a3's strand check runs inside a poll loop that breaks the instant both
# green patterns match, so a strand first observed after that instant was never
# scored. The census is cumulative and emitted every 5s for the life of the
# boot, so the only scan that can see a late strand is one that runs over the
# finished file. This adds failure conditions; it relaxes none, and it runs even
# when every phase above passed.
if [ -f "$OUTPUT_DIR/serial.txt" ]; then
    if grep -qE '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        STRAND_LINE=$(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$OUTPUT_DIR/serial.txt" | tail -1)
        FAIL_REASON="${FAIL_REASON:-Post-run scan: scheduler strand census reported stranded work ($STRAND_LINE)}"
    fi
    if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        STRAND_LINE=$(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$OUTPUT_DIR/serial.txt" | tail -1)
        FAIL_REASON="${FAIL_REASON:-Post-run scan: scheduler strand injection oracle reported stranded work ($STRAND_LINE)}"
    fi
fi

# --- Phase 5: refusal-drain oracle legs (feature profile, own kernel build) ---
#
# Legs G and H are feature-gated (`resume_pc_foreign_oracle`), so they are not in
# this boot's kernel. Running them here is what makes them gate evidence instead
# of something a human remembers to run. The sub-gate builds into its own target
# dir, so the kernel this script just booted is left untouched.
echo ""
echo "Phase 5: refusal-drain oracle legs (leg G foreign record, leg H departure guard)..."
if ! bash "$SCRIPT_DIR/run-aarch64-refusal-drain-gate.sh"; then
    FAIL_REASON="${FAIL_REASON:-Phase 5: refusal-drain oracle legs failed}"
else
    echo "Phase 5: PASS"
fi

# --- Report ---
echo ""
TOTAL_LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null | tr -d ' ')
TOTAL_LINES=${TOTAL_LINES:-0}

if [ -z "$FAIL_REASON" ]; then
    echo "========================================="
    if $BOOT_TESTS_ONLY; then
        echo "ARM64 BOOT TESTS: PASSED"
    else
        echo "ARM64 FULL SYSTEM TEST: PASSED"
    fi
    echo "========================================="
    echo "Tests: ${TESTS_PASSED}/${TESTS_TOTAL} passed"
    if ! $BOOT_TESTS_ONLY; then
        echo "Stability: 15s soak clean"
    fi
    echo "Serial: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    exit 0
else
    echo "========================================="
    echo "ARM64 FULL SYSTEM TEST: FAILED"
    echo "========================================="
    echo "Reason: $FAIL_REASON"
    if [ -n "$TEST_FAILURES" ]; then
        echo ""
        echo "Failed tests:"
        echo "$TEST_FAILURES"
    fi
    echo ""
    echo "Serial: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    echo ""
    echo "Last 20 lines of serial output:"
    tail -20 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
    exit 1
fi
