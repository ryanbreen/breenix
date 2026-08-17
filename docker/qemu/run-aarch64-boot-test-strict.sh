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
# construct_residual is the counted frame residue of the two construction-failure arms read off a measured green run, and it is architecture-specific (4 on x86, 2 on aarch64) because the two page-table constructors record different table-frame counts.
INIT_DESIGNATION_ORACLE_LITERAL='[INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=2:construct_residual=2:refused=4:accepted=1:published=1:retired=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]'

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find ext2 disk (required for userspace)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
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
    local crash_type

    if [ ! -f "$serial_file" ]; then
        echo "Userspace not detected"
        return 1
    fi
    if crash_type=$(check_crash_markers "$serial_file"); then
        echo "$crash_type"
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
    if ! grep -qF "[INIT_DESIGNATION:aarch64:designated_pid=1:reserved_collisions=0]" "$serial_file" 2>/dev/null; then
        echo "Init designation marker missing"
        return 1
    fi
    if ! grep -qF -x "$INIT_DESIGNATION_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Init designation oracle counter marker missing"
        return 1
    fi
    return 0
}

# Scoring-only entry point: score an already-captured serial log and exit. This
# exists so the scoring rules can be exercised against a preserved serial without
# booting, which is how the "a serial containing every success marker scores as a
# success" property is proven.
if [ -n "${BREENIX_STRICT_SCORE_ONLY:-}" ]; then
    if SCORE_REASON=$(score_serial "$BREENIX_STRICT_SCORE_ONLY"); then
        echo "SCORE: PASS - $BREENIX_STRICT_SCORE_ONLY"
        exit 0
    else
        echo "SCORE: FAIL - $SCORE_REASON ($BREENIX_STRICT_SCORE_ONLY)"
        exit 1
    fi
fi

report_failure() {
    local iteration="$1"
    local reason="$2"
    local serial_file="$3"
    local failure_dir="/tmp/breenix_aarch64_strict_failures"
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
    echo "  [FAIL] Boot $iteration: $reason ($lines lines); serial: $preserved_serial"
}

run_single_test() {
    local iteration=$1
    local OUTPUT_DIR="/tmp/breenix_aarch64_strict_$iteration"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 20s timeout.
    # Breenix ARM64 expects a GICv3 CPU interface, matching Parallels.
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
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

    # Wait for userspace liveness AND exec smoke completion (18s max, checking every 1.5s)
    # Accept any of these as the liveness condition:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    #   "[bcheck] Complete:" - bcheck self-test suite finished (headless/no-VirGL mode)
    #   "[heartbeat]" - the default ARM64 init service executed in userspace
    # Also require "[EXEC_SMOKE:TARGET_OK]" as the exec completion condition.
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local USERSPACE_DETECTED=false
    local EXEC_SMOKE_COMPLETE=false
    local CRASH_TYPE=""
    # Named POLL, not i: the caller's loop variable is also i, and an unscoped
    # inner i made the summary report the poll counter instead of the boot number.
    local POLL
    for POLL in $(seq 1 12); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -qE "(breenix>|bsh |\[bwm\] Display:|\[bcheck\] Complete:|\[heartbeat\])" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                USERSPACE_DETECTED=true
            fi
            if grep -qF "[EXEC_SMOKE:TARGET_OK]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                EXEC_SMOKE_COMPLETE=true
            fi
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
            if $USERSPACE_DETECTED && $EXEC_SMOKE_COMPLETE; then
                break
            fi
        fi
        sleep 1.5
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    # The poll booleans above are a stop condition, not a verdict. Score the boot
    # from the serial file QEMU actually left behind.
    local FAIL_DETAIL
    if FAIL_DETAIL=$(score_serial "$OUTPUT_DIR/serial.txt"); then
        echo "  [OK] Boot $iteration: SUCCESS"
        return 0
    fi

    report_failure "$iteration" "$FAIL_DETAIL" "$OUTPUT_DIR/serial.txt"
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
    echo "Serial output from failed boots can be found in /tmp/breenix_aarch64_strict_N/"
    exit 1
fi
