#!/bin/bash
# Run ARM64 kernel test in Docker
# Usage: ./run-aarch64-test.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix.json -p kernel --features aarch64-qemu --bin kernel-aarch64"
    exit 1
fi

echo "Running ARM64 kernel test in Docker..."
echo "Kernel: $KERNEL"

# Create output directory
OUTPUT_DIR="/tmp/breenix_aarch64_1"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Build the ARM64 Docker image if not exists
if ! docker images breenix-qemu-aarch64 --format "{{.Repository}}" | grep -q breenix-qemu-aarch64; then
    echo "Building ARM64 Docker image..."
    docker build -t breenix-qemu-aarch64 -f "$SCRIPT_DIR/Dockerfile.aarch64" "$SCRIPT_DIR"
fi

echo "Starting QEMU ARM64..."

# Run QEMU with ARM64 virt machine
# -M virt: Standard ARM virtual machine
# -cpu cortex-a72: 64-bit ARMv8-A CPU
# -kernel: Load ELF directly (QEMU handles this)
# -m 512: 512MB RAM
# -serial: Serial output to file
docker run --rm \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$OUTPUT_DIR:/output" \
    breenix-qemu-aarch64 \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512 \
        -kernel /breenix/kernel \
        -display none \
        -no-reboot \
        -serial file:/output/serial.txt \
        &

QEMU_PID=$!

# Wait for output (30 second timeout)
echo "Waiting for kernel output (30s timeout)..."
FOUND=false
for i in $(seq 1 30); do
    if [ -f "$OUTPUT_DIR/serial.txt" ] && [ -s "$OUTPUT_DIR/serial.txt" ]; then
        # Check for any meaningful output
        if grep -qE "(Breenix|kernel|panic|Hello)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
    fi
    sleep 1
done

# Show output
echo ""
echo "========================================="
echo "Serial Output:"
echo "========================================="
if [ -f "$OUTPUT_DIR/serial.txt" ]; then
    cat "$OUTPUT_DIR/serial.txt"
else
    echo "(no output)"
fi
echo "========================================="

# Cleanup
docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true

if $FOUND; then
    echo "ARM64 kernel produced output!"
    exit 0
else
    echo "Timeout or no meaningful output"
    exit 1
fi
