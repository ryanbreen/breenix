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

# Find ext2 disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

OUTPUT_DIR="/tmp/breenix_aarch64_full_test"
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

if $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    if ! grep -Fq '[CREATING_DISPATCH_ORACLE:aarch64:injected=1:refused_via_dispatch=1:requeue_retried=1:dispatched_after_publish=1:balance=0:leaf_residual=16:user_stack_residual=16]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FAIL_REASON="Phase 1: missing creating-dispatch refusal oracle marker"
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
        else
            echo "Phase 1c: PASS"
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
        if grep -qE "\[pty\] Unlocked PTY" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
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
