#!/bin/bash
# Run ARM64 kernel boot test in Docker
# Usage: ./run-arm64-boot.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel binary
KERNEL_BIN="$BREENIX_ROOT/target/aarch64-unknown-none/release/kernel-aarch64"
if [ ! -f "$KERNEL_BIN" ]; then
    echo "Error: ARM64 kernel not found. Build with:"
    echo "  cargo build --release --target aarch64-unknown-none --features testing -p kernel --bin kernel-aarch64"
    exit 1
fi

echo "Running ARM64 boot test in Docker..."
echo "Kernel: $KERNEL_BIN"

# Create output directory
OUTPUT_DIR="/tmp/breenix_arm64_boot"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Build Docker image if needed
docker build -q -t breenix-qemu-aarch64 "$SCRIPT_DIR" > /dev/null

# Run QEMU in Docker
# Using virt machine with:
# - 512MB RAM
# - 1 CPU
# - PL011 UART for serial output
# - No graphics
docker run --rm \
    -v "$KERNEL_BIN:/breenix/kernel.elf:ro" \
    -v "$OUTPUT_DIR:/output" \
    breenix-qemu-aarch64 \
    timeout 30 qemu-system-aarch64 \
        -machine virt \
        -cpu cortex-a72 \
        -m 512 \
        -kernel /breenix/kernel.elf \
        -nographic \
        -serial file:/output/serial.txt \
        -d guest_errors,unimp \
        -D /output/qemu_debug.txt \
        -no-reboot \
    &
QEMU_PID=$!

# Wait for output or timeout
echo "Waiting for kernel output (30s timeout)..."
FOUND=false
for i in $(seq 1 30); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        if grep -q "Breenix" "$OUTPUT_DIR/serial.txt" 2>/dev/null || \
           grep -q "kernel_main" "$OUTPUT_DIR/serial.txt" 2>/dev/null || \
           grep -q "Hello" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
    fi
    sleep 1
done

# Cleanup
docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true

# Check results
echo ""
echo "========================================="
if $FOUND; then
    echo "ARM64 BOOT: PASS"
    echo "Kernel produced output!"
    echo ""
    echo "Serial output:"
    cat "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -50
else
    echo "ARM64 BOOT: FAIL/TIMEOUT"
    echo ""
    echo "Serial output (if any):"
    cat "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -20 || echo "(no output)"
    echo ""
    echo "QEMU debug log:"
    cat "$OUTPUT_DIR/qemu_debug.txt" 2>/dev/null | head -20 || echo "(no debug log)"
    exit 1
fi
echo "========================================="
