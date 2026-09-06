#!/bin/bash
# Run ARM64 kernel with userspace binaries in Docker
# Usage: ./run-aarch64-userspace.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/R181: serializes qemu-system-aarch64 boots host-wide. This script is
# Docker-wrapped -- see run-aarch64-test.sh's identical comment for why the
# host-side lock still applies (the `docker run` CLI blocks on the host
# with the qemu-system-aarch64 token in its own argv).
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #825: this script's OUTPUT_DIR is a HOST path that is rm -rf'd, mkdir'd and
# then bind-mounted into the container, so the same-host collision #825
# reports applies here despite the QEMU process itself running inside
# Docker -- and this script shares the identical literal
# /tmp/breenix_aarch64_1 path with run-aarch64-test.sh, so even two
# DIFFERENT scripts running at once collide. Defaulting to /tmp keeps a caller that
# leaves it unset byte-identical; a concurrent-lane launcher sets this to a
# per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac

# #829: CONTAINER_NAME is unique to this invocation (this script's own
# PID), so the cleanup below targets the exact container this run started
# instead of matching by ancestor image -- an ancestor-image filter can
# kill a DIFFERENT running container from the same image, including one a
# concurrent invocation of this same script (or of run-aarch64-test.sh,
# which shares the image) legitimately owns. The trap fires on each exit
# path this script can take (normal completion, an early `exit 1`, or a
# signal) -- not only the
# bottom-of-script cleanup line the pre-#829 version relied on -- so a
# `set -e` exit partway through the boot-poll loop below still stops this
# invocation's own container instead of leaking it.
CONTAINER_NAME="breenix-aarch64-userspace-$$"
docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
_cleanup_aarch64_userspace_container() {
    # TERM then KILL after a bounded wait -- docker stop's own contract:
    # SIGTERM to the container's PID 1, SIGKILL only if it has not exited
    # within the timeout.
    docker stop -t 5 "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap _cleanup_aarch64_userspace_container EXIT

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find or create ARM64 ext2 disk.
#
# #850: this used to pin a hardcoded 8MB size (both to pass --size and to
# decide "size doesn't match, recreate"), which stopped fitting the real
# aarch64 userspace binary + font payload (measured ~62MB) long before this
# bug was filed -- create_ext2_disk.sh's own --size guard now rejects an
# 8MB request outright rather than failing partway through the copy with
# ENOSPC. The other 12 of 13 real call sites in this tree pass no --size
# at all and just check whether the image already exists (census in
# tests/ext2_disk_size_structure.rs); this caller now does the same instead
# of carrying its own size-tracking magic number that can drift out of
# sync with the payload again.
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Creating ARM64 ext2 disk image..."
    "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64

    if [ ! -f "$EXT2_DISK" ]; then
        echo "Error: Failed to create ext2 disk image at $EXT2_DISK"
        exit 1
    fi
fi

echo "Running ARM64 kernel with userspace..."
echo "Kernel: $KERNEL"
echo "Ext2 disk: $EXT2_DISK"

# Create output directory
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_1"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Build the ARM64 Docker image if not exists
if ! docker images breenix-qemu-aarch64 --format "{{.Repository}}" | grep -q breenix-qemu-aarch64; then
    echo "Building ARM64 Docker image..."
    docker build -t breenix-qemu-aarch64 -f "$SCRIPT_DIR/Dockerfile.aarch64" "$SCRIPT_DIR"
fi

echo "Starting QEMU ARM64 with VirtIO devices..."

# Create writable copy of ext2 disk to allow filesystem write tests
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

# Run QEMU with ARM64 virt machine and VirtIO devices
# QEMU virt machine provides 32 VirtIO MMIO slots at:
#   0x0a000000 + n*0x200  for n=0..31
# Devices are assigned from slot 31 downward.
# Use writable disk copy (no readonly=on) to allow filesystem writes
qemu_host_lock_acquire
docker run --rm \
    --name "$CONTAINER_NAME" \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$EXT2_WRITABLE:/breenix/ext2.img" \
    -v "$OUTPUT_DIR:/output" \
    breenix-qemu-aarch64 \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512 \
        -kernel /breenix/kernel \
        -drive if=none,id=ext2disk,format=raw,file=/breenix/ext2.img \
        -device virtio-blk-device,drive=ext2disk \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -display none \
        -no-reboot \
        -serial file:/output/serial.txt \
        &

QEMU_PID=$!
# F2: registers the docker run client with the lock's own EXIT trap (see
# lib/qemu-host-lock.sh) so a SIGTERM/SIGINT delivered to just this process
# still stops the container instead of orphaning it with the lock free.
qemu_host_lock_track_pid "$QEMU_PID"

# Wait for output (60 second timeout)
echo "Waiting for kernel output (60s timeout)..."
FOUND=false
for i in $(seq 1 60); do
    if [ -f "$OUTPUT_DIR/serial.txt" ] && [ -s "$OUTPUT_DIR/serial.txt" ]; then
        # Check for boot complete or userspace output
        if grep -qE "(Boot Complete|Hello|userspace|fork)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
    fi
    sleep 1
done

# Wait a bit more for any additional output
sleep 2

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

# Cleanup: the container this invocation started is stopped by
# _cleanup_aarch64_userspace_container above (chained onto this script's
# own EXIT trap by qemu_host_lock_acquire), not by an ancestor-image match.
qemu_host_lock_release

if $FOUND; then
    echo "ARM64 kernel produced output!"
    exit 0
else
    echo "Timeout or no meaningful output"
    exit 1
fi
