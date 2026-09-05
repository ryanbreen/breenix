#!/bin/bash
# Automated home of the refusal-drain oracle legs: G proves a foreign record is
# reported and never acted on; H proves a victim dispatched after the record is
# published is never terminated, dequeued or unpublished. The gate boots a
# boot_tests kernel, so it also rejects boot-test failures and a failing block-
# EINTR oracle. This exists so those legs run without a human, and it builds into
# its own target dir so the shared target/aarch64-breenix-kernel kernel other
# gates boot is never replaced by an oracle-feature build.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE_TARGET_DIR="$BREENIX_ROOT/target/refusal-drain-gate"
# #825: fixed by default so the serial lands where every runbook expects it.
# It is overridable because the directory is rm -rf'd on entry: a second copy
# of this gate running on the same host (another worktree, a parallel soak)
# otherwise deletes and rewrites the first one's serial mid-boot, and the
# first run then reports the *second* run's kernel as its own result --
# exactly the collision #825 reports for this gate's strict and prod-profile
# siblings. BREENIX_REFUSAL_DRAIN_OUTPUT_DIR (this gate's own pre-existing
# override, kept for any caller already setting it) takes priority;
# otherwise this now falls back to the shared BREENIX_GATE_TMP convention
# PR #801 gave the x86 gate scripts for #797, rather than a bare /tmp
# literal a caller had no shared knob to redirect.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac
OUTPUT_DIR="${BREENIX_REFUSAL_DRAIN_OUTPUT_DIR:-$BREENIX_GATE_TMP/breenix_aarch64_refusal_drain_gate}"

# This proves init's block EINTR oracle ran during the boot-tests sequence.
BLOCK_EINTR_ORACLE_LITERAL='[BLOCK_EINTR_ORACLE:'
# This proves init's block EINTR oracle did not self-report a failure.
BLOCK_EINTR_ORACLE_FAIL_LITERAL='[BLOCK_EINTR_ORACLE:FAIL'
# #568: this proves init's blocking-poll-on-connected-TCP oracle ran.
POLL_TCP_ORACLE_LITERAL='[POLL_TCP_ORACLE:'
# #568: this proves that oracle did not self-report a failure. Both halves are
# needed: presence alone passes a boot whose verdict was FAIL, and the FAIL
# grep alone passes a boot where the program never started.
POLL_TCP_ORACLE_FAIL_LITERAL='[POLL_TCP_ORACLE:FAIL'
# #693: the kernel's own lost-readiness report from sys_poll. Pinned separately
# from the oracle verdict because it is a statement about kernel state, not a
# userspace program's opinion.
POLL_TCP_READY_LOST_LITERAL='[POLL_TCP_READY_LOST]'
# This proves the complete boot-tests sequence reported success.
BOOT_TESTS_PASS_LITERAL='[BOOT_TESTS:PASS]'

cleanup() {
    if [ -n "${QEMU_PID:-}" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    echo "Serial: $OUTPUT_DIR/serial.txt"
}
trap cleanup EXIT

(cd "$BREENIX_ROOT" && CARGO_TARGET_DIR="$GATE_TARGET_DIR" cargo build --release \
    --features boot_tests,resume_pc_foreign_oracle \
    --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64)
KERNEL="$GATE_TARGET_DIR/aarch64-breenix-kernel/release/kernel-aarch64"

"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

timeout 200 qemu-system-aarch64 \
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

LEG_G_MARKER='[RESUME_PC_FOREIGN_ORACLE:aarch64:leg=G:'
LEG_H_MARKER='[RESUME_PC_DRAIN_DEPARTURE_ORACLE:aarch64:leg=H:'
LEG_G_LINE=""
LEG_H_LINE=""
BOOT_TESTS_PASS_LINE=""
BLOCK_EINTR_ORACLE_LINE=""
POLL_TCP_ORACLE_LINE=""
FATAL_LINE=""

for _ in $(seq 1 180); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        LEG_G_LINE=$(grep -F "$LEG_G_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        LEG_H_LINE=$(grep -F "$LEG_H_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        BOOT_TESTS_PASS_LINE=$(grep -F "$BOOT_TESTS_PASS_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        BLOCK_EINTR_ORACLE_LINE=$(grep -F "$BLOCK_EINTR_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        POLL_TCP_ORACLE_LINE=$(grep -F "$POLL_TCP_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        FATAL_LINE=$(grep -iE 'KERNEL PANIC|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        if [ -n "$FATAL_LINE" ]; then
            echo "ARM64 REFUSAL DRAIN GATE: FAILED"
            echo "$FATAL_LINE"
            exit 1
        fi
        if grep -qF '[BOOT_TESTS:FAIL' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOOT_TEST_FAIL_LINE=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -1 || true)
            echo "ARM64 REFUSAL DRAIN GATE: FAILED"
            echo "boot test failure: ${BOOT_TEST_FAIL_LINE:-[TEST:<missing>:FAIL:<missing>]}"
            exit 1
        fi
        if [ -n "$LEG_G_LINE" ] && [ -n "$LEG_H_LINE" ] && \
            [ -n "$BOOT_TESTS_PASS_LINE" ] && [ -n "$BLOCK_EINTR_ORACLE_LINE" ] && \
            [ -n "$POLL_TCP_ORACLE_LINE" ]; then
            break
        fi
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done

if ! grep -qF "$BOOT_TESTS_PASS_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Missing marker: $BOOT_TESTS_PASS_LITERAL"
    exit 1
fi
if ! grep -qF "$BLOCK_EINTR_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Missing marker: $BLOCK_EINTR_ORACLE_LITERAL"
    exit 1
fi
if grep -qF "$BLOCK_EINTR_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Block EINTR oracle reported failure: $BLOCK_EINTR_ORACLE_FAIL_LITERAL"
    exit 1
fi
if ! grep -qF "$POLL_TCP_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Missing marker: $POLL_TCP_ORACLE_LITERAL"
    exit 1
fi
if grep -qF "$POLL_TCP_READY_LOST_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Kernel reported a lost TCP readiness publication (#693): $(grep -aF "$POLL_TCP_READY_LOST_LITERAL" "$OUTPUT_DIR/serial.txt" | tail -1)"
    exit 1
fi
if grep -qF "$POLL_TCP_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Poll TCP oracle reported failure: $(grep -aF "$POLL_TCP_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" | tail -1)"
    exit 1
fi

if [ -z "$LEG_G_LINE" ]; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Missing marker: $LEG_G_MARKER"
    exit 1
fi
if [ -z "$LEG_H_LINE" ]; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "Missing marker: $LEG_H_MARKER"
    exit 1
fi
if ! echo "$LEG_G_LINE" | grep -q ':PASS]$'; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "$LEG_G_LINE"
    exit 1
fi
if ! echo "$LEG_H_LINE" | grep -q ':PASS]$'; then
    echo "ARM64 REFUSAL DRAIN GATE: FAILED"
    echo "$LEG_H_LINE"
    exit 1
fi

echo "ARM64 REFUSAL DRAIN GATE: PASSED"
echo "$LEG_G_LINE"
echo "$LEG_H_LINE"
echo "$BOOT_TESTS_PASS_LINE"
echo "$BLOCK_EINTR_ORACLE_LINE"
exit 0
