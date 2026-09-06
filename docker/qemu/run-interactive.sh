#!/bin/bash
# Run QEMU interactively in Docker with VNC display
# Usage: ./run-interactive.sh
#
# Automatically opens TigerVNC connected to the QEMU display.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/#834/#865/R181: this script's qemu-system-x86_64 boot runs behind the
# host-wide lock in lib/qemu-host-lock.sh (one lock domain per QEMU binary),
# closing the gap the F1-review comment below used to disclose ("qemu-system-x86_64
# has no such lock, and this script does not add one") -- it now cooperates
# with a concurrent x86 gate lane the same way run-aarch64-interactive.sh
# already cooperates with #826's own aarch64 lock.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"

# Build Docker image if needed
IMAGE_NAME="breenix-qemu"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building Docker image..."
    docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"
fi

# #849/F1-review: CONTAINER_NAME is unique to this invocation (this
# script's own PID). This replaces the previous ancestor-image-filtered
# "kill any existing container" preflight (docker ps -q --filter
# ancestor="$IMAGE_NAME"), which could kill a DIFFERENT concurrent
# invocation's own container from the same image -- not only a leftover
# from a crashed earlier run of this exact script, which is the only
# container the `docker rm -f "$CONTAINER_NAME"` line below can now ever
# remove.
#
# #865 update: this is now the SAME tradeoff as #829's aarch64 precedent
# (run-aarch64-interactive.sh), not a different one -- both scripts
# cooperate with the host-wide lock in lib/qemu-host-lock.sh (one lock
# domain per QEMU binary as of #865), so a second invocation blocks in
# qemu_host_lock_acquire before it ever reaches `docker run`, rather than
# racing straight into a port-5900 conflict. The DOCKER_PID liveness check
# below (before opening TigerVNC) is kept as a second line of defense for a
# failure this lock does not cover -- e.g. a stray container from outside
# this script still holding host port 5900 -- so a `docker run` failure
# still fails loudly with an actionable message instead of silently
# opening TigerVNC onto some other session's screen.
CONTAINER_NAME="breenix-interactive-$$"
docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
_cleanup_interactive_container() {
    # TERM then KILL after a bounded wait -- docker stop's own contract:
    # SIGTERM to the container's PID 1, SIGKILL only if it has not exited
    # within the timeout.
    docker stop -t 5 "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap _cleanup_interactive_container EXIT

# Find the UEFI image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found. Build with:"
    echo "  cargo build --release --features interactive --bin qemu-uefi"
    exit 1
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)

# Copy OVMF files
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Create empty serial output files
touch "$OUTPUT_DIR/serial_user.txt"
touch "$OUTPUT_DIR/serial_kernel.txt"

echo ""
echo "========================================="
echo "Starting QEMU with VNC display"
echo "========================================="
echo "Output: $OUTPUT_DIR"
echo ""
echo "Press Ctrl+C to stop"
echo ""

# Run QEMU with VNC in background
qemu_host_lock_acquire qemu-system-x86_64
docker run --rm \
    --name "$CONTAINER_NAME" \
    -p 5900:5900 \
    -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
    -v "$BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro" \
    -v "$BREENIX_ROOT/target/ext2.img:/breenix/ext2.img:ro" \
    -v "$OUTPUT_DIR:/output" \
    "$IMAGE_NAME" \
    qemu-system-x86_64 \
        -pflash /output/OVMF_CODE.fd \
        -pflash /output/OVMF_VARS.fd \
        -drive if=none,id=hd,format=raw,media=disk,readonly=on,file=/breenix/breenix-uefi.img \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -machine pc,accel=tcg \
        -cpu qemu64 \
        -smp 1 \
        -m 512 \
        -device virtio-vga \
        -vnc :0 \
        -k en-us \
        -no-reboot \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        -serial file:/output/serial_user.txt \
        -serial file:/output/serial_kernel.txt \
    &

DOCKER_PID=$!
# F2 (#835 idiom): registers this PID with the lock's own EXIT trap so a
# SIGTERM/SIGINT delivered to just this script's own PID still stops the
# container instead of orphaning it with the lock held.
qemu_host_lock_track_pid "$DOCKER_PID"

# Wait for VNC to be ready
echo "Waiting for VNC server..."
sleep 3

# F1-review: confirm THIS invocation's own docker run client (DOCKER_PID)
# is still alive before opening a viewer. There is no host-wide lock for
# x86 (see the CONTAINER_NAME comment above), so if a still-running
# earlier run-interactive.sh already holds host port 5900, the `docker
# run --name "$CONTAINER_NAME" -p 5900:5900 ...` above fails immediately
# ("port is already allocated") and DOCKER_PID exits before this check
# runs. Failing loudly here -- by this invocation's own PID, not by
# killing anything -- replaces what would otherwise be a silent wrong
# outcome: TigerVNC opening against the OTHER session's still-live QEMU
# with no indication that this invocation's own container did not start.
if ! kill -0 "$DOCKER_PID" 2>/dev/null; then
    echo "" >&2
    echo "Error: this invocation's own QEMU container ($CONTAINER_NAME) failed to start." >&2
    echo "The most likely cause is host port 5900 already being held by an earlier," >&2
    echo "still-running run-interactive.sh session (see the docker run error above)." >&2
    echo "Stop that session (Ctrl-C in its terminal) and retry." >&2
    exit 1
fi

# Auto-open TigerVNC
echo "Opening TigerVNC..."
open "/Applications/TigerVNC Viewer 1.15.0.app" --args localhost:5900

# Wait for docker to finish
wait $DOCKER_PID 2>/dev/null

echo ""
echo "========================================="
echo "QEMU stopped"
echo "========================================="
echo ""
echo "Serial output saved to:"
echo "  User (COM1):   $OUTPUT_DIR/serial_user.txt"
echo "  Kernel (COM2): $OUTPUT_DIR/serial_kernel.txt"
