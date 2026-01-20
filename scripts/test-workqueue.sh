#!/bin/bash
# Run workqueue test with timeout using Docker
# Usage: ./scripts/test-workqueue.sh [timeout_seconds]
#
# Exit codes:
#   0 - Success (WORKQUEUE_TEST_ONLY_COMPLETE found)
#   1 - Timeout or failure

set -e

TIMEOUT=${1:-60}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Kill any existing QEMU processes first (as per CLAUDE.md guidance)
pkill -9 qemu-system-x86_64 2>/dev/null || true
docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true
sleep 0.5

# Find the UEFI image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: No UEFI image found. Build with:"
    echo "  cargo build --release --features workqueue_test_only --bin qemu-uefi"
    exit 1
fi

# Setup temp directory for this run
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR; docker kill \$(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true" EXIT

cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$TMPDIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$TMPDIR/OVMF_VARS.fd"
touch "$TMPDIR/serial.txt"

echo "Running workqueue test with ${TIMEOUT}s timeout..."
echo "Image: $UEFI_IMG"

# Start QEMU in Docker background
docker run --rm \
    -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
    -v "$TMPDIR:/output" \
    breenix-qemu \
    qemu-system-x86_64 \
        -pflash /output/OVMF_CODE.fd \
        -pflash /output/OVMF_VARS.fd \
        -drive if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial file:/output/serial.txt \
    &>/dev/null &
DOCKER_PID=$!

# Wait for completion or timeout
START_TIME=$(date +%s)
while true; do
    ELAPSED=$(($(date +%s) - START_TIME))

    # Check if Docker exited
    if ! kill -0 $DOCKER_PID 2>/dev/null; then
        # Docker exited - check if it was success
        if grep -q "WORKQUEUE_TEST_ONLY_COMPLETE" "$TMPDIR/serial.txt" 2>/dev/null; then
            echo ""
            echo "SUCCESS: Workqueue test completed in ${ELAPSED}s"
            echo "Last 30 lines of output:"
            tail -30 "$TMPDIR/serial.txt"
            exit 0
        else
            echo ""
            echo "FAILURE: Docker exited without success marker"
            echo "Serial output:"
            cat "$TMPDIR/serial.txt" 2>/dev/null | tail -50 || echo "(no output)"
            exit 1
        fi
    fi

    # Check for success in serial output
    if grep -q "WORKQUEUE_TEST_ONLY_COMPLETE" "$TMPDIR/serial.txt" 2>/dev/null; then
        echo ""
        echo "SUCCESS: Workqueue test completed in ${ELAPSED}s"
        kill $DOCKER_PID 2>/dev/null || true
        echo "Last 30 lines of output:"
        tail -30 "$TMPDIR/serial.txt"
        exit 0
    fi

    # Check timeout
    if [ $ELAPSED -ge $TIMEOUT ]; then
        echo ""
        echo "TIMEOUT: Test did not complete in ${TIMEOUT}s"
        echo "Serial output (last 50 lines):"
        tail -50 "$TMPDIR/serial.txt" 2>/dev/null || echo "(no output)"
        kill -9 $DOCKER_PID 2>/dev/null || true
        docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true
        exit 1
    fi

    sleep 0.5
done
