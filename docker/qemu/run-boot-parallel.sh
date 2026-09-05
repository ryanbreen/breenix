#!/bin/bash
# Run N parallel full boot tests
# Usage: ./run-boot-parallel.sh [count]
#
# The QEMU invocation is identical whether it runs natively or inside the
# breenix-qemu container; Docker only ever supplied the qemu binary. The x86 gate
# host runs QEMU natively (as docker/qemu/run-x86-boot-tests.sh already does) and
# has no Docker daemon, so hard-requiring `docker run` made this gate
# unconditionally red there — and because the launch was backgrounded with its
# output discarded, the missing binary surfaced only as a marker-wait "TIMEOUT" with no
# serial output. Select the runner, and make a failed launch say so.

set -e

COUNT=${1:-5}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #797: concurrent lanes sharing one host (e.g. the beast Incus container) each
# invoking this script hardcode the identical /tmp/breenix_boot_$i path, so one
# lane's rm -rf/mkdir can clobber another lane's in-flight run. Defaulting to
# /tmp keeps every existing caller byte-identical; a concurrent-lane launcher
# sets this to a per-clone directory instead.
# claim-lint:ok: #797, diff-empty against origin/main -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Re-pin consciously whenever this profile's launched test-program set changes.
#
# Measured on the x86 gate host with this runner, one boot each:
#   main @ 43336f54 : TEST_TALLY: exited=15 nonzero=0 failed=[]
#   this branch     : TEST_TALLY: exited=17 nonzero=0 failed=[]
#
# The old pin of 10 was five under main's own measurement: the loopback_wake_test
# cohort (parent, reader, peer, load, watchdog = 5 process deaths) reached this
# profile without the floor being re-pinned, and a `>=` floor hides an under-pin.
# 10 + 5 = 15 is main; the two this branch adds are
#   +1 /usr/local/test/bin/clonevm_exec_test - the launched program. It execs
#      into its own second stage, so it is one row and one death, renamed by the
#      exec.
#   +1 thread-14 - its phase-1 CLONE_VM child (sys_clone names child rows
#      thread-<pid>; the scheduler thread appears as clone-child-26).
# 15 + 1 + 1 = 17. The clone-admission and exec-detach oracles contribute zero
# here: both are #[cfg(feature = "boot_tests")] and this profile is built
# testing,external_test_bins.
readonly EXPECTED_USERSPACE_EXITS=17

# Rebuild userspace ELFs, then repack with cargo run -p xtask -- create-test-disk
# before invoking this pure runner; a stale image boots the previous branch's binaries.
# Find the full boot image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: No UEFI image found. Build with:"
    echo "  cargo build --release --features testing,external_test_bins --bin qemu-uefi"
    exit 1
fi

for image in "$BREENIX_ROOT/target/test_binaries.img" "$BREENIX_ROOT/target/ext2.img"; do
    if [ ! -f "$image" ]; then
        echo "Error: missing $image. Repack with:"
        echo "  cargo run -p xtask -- create-test-disk && ./scripts/create_ext2_disk.sh"
        exit 1
    fi
done

# Pick the QEMU runner. Native first: it is what the x86 gate host provides and
# what run-x86-boot-tests.sh already uses, and it removes a Docker daemon from
# the trusted path of a gate that only ever needed a qemu binary.
if command -v qemu-system-x86_64 >/dev/null 2>&1; then
    RUNNER=native
elif command -v docker >/dev/null 2>&1; then
    RUNNER=docker
else
    echo "Error: no QEMU runner available (need qemu-system-x86_64 on PATH, or docker with the breenix-qemu image)"
    exit 1
fi

echo "Running $COUNT parallel full boot tests (runner: $RUNNER)..."
echo "Image: $UEFI_IMG"

declare -a RUNNER_PIDS=()

# Create output directories and launch the boots
for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_boot_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    # Both branches pass byte-identical QEMU arguments; only the file paths
    # differ, because the container sees the images through bind mounts.
    if [ "$RUNNER" = native ]; then
        qemu-system-x86_64 \
            -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
            -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
            -drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
            -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
            -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
            -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
            -display none -no-reboot -no-shutdown \
            -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
            -serial "file:$OUTPUT_DIR/serial_user.txt" \
            -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
            >"$OUTPUT_DIR/runner.log" 2>&1 &
    else
        docker run --rm \
            -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
            -v "$BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro" \
            -v "$BREENIX_ROOT/target/ext2.img:/breenix/ext2.img:ro" \
            -v "$OUTPUT_DIR:/output" \
            breenix-qemu \
            qemu-system-x86_64 \
                -pflash /output/OVMF_CODE.fd \
                -pflash /output/OVMF_VARS.fd \
                -drive if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img \
                -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
                -drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img \
                -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
                -drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img \
                -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
                -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
                -display none -no-reboot -no-shutdown \
                -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
                -serial file:/output/serial_user.txt \
                -serial file:/output/serial_kernel.txt \
            >"$OUTPUT_DIR/runner.log" 2>&1 &
    fi
    RUNNER_PIDS[$i]=$!
    echo "  Started test $i"
done

# Wait for all to complete: retain the early kthread subsystem checks, then wait
# for the userspace TEST_TALLY and require the computed x86 gate verdict. This
# proves the full pinned test-program cohort ran and exited successfully.
echo "Waiting for kthread tests to complete (900s timeout)..."
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_boot_$i"
    FOUND=false

    LAUNCH_FAILED=false
    # Use a 900s bound to match run-x86-boot-tests.sh: a shorter bound scores a
    # slow-but-healthy boot as failed. This changes only how long the gate waits;
    # a missing marker, a dead runner, and a failing x86-gate-verdict.sh verdict
    # still fail the gate exactly as before. Measured on the beast x86 VM: 120s
    # fails identically on main and this branch; 600s passes with exited=100.
    # A runner that died before producing any output is reported as a launch
    # failure with its log, not as an indistinguishable timeout.
    for j in $(seq 1 900); do
        if grep -q "KTHREAD JOIN TEST: Completed" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
        if ! kill -0 "${RUNNER_PIDS[$i]}" 2>/dev/null \
            && [ ! -s "$OUTPUT_DIR/serial_kernel.txt" ]; then
            LAUNCH_FAILED=true
            break
        fi
        sleep 1
    done

    if $LAUNCH_FAILED; then
        echo "  Test $i: FAIL (QEMU runner exited without producing serial output)"
        sed -n '1,20p' "$OUTPUT_DIR/runner.log" 2>/dev/null || echo "    (no runner log)"
        FAILED=$((FAILED + 1))
    elif $FOUND; then
        # Check if kthread tests actually passed
        if grep -q "KTHREAD_EXIT: kthread exited cleanly" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            # Kthread markers alone don't prove the userspace cohort completed.
            # Wait for the TERMINAL verdict marker, not the tally: the kernel
            # prints TEST_TALLY first and "TEST RUNNER: All tests passed" /
            # "TEST RUNNER: FAILED" last, and x86-gate-verdict.sh requires the
            # latter. Evaluating the verdict at first-tally-sighting raced the
            # terminal marker and scored a healthy boot as
            # "nonzero=0 but the all-tests-passed marker is absent".
            for j in $(seq 1 90); do
                if grep -qE "TEST RUNNER: (All tests passed|FAILED)" \
                    "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
                    break
                fi
                sleep 1
            done

            # #568 anti-vacuity for the poll oracle used to be pinned here: a
            # missing [POLL_TCP_ORACLE:] marker failed this gate the way the
            # aarch64 gates pin their oracles. The assertion is removed with the
            # oracle itself, which is not launched on x86 (see the #697 comment
            # in kernel/src/main.rs) because it shifts the tombstone census
            # docker/qemu/run-x86-boot-tests.sh pins and its verdict cannot yet
            # be told apart from TCG starvation. Restoring both the launch and
            # this assertion is #697. On aarch64 the oracle and its eight gate
            # assertions are unchanged and green.

            if VERDICT_OUTPUT="$(
                EXPECTED_EXITS="$EXPECTED_USERSPACE_EXITS" \
                    "$BREENIX_ROOT/scripts/x86-gate-verdict.sh" \
                    "$OUTPUT_DIR/serial_kernel.txt" "$OUTPUT_DIR/serial_user.txt" 2>&1
            )"; then
                echo "  Test $i: PASS"
                echo "    $VERDICT_OUTPUT"
                PASSED=$((PASSED + 1))
            else
                echo "  Test $i: FAIL (x86 userspace verdict rejected the run)"
                echo "    $VERDICT_OUTPUT"
                tail -10 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "    (no output)"
                FAILED=$((FAILED + 1))
            fi
        else
            echo "  Test $i: FAIL (kthread didn't exit cleanly)"
            FAILED=$((FAILED + 1))
        fi
    else
        echo "  Test $i: TIMEOUT"
        tail -10 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "    (no output)"
        FAILED=$((FAILED + 1))
    fi
done

# Cleanup: reap whichever runner was used.
if [ "$RUNNER" = native ]; then
    for i in $(seq 1 $COUNT); do
        kill "${RUNNER_PIDS[$i]}" 2>/dev/null || true
        wait "${RUNNER_PIDS[$i]}" 2>/dev/null || true
    done
else
    docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true
fi

echo ""
echo "========================================="
echo "Results: $PASSED passed, $FAILED failed out of $COUNT"
echo "========================================="

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
