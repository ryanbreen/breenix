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
# a gate, is driving it. `exec` is dropped because replacing this shell's
# own process image would discard the qemu_host_lock_acquire EXIT trap
# before it could release the lock. A plain foreground launch (no `&`) is
# not a safe substitute for `exec`, though: a SIGTERM/SIGINT delivered to
# just this script's own PID does not propagate to a foreground child on
# its own, so the EXIT trap would run and release the lock while QEMU kept
# running, orphaned and untracked -- reproducing the exact unserialized
# double-boot contention this lock exists to prevent (#834 fix-round F1,
# 2026-09-05). QEMU is backgrounded instead, with its PID handed to
# qemu_host_lock_track_pid so the lock's own EXIT trap kills it before
# releasing the lock on that path, then `wait`ed on so this script still
# blocks in the foreground exactly as before. `0<&0` is required on the
# backgrounded launch: bash redirects a backgrounded command's stdin from
# /dev/null unless the command carries its own explicit stdin redirection,
# and this session's serial monitor needs this script's own stdin attached.
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
    -kernel "$KERNEL" \
    0<&0 &
QEMU_PID=$!
qemu_host_lock_track_pid "$QEMU_PID"
wait "$QEMU_PID"
