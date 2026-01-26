#!/bin/bash
# Run ARM64 kernel with userspace binaries natively (no Docker)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Build ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-unknown-none/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel..."
    cargo build --release --target aarch64-unknown-none -p kernel --bin kernel-aarch64
fi

# Build ARM64 userspace if needed
USERSPACE_DIR="$BREENIX_ROOT/userspace/tests/aarch64"
if [ ! -d "$USERSPACE_DIR" ] || [ -z "$(ls -A $USERSPACE_DIR/*.elf 2>/dev/null)" ]; then
    echo "Building ARM64 userspace binaries..."
    cd "$BREENIX_ROOT/userspace/tests"
    ./build-aarch64.sh
    cd "$BREENIX_ROOT"
fi

# Create test disk if needed
TEST_DISK="$BREENIX_ROOT/target/aarch64_test_binaries.img"
if [ ! -f "$TEST_DISK" ]; then
    echo "Creating ARM64 test disk..."
    cargo run -p xtask -- create-test-disk-aarch64
fi

echo ""
echo "========================================="
echo "  Breenix ARM64 with Userspace"
echo "========================================="
echo "Kernel: $KERNEL"
echo "Test disk: $TEST_DISK"
echo ""
echo "Press Ctrl-A X to exit QEMU"
echo ""

# Determine display backend
case "$(uname)" in
    Darwin) DISPLAY_OPT="-display cocoa,show-cursor=on" ;;
    *)      DISPLAY_OPT="-display sdl" ;;
esac

exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -serial mon:stdio \
    -device virtio-gpu-device \
    $DISPLAY_OPT \
    -device virtio-blk-device,drive=testdisk \
    -blockdev driver=file,node-name=testfile,filename="$TEST_DISK" \
    -blockdev driver=raw,node-name=testdisk,file=testfile \
    -device virtio-keyboard-device \
    -kernel "$KERNEL"
