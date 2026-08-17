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

fail() {
    echo "x86 frame-custody gate: FAIL - $1"
    exit 1
}

COUNT="${1:-1}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FRAME_CUSTODY_PATTERN='^\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\]$'
# The x86 failed-exec release oracle records one three-table hierarchy and retires it:
# recorded rises by the hierarchy, returned by the hierarchy plus its root, and undecided deliberately does not move.
PT_CUSTODY_LITERAL='[PT_CUSTODY_COUNTERS:x86:recorded=14:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=2:returned=14:lost=0:requeued=0]'
PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]'
PT_EXEC_COHORT_LITERAL='[PT_EXEC_COHORT:x86:children=16:superseded=3:roots=64:returned=640:recorded=576:lost=0:leaf_recorded=192:leaf_released=192:leaf_returned=192:custody_refused=0:decref_unregistered=0:undecided=0:mid_retire=0:no_arch=0:balance=0]' # The returned and recorded table-frame fields are pinned from the measured run.
EXEC_DETACH_ORACLE_LITERAL='[EXEC_DETACH_ORACLE:x86:bodies=2:fail_preserved=2:sibling_refused=0:success_detached=2:fresh_root=2:tgid_self=2:custody_balance=0:leaf_residual=16:stack_residual=149:old_group_reached_pre=2:old_group_missed_post=2:self_group_reached_post=2]'
CLONE_ADMISSION_ORACLE_LITERAL='[CLONE_ADMISSION_ORACLE:x86:admitted=1:refused=2:creating_refused=1:published_admitted=2:balance=0]'
# REMOTE-BOOT PLACEHOLDER: replace both residuals from the x86 DIAG line,
# together with the kernel constants and structural pins. The impossible
# sentinels deliberately keep this gate red until that measurement exists.
CREATING_DISPATCH_ORACLE_X86_LITERAL='[CREATING_DISPATCH_ORACLE:x86:injected=1:refused_via_dispatch=1:requeue_retried=1:dispatched_after_publish=1:balance=0:leaf_residual=18446744073709551615:user_stack_residual=-9223372036854775808]'
# Absolute frame counts are boot-state dependent, so pin every delta exactly,
# including the three-table recorded_pre hierarchy cost and computed tables_returned=4;
# the in-kernel oracle asserts used_after == used_before, and a skipped/cfg'd-out block fails this gate.
EXEC_FAILED_RELEASE_ORACLE_PATTERN='^\[EXEC_FAILED_RELEASE_ORACLE:x86:used_before=[0-9]+:used_after=[0-9]+:recorded_pre=3:leaf_recorded=1:leaf_released=1:leaf_returned=1:tables_returned=4:roots_retired=1:undecided=0:live_refused=0\]$'
EXEC_FAILED_RELEASE_PROD_LITERAL='[EXEC_FAILED_RELEASE_PROD:x86:plain_err=true:plain_kept=true:argv_err=true:argv_kept=true:name_kept=true:balance=0:undecided=0:mid_retire=0:lost=0:custody_refused=0:decref_unregistered=0:double=0:stale=0:untracked=0:root_slot_refused=0]'
# Ten launched test programs, 64 retire-cohort children, five loopback_wake_test
# processes (parent, reader, peer, load, watchdog), 16 exec-cohort children, one
# clonevm_exec_test process (renamed by its second-stage exec), its phase-1
# CLONE_VM child, and two clone-admission oracle rows:
# 10 + 64 + 5 + 16 + 1 + 1 + 1 + 1 = 99. The exec-detach oracle contributes
# zero because its rows use the deferred-reclaim path rather than the
# Process::terminate / terminate_minimal tally choke point. This is a floor,
# checked >= by scripts/x86-gate-verdict.sh; the production-path arm execs the
# cohort's already-inserted parent and fails without launching a new userspace
# process; re-pin consciously.
readonly EXPECTED_USERSPACE_EXITS=99

cd "$BREENIX_ROOT"
cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
    --features boot_tests,testing,external_test_bins --bin qemu-uefi >/dev/null
# create-test-disk packs userspace/programs/*.elf without rebuilding them, so
# repack every run to pick up rebuilt userspace; callers must rebuild those
# ELFs with ./userspace/programs/build.sh when userspace or libs/libbreenix-libc changed.
rm -f target/test_binaries.img
cargo run -p xtask -- create-test-disk
# The ext2 image carries the same userspace binaries, so rebuild it every run:
# a cached image silently boots old programs, and a fresh program execv-ing its
# own installed path can land in a stale copy of itself.
rm -f target/ext2.img
./scripts/create_ext2_disk.sh

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
    # The poll loop kills QEMU the moment it breaks, so it must never break on a
    # marker the kernel prints BEFORE the one the verdict needs. `TEST_TALLY:` is
    # emitted first and `TEST RUNNER: All tests passed` / `TEST RUNNER: FAILED`
    # last (kernel/src/syscall/handlers.rs), so breaking on the tally alone raced
    # the terminal marker and scored a healthy boot as
    # "nonzero=0 but the all-tests-passed marker is absent". Wait for the terminal
    # verdict marker itself; either polarity ends the wait, and the failing
    # polarity is still rejected downstream by scripts/x86-gate-verdict.sh.
    for _ in $(seq 1 900); do
        if grep -q '\[TEST:process:frame_custody_refusal_gate:PASS\]' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$FRAME_CUSTODY_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:page_table_custody_disposition_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$PT_CUSTODY_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:retirement_fence_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:reclaim_progress_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_retire_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$PT_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_exec_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$PT_EXEC_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:exec_detach_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$EXEC_DETACH_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:clone_admission_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$CLONE_ADMISSION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:creating_dispatch_refusal_x86:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$CREATING_DISPATCH_ORACLE_X86_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF -x "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:userspace:loopback_recv_wake:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q 'TEST_TALLY:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE 'TEST RUNNER: (All tests passed|FAILED)' \
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

    grep -q '\[TEST:process:creating_dispatch_refusal_x86:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt \
        || fail "missing x86 creating-dispatch refusal test PASS marker"
    grep -qF -x "$CREATING_DISPATCH_ORACLE_X86_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt \
        || fail "missing x86 creating-dispatch refusal oracle literal"
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
    test "$(grep -h -c '\[TEST:process:x86_exec_cohort:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:exec_detach_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:clone_admission_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:creating_dispatch_refusal_x86:PASS\]' \
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
    test "$(grep -h -E -c "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$PT_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$PT_EXEC_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$EXEC_DETACH_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$CLONE_ADMISSION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$CREATING_DISPATCH_ORACLE_X86_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -x -c "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    EXPECTED_EXITS="$EXPECTED_USERSPACE_EXITS" \
        "$BREENIX_ROOT/scripts/x86-gate-verdict.sh" "$OUTPUT_DIR"/serial_*.txt
    COUNTER_LINE=$(grep -hE "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$COUNTER_LINE"
    echo "$PT_CUSTODY_LITERAL"
    echo "$PT_COHORT_LITERAL"
    echo "$PT_EXEC_COHORT_LITERAL"
    echo "$EXEC_DETACH_ORACLE_LITERAL"
    echo "$CLONE_ADMISSION_ORACLE_LITERAL"
    echo "$CREATING_DISPATCH_ORACLE_X86_LITERAL"
    echo "$EXEC_FAILED_RELEASE_PROD_LITERAL"
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
