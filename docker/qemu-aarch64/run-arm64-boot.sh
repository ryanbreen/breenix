#!/bin/bash
# Run ARM64 kernel boot test in Docker
# Usage: ./run-arm64-boot.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# #826/#834/R181: this script's qemu-system-aarch64 boot runs behind the
# host-wide lock in docker/qemu/lib/qemu-host-lock.sh. This script is
# Docker-wrapped -- see docker/qemu/run-aarch64-test.sh's identical comment
# for why the host-side lock still applies (the `docker run` CLI blocks on
# the host with the qemu-system-aarch64 token in its own argv). #834
# extends this lock's coverage from docker/qemu/*.sh (its original
# #826/R181 scope, which this sibling directory falls outside of) to the
# rest of the tree that launches qemu-system-aarch64.
# shellcheck source=../qemu/lib/qemu-host-lock.sh
source "$SCRIPT_DIR/../qemu/lib/qemu-host-lock.sh"

# #834 fix-round F3 (2026-09-05): OUTPUT_DIR is a HOST path this script
# rm -rf's, mkdir's, and bind-mounts into the container, so it collides with
# any other concurrent invocation the same way #825 already reports for
# docker/qemu/run-aarch64-test.sh's OUTPUT_DIR (see that file's own #825
# comment for the identical BREENIX_GATE_TMP shape). Defaulting to /tmp
# keeps a caller that leaves it unset byte-identical; a concurrent-lane
# launcher sets this to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac

# Find the ARM64 kernel binary
KERNEL_BIN="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL_BIN" ]; then
    echo "Error: ARM64 kernel not found. Build with:"
    echo "  cd \"$BREENIX_ROOT\" && cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem --features testing -p kernel --bin kernel-aarch64"
    exit 1
fi

echo "Running ARM64 boot test in Docker..."
echo "Kernel: $KERNEL_BIN"

# Create output directory
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_arm64_boot"
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
# #834 fix-round F2 (2026-09-05): CONTAINER_NAME is unique to this
# invocation (this script's own PID), so the cleanup `docker kill` below
# can target the exact container this run started instead of matching by
# ancestor image -- an ancestor-image filter can kill a DIFFERENT running
# container from that image, including one a concurrent invocation of this
# same script (or of another script sharing the image) legitimately owns.
CONTAINER_NAME="breenix-arm64-boot-$$"
docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
qemu_host_lock_acquire
docker run --rm \
    --name "$CONTAINER_NAME" \
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
# F2: registers the docker run client with the lock's own EXIT trap (see
# docker/qemu/lib/qemu-host-lock.sh) so a SIGTERM/SIGINT delivered to just
# this process during the poll below still stops the container instead of
# orphaning it with the lock free.
qemu_host_lock_track_pid "$QEMU_PID"

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

# Cleanup: targets this invocation's own container by name (see
# CONTAINER_NAME above), not the whole image's running containers.
docker kill "$CONTAINER_NAME" 2>/dev/null || true

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
