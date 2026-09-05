#!/bin/bash
# Run ARM64 kernel test in Docker
# Usage: ./run-aarch64-test.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/R181: this script's qemu-system-aarch64 boot runs behind the
# host-wide lock in lib/qemu-host-lock.sh. This script is Docker-wrapped --
# the actual qemu-system-aarch64 process runs inside Docker's own Linux VM,
# invisible to this host's pgrep -- but the `docker run` CLI invocation
# blocks on the host with that exact token in its own argv for as long as
# the container runs, so qemu_host_lock_count() (a pgrep -f) still sees it,
# and holding the lock around the `docker run` call below still serializes
# this script against the native-QEMU gates.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #825: this script's OUTPUT_DIR is a HOST path that is rm -rf'd, mkdir'd and
# then bind-mounted into the container (-v "$OUTPUT_DIR:/output" below), so
# the same-host collision #825 reports applies here despite the QEMU process
# itself running inside Docker -- and this script shares the identical
# literal /tmp/breenix_aarch64_1 path with run-aarch64-userspace.sh, so even
# two DIFFERENT scripts running at once collide. Defaulting to /tmp keeps
# a caller that leaves it unset byte-identical; a concurrent-lane launcher sets this
# to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

echo "Running ARM64 kernel test in Docker..."
echo "Kernel: $KERNEL"

# Create output directory
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_1"
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
qemu_host_lock_acquire
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
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
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
qemu_host_lock_release

if $FOUND; then
    echo "ARM64 kernel produced output!"
    exit 0
else
    echo "Timeout or no meaningful output"
    exit 1
fi
