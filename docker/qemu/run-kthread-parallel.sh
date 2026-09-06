#!/bin/bash
# Run N parallel Docker kthread tests
# Usage: ./run-kthread-parallel.sh [count]

set -e

COUNT=${1:-10}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/#834/#865/R181: this script's own COUNT qemu-system-x86_64 containers
# are deliberately launched concurrently WITH EACH OTHER (its own internal
# parallelism, matching run-boot-parallel.sh's x86 twin) -- the host-wide
# lock in lib/qemu-host-lock.sh adds exclusion against a DIFFERENT script's
# x86 boot lane running at the same time on the same host, acquired once
# before the launch loop and released once after the wait loop, treating
# this whole COUNT-container batch as one occupant of the x86 lock domain.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #797: concurrent lanes sharing one host (e.g. the beast Incus container) each
# invoking this script hardcode the identical /tmp/breenix_kthread_$i path, so
# one lane's rm -rf/mkdir can clobber another lane's in-flight run. Defaulting
# to /tmp keeps every existing caller byte-identical; a concurrent-lane
# launcher sets this to a per-clone directory instead.
# claim-lint:ok: #797, diff-empty against origin/main -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value resolves against whatever directory is
# current at the point each command runs (review finding F6 on #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "GATE: FAIL (BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP)" >&2; exit 1 ;;
esac

# Find the kthread_test_only image (build with: cargo build --release --features kthread_test_only --bin qemu-uefi)
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: No UEFI image found. Build with:"
    echo "  cargo build --release --features kthread_test_only --bin qemu-uefi"
    exit 1
fi

echo "Running $COUNT parallel Docker kthread tests..."
echo "Image: $UEFI_IMG"

# #849: each iteration gets its own container name (this script's own PID
# plus the loop index), so cleanup below stops only the containers THIS
# invocation started -- an ancestor-image filter (the pre-#849 shape)
# matches any breenix-qemu container currently running, including a
# concurrent invocation's still-running one.
declare -a CONTAINER_NAMES=()
declare -a RUNNER_PIDS=()

qemu_host_lock_acquire qemu-system-x86_64

# Create output directories and launch containers
for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_kthread_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"
    CONTAINER_NAMES[$i]="breenix-kthread-parallel-$$-$i"

    docker run --rm \
        --name "${CONTAINER_NAMES[$i]}" \
        -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
        -v "$OUTPUT_DIR:/output" \
        breenix-qemu \
        qemu-system-x86_64 \
            -pflash /output/OVMF_CODE.fd \
            -pflash /output/OVMF_VARS.fd \
            -drive if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
            -display none -no-reboot -no-shutdown \
            -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
            -serial file:/output/serial_user.txt \
            -serial file:/output/serial_kernel.txt \
        &>/dev/null &
    RUNNER_PIDS[$i]=$!
    qemu_host_lock_track_pid "${RUNNER_PIDS[$i]}"
    echo "  Started test $i"
done

# Wait for all to complete (with timeout)
echo "Waiting for tests to complete (60s timeout)..."
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_kthread_$i"

    # Wait up to 60 seconds for this test
    for j in $(seq 1 60); do
        if grep -q "KTHREAD_TEST_ONLY_COMPLETE" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            echo "  Test $i: PASS"
            PASSED=$((PASSED + 1))
            break
        fi
        sleep 1
    done

    if [ $j -eq 60 ]; then
        echo "  Test $i: TIMEOUT"
        FAILED=$((FAILED + 1))
    fi
done

# Cleanup: stop only the containers this invocation started.
for i in $(seq 1 $COUNT); do
    docker stop -t 5 "${CONTAINER_NAMES[$i]}" >/dev/null 2>&1 || true
done
# #865: this batch's own COUNT containers are each reaped above, so the x86
# lock domain is freed here rather than deferred to script exit.
qemu_host_lock_release

echo ""
echo "========================================="
echo "Results: $PASSED passed, $FAILED failed out of $COUNT"
echo "========================================="

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
