#!/bin/bash
# Run ARM64 kernel with userspace binaries natively (no Docker)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$BREENIX_ROOT"

# #826/#834/R181: this script's qemu-system-aarch64 boot runs behind the
# host-wide lock in docker/qemu/lib/qemu-host-lock.sh -- #834 extends that
# lock's coverage from docker/qemu/*.sh (its original #826/R181 scope) to
# scripts/ as well.
# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$BREENIX_ROOT/docker/qemu/lib/qemu-host-lock.sh"

# Build ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel..."
    cargo build --release --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc \
        -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64
fi

# Build ARM64 userspace if needed
USERSPACE_DIR="$BREENIX_ROOT/userspace/programs/aarch64"
if [ ! -d "$USERSPACE_DIR" ] || [ -z "$(ls -A $USERSPACE_DIR/*.elf 2>/dev/null)" ]; then
    echo "Building ARM64 userspace binaries..."
    cd "$BREENIX_ROOT/userspace/programs"
    ./build.sh --arch aarch64
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

# #834: this is an interactive display session (Ctrl-A X exits QEMU), but
# it still takes the host-wide lock -- not exempt just because a human, not
# a gate, is driving it. `exec` is dropped in favor of a plain foreground
# launch: replacing this shell's own process image would discard the
# qemu_host_lock_acquire EXIT trap before it could release the lock, since
# there would be no bash process left to run it.
qemu_host_lock_acquire
qemu-system-aarch64 \
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
