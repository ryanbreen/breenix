#!/bin/bash
# Run Breenix ARM64 kernel with graphics in QEMU
#
# Usage: ./scripts/run-arm64-graphics.sh [release|debug]
#
# This version enables VirtIO GPU for graphical output.
# Use Cocoa display on macOS, or SDL on Linux.
# If a test disk exists, it will be loaded for userspace programs.

set -e

BUILD_TYPE="${1:-release}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ "$BUILD_TYPE" = "debug" ]; then
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/debug/kernel-aarch64"
else
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
fi

# Check if kernel exists
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel ($BUILD_TYPE)..."
    if [ "$BUILD_TYPE" = "debug" ]; then
        cargo build --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    else
        cargo build --release --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    fi
fi

# Prefer ext2 disk image (for /bin/init_shell); fall back to BXTEST disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
TEST_DISK="$BREENIX_ROOT/target/aarch64_test_binaries.img"
DISK_OPTS=""
if [ -f "$EXT2_DISK" ]; then
    echo "Found ext2 disk image: $EXT2_DISK"
    DISK_OPTS="-device virtio-blk-device,drive=ext2disk \
        -blockdev driver=file,node-name=ext2file,filename=$EXT2_DISK \
        -blockdev driver=raw,node-name=ext2disk,file=ext2file"
elif [ -f "$TEST_DISK" ]; then
    echo "Found BXTEST disk with userspace binaries: $TEST_DISK"
    DISK_OPTS="-device virtio-blk-device,drive=testdisk \
        -blockdev driver=file,node-name=testfile,filename=$TEST_DISK \
        -blockdev driver=raw,node-name=testdisk,file=testfile"
else
    echo "No ext2/BXTEST disk found"
    DISK_OPTS="-device virtio-blk-device,drive=empty \
        -blockdev driver=file,node-name=nullfile,filename=/dev/null \
        -blockdev driver=raw,node-name=empty,file=nullfile"
fi

echo ""
echo "========================================="
echo "  Breenix ARM64 Kernel"
echo "========================================="
echo "Kernel: $KERNEL"
if [ -f "$EXT2_DISK" ]; then
    echo "Ext2 disk: $EXT2_DISK"
elif [ -f "$TEST_DISK" ]; then
    echo "Test disk: $TEST_DISK"
fi
echo ""
echo "Press Ctrl-A X to exit QEMU"
echo ""

# Determine display backend based on OS
case "$(uname)" in
    Darwin)
        DISPLAY_OPT="-display cocoa,show-cursor=on"
        ;;
    *)
        DISPLAY_OPT="-display sdl"
        ;;
esac

# Run QEMU with:
# - VirtIO GPU device (MMIO) for framebuffer
# - Serial output to terminal (mon:stdio)
# - VirtIO block device (MMIO) for test disk
# - VirtIO net device (MMIO)
# - VirtIO keyboard device (MMIO) for keyboard input
# NOTE: Use -device virtio-*-device (MMIO) not virtio-*-pci
exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -serial mon:stdio \
    -device virtio-gpu-device \
    $DISPLAY_OPT \
    $DISK_OPTS \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-keyboard-device \
    -kernel "$KERNEL"
