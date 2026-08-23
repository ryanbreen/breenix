#!/bin/bash
# Automated home of the refusal-drain oracle legs: G proves a foreign record is
# reported and never acted on; H proves a victim dispatched after the record is
# published is never terminated, dequeued or unpublished. This exists so those
# legs run without a human, and it builds into its own target dir so the shared
# target/aarch64-breenix-kernel kernel other gates boot is never replaced by an
# oracle-feature build.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE_TARGET_DIR="$BREENIX_ROOT/target/refusal-drain-gate"
OUTPUT_DIR="/tmp/breenix_aarch64_refusal_drain_gate"

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

timeout 120 qemu-system-aarch64 \
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
FATAL_LINE=""

for _ in $(seq 1 100); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        LEG_G_LINE=$(grep -F "$LEG_G_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        LEG_H_LINE=$(grep -F "$LEG_H_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        FATAL_LINE=$(grep -iE 'KERNEL PANIC|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        if [ -n "$FATAL_LINE" ]; then
            echo "ARM64 REFUSAL DRAIN GATE: FAILED"
            echo "$FATAL_LINE"
            exit 1
        fi
        if [ -n "$LEG_G_LINE" ] && [ -n "$LEG_H_LINE" ]; then
            break
        fi
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done

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
exit 0
