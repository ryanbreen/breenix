#!/bin/bash
# Build and execute the x86_64 frame/page-table custody injection gates.
# This script deliberately does not treat [BOOT_TESTS:PASS] as test evidence:
# advance_stage_marker_only
# emits it unconditionally alongside [TESTS_COMPLETE:0/0]. The removed
# KERNEL_POST_TESTS_COMPLETE marker is likewise never used as a gate.
# The 900-second poll bound allows the x86 boot-test registry to run after the
# userspace programs; a shorter bound scores a slow-but-healthy boot as failed.
# http_test's live external fetches are bounded in-process by a receive
# deadline. A connect-phase failure prints an explicit SKIP marker and the boot
# continues; a mid-stream stall is an honest FAIL that appears in the tally as
# a nonzero http_test exit. A quiet boot with no marker remains a gate failure.
# This gate never retries a hung run: a blanket retry could swallow exactly the
# recv-wake regression this gate exists to catch.

set -euo pipefail

COUNT="${1:-1}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FRAME_CUSTODY_PATTERN='^\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\]$'
PT_CUSTODY_LITERAL='[PT_CUSTODY_COUNTERS:x86:recorded=11:no_proof=0:no_arch=0:terminated=1:undecided=1:exec_unreturned=0:retired=1:returned=10:lost=0:requeued=0]'
PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=64:returned=640:recorded=576:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]'
# Ten launched test programs plus 64 retire-cohort children pinned by
# PT_COHORT_LITERAL, and five loopback_wake_test processes (parent, reader,
# peer, load, watchdog): 10 + 64 + 5 = 79. This is a floor, checked >= by
# scripts/x86-gate-verdict.sh; re-pin consciously.
readonly EXPECTED_USERSPACE_EXITS=79

cd "$BREENIX_ROOT"
cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
    --features boot_tests,testing,external_test_bins --bin qemu-uefi >/dev/null
# create-test-disk packs userspace/programs/*.elf without rebuilding them, so
# repack every run to pick up rebuilt userspace; callers must rebuild those
# ELFs with ./userspace/programs/build.sh when userspace or libs/libbreenix-libc changed.
rm -f target/test_binaries.img
cargo run -p xtask -- create-test-disk
test -f target/ext2.img || ./scripts/create_ext2_disk.sh

UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"

for i in $(seq 1 "$COUNT"); do
    OUTPUT_DIR="/tmp/breenix_x86_boot_tests_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"

    qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
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
        >"$OUTPUT_DIR/qemu.log" 2>&1 &
    RUNNER_PID=$!

    passed=false
    # Four scheduling tests remain deferred on x86 until #567 is fixed:
    # loopback_recv_wake_when_idle, loopback_recv_wake_under_load,
    # loopback_pump_does_not_busy_spin, and tcp_final_ack_survives_accept_publish_race.
    # Review finding B1: the boot-window loopback wake-loss counter gate is a
    # bonus, not the proof for #545. It samples before any user process exists,
    # so three of its four counters are structurally zero and it cannot go red
    # for a #545 regression. The userspace recv/EOF wake marker below is the
    # #545 regression marker on x86: it proves end-to-end loopback FIN delivery
    # and blocked-reader wake, and goes red under a wake-path defect injection.
    # It is NOT a proof that kloopbackd is necessary — syscall-path drains can
    # deliver the same FIN, so the mechanism-level necessity proof is the
    # aarch64 deterministic registry suite (loopback_recv_wake_when_idle /
    # loopback_recv_wake_under_load), which is red on main.
    for _ in $(seq 1 900); do
        if grep -q '\[TEST:process:frame_custody_refusal_gate:PASS\]' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$FRAME_CUSTODY_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:page_table_custody_disposition_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$PT_CUSTODY_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:retirement_fence_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:reclaim_progress_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_retire_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$PT_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:userspace:loopback_recv_wake:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q 'TEST_TALLY:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            passed=true
            break
        fi
        if grep -qE '\[BOOT_TESTS:FAIL|KERNEL PANIC|panic!' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            break
        fi
        if grep -qE '\[TEST:network:[^]]*:FAIL' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            break
        fi
        if grep -qE '\[TEST:userspace:[^]]*:FAIL' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            break
        fi
        if ! kill -0 "$RUNNER_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true

    $passed
    test "$(grep -h -c '\[TEST:process:frame_custody_refusal_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:page_table_custody_disposition_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:retirement_fence_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:reclaim_progress_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:x86_retire_cohort:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # Four scheduling tests remain deferred on x86 until #567 is fixed:
    # loopback_recv_wake_when_idle, loopback_recv_wake_under_load,
    # loopback_pump_does_not_busy_spin, and tcp_final_ack_survives_accept_publish_race.
    test "$(grep -h -c '\[TEST:userspace:loopback_recv_wake:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c 'Refusing to map' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$PT_CUSTODY_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$PT_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    EXPECTED_EXITS="$EXPECTED_USERSPACE_EXITS" \
        "$BREENIX_ROOT/scripts/x86-gate-verdict.sh" "$OUTPUT_DIR"/serial_*.txt
    COUNTER_LINE=$(grep -hE "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$COUNTER_LINE"
    echo "$PT_CUSTODY_LITERAL"
    echo "$PT_COHORT_LITERAL"
    if grep -qE '\[BOOT_TESTS:FAIL|KERNEL PANIC|panic!' \
        "$OUTPUT_DIR"/serial_*.txt; then
        exit 1
    fi
    if grep -qE '\[TEST:network:[^]]*:FAIL' \
        "$OUTPUT_DIR"/serial_*.txt; then
        exit 1
    fi
    if grep -qE '\[TEST:userspace:[^]]*:FAIL' \
        "$OUTPUT_DIR"/serial_*.txt; then
        exit 1
    fi
    echo "x86 frame-custody gate run $i: PASS"
done
