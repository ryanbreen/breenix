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
# The x86 serial console carries the scheduler's single-character trace stream
# on the same port as kernel and userspace output, so any marker line can carry
# a prefix. The markers are self-delimiting (`[...]` or a unique sentence), so
# a substring match is still exact; do not re-anchor these.
FRAME_CUSTODY_PATTERN='\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\]'
# The x86 failed-exec release oracle records one three-table hierarchy and retires it:
# recorded rises by the hierarchy, returned by the hierarchy plus its root, and undecided deliberately does not move.
PT_CUSTODY_LITERAL='[PT_CUSTODY_COUNTERS:x86:recorded=14:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=2:returned=14:lost=0:requeued=0]'
# One kernel-stack slot returns per child death across this 64-child cohort, so kstack_returns == children.
PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:kstack_returns=64:balance=0]'
PT_EXEC_COHORT_LITERAL='[PT_EXEC_COHORT:x86:children=16:superseded=3:roots=64:returned=640:recorded=576:lost=0:leaf_recorded=192:leaf_released=192:leaf_returned=192:custody_refused=0:decref_unregistered=0:undecided=0:mid_retire=0:no_arch=0:balance=0]' # The returned and recorded table-frame fields are pinned from the measured run.
# KernelStack::drop now unmaps the x86 stack VA range and releases its 128 frames; exactly one stack is allocated and dropped in this window, so 149 - 128 * 1 = 21.
# kstack_frames_released proves that arithmetic in-kernel: the oracle asserts 149 - stack_residual == kstack_frames_released, preventing silent drift.
EXEC_DETACH_ORACLE_LITERAL='[EXEC_DETACH_ORACLE:x86:bodies=2:fail_preserved=2:sibling_refused=0:success_detached=2:fresh_root=2:tgid_self=2:custody_balance=0:leaf_residual=16:stack_residual=21:kstack_frames_released=128:old_group_reached_pre=2:old_group_missed_post=2:self_group_reached_post=2]'
CLONE_ADMISSION_ORACLE_LITERAL='[CLONE_ADMISSION_ORACLE:x86:admitted=1:refused=2:creating_refused=1:published_admitted=2:balance=0]'
# Every field is a delta the oracle drives itself in the same run except
# reserved_collisions, which is the absolute boot-wide count of ordinary
# allocations that landed on the reserved init PID and must be zero.
# construct_residual is the counted frame residue of the two construction-failure arms read off a measured green run, and it is architecture-specific (4 on x86, 2 on aarch64) because the two page-table constructors record different table-frame counts.
INIT_DESIGNATION_ORACLE_LITERAL='[INIT_DESIGNATION_ORACLE:x86:construct_failed=2:construct_undecided=2:construct_residual=4:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]'
INIT_GROUP_REFUSAL_ORACLE_LITERAL='[INIT_GROUP_REFUSAL_ORACLE:x86:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]'
# driven=2 proves both handoff seams ran; stage1/2 return, wake, and park fields
# expose D1/D2. stage3_elapsed_ok=1 proves no early timeout return, while
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms is the measured duration; residual/balance prove cleanup.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:x86:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
# Absolute frame counts are boot-state dependent, so pin every delta exactly,
# including the three-table recorded_pre hierarchy cost and computed tables_returned=4;
# the in-kernel oracle asserts used_after == used_before, and a skipped/cfg'd-out block fails this gate.
EXEC_FAILED_RELEASE_ORACLE_PATTERN='\[EXEC_FAILED_RELEASE_ORACLE:x86:used_before=[0-9]+:used_after=[0-9]+:recorded_pre=3:leaf_recorded=1:leaf_released=1:leaf_returned=1:tables_returned=4:roots_retired=1:undecided=0:live_refused=0\]'
EXEC_FAILED_RELEASE_PROD_LITERAL='[EXEC_FAILED_RELEASE_PROD:x86:plain_err=true:plain_kept=true:argv_err=true:argv_kept=true:name_kept=true:balance=0:undecided=0:mid_retire=0:lost=0:custody_refused=0:decref_unregistered=0:double=0:stale=0:untracked=0:root_slot_refused=0]'
# Creation/fork/slot/frame/refusal/classifier/balance fields are oracle-driven and exact.
# live_checks is nonzero because every allocation evaluates the guard; pub_pooled and pub_sched_owned are nonzero boot-wide totals whose exact values depend on process creation, while the oracle asserts they are equal and both publication residuals are zero.
# sched_publications is a nonzero boot-wide driver for sched_pm_held_production=0.
# frame_used_delta is boot-state dependent because of heap growth during the stress; the oracle asserts it is strictly less than 128 frames, one x86 kernel stack's worth.
KSTACK_OWNER_ORACLE_PATTERN='\[KSTACK_OWNER_ORACLE:x86:creation_rows=1000:creation_owned=1000:one_owner=1000:two_owner=0:zero_owner=0:fork_rows=2:fork_owned=2:slot_returns_exact_one=2:slot_alloc_delta=1000:slot_free_delta=1000:slot_balance=0:frames_mapped_delta=128000:frames_released_delta=128000:frame_balance=0:frame_used_delta=[0-9]+:frame_used_bounded=1:live_checks=[1-9][0-9]*:live_refusals_production=0:live_refusals_injected=1:drop_refused_live=0:pte_overwrite_refusals=0:pub_pooled=[1-9][0-9]*:pub_sched_owned=[1-9][0-9]*:pub_row_residual=0:pub_unowned=0:classifier_sched_owned=1:classifier_row_residual=1:classifier_unowned=1:classifier_not_pooled=1:sched_publications=[1-9][0-9]*:sched_pm_held_production=0:sched_pm_held_injected=1:balance=0\]'
# P6a PR-2 gate extras (b)/(f)/(g). Every field is a delta the oracle drives
# itself inside one run, so the whole line is a literal: two fixture rows, one
# joined by retirement (retire_second) and one by the reap (reap_second), the
# gauge back at its entry value (resident_delta=0) and no tombstone left behind
# (tombstone_rows=0 is absolute, not a delta). This is the ONLY evidence that
# gate (g)'s retire-second arm ever executes on x86, and until this pin existed
# deleting run_x86_tombstone_join_gate() from main.rs left this gate green.
TOMBSTONE_JOIN_ORACLE_LITERAL='[TOMBSTONE_JOIN_ORACLE:x86:retire_second=1:reap_second=1:removed=2:resident_delta=0:tombstone_rows=0:PASS]'
# P6a PR-2 review finding B2, re-derived for #653 — the x86 retention claim,
# measured on production rows instead of on the oracle's fixture, in two samples
# that mean different things and are pinned separately.
#
# #653 is FIXED, and these pins are what the fix moved. The production
# deferred-reclaim claim used to be a bare boolean released only by the tail of
# the function that took it; the first timer preemption inside an owned pass
# discarded that release (x86's dispatcher restarts `idle_loop` rather than
# resuming the mid-drain continuation), so the claim latched true and every later
# drain refused for the rest of the boot. Eighteen receipts stayed queued, no
# retirement ever completed, and the four rows the live reaps claimed could never
# join. The claim window is now non-preemptible, so production reclamation runs
# to completion and BOTH samples below assert a drained system rather than
# narrating a strand.
#
# Two fixture constants carry the arithmetic. The join oracle stages two rows of
# its own before any user process exists and removes both (TOMBSTONE_JOIN_ORACLE
# above reports resident_delta=0), and the userspace phase reaps exactly four
# production rows through the live `complete_wait` path.
readonly TOMBSTONE_FIXTURE_REMOVALS=2
readonly PRODUCTION_REAPED_ROWS=4
#
# (1) End of the userspace phase, emitted from the `sys_exit` arm entered when no
# userspace thread remains. This sample used to be pinnable as an exact literal
# only BECAUSE reclamation was dead: `resident=4:removed=2` was the frozen state
# of a system that had stopped retiring. With the drain live, the split between
# "still a tombstone" and "already joined" at that instant depends on how many
# retirements the drain happened to complete while the workload was still
# running, and pinning either half would pin a race. What is timing-safe is the
# conservation law: every production row the reaps claimed is either still
# resident or already removed, so
#   resident + (removed - TOMBSTONE_FIXTURE_REMOVALS) == PRODUCTION_REAPED_ROWS
# is invariant across every scheduling of the same workload. It is asserted below
# on the LAST `[TOMBSTONE_CENSUS:` line, which is this sample: the other two
# census emitters on this arch (kernel_main's and the strand oracle's once-only
# report) both fire earlier, and the total emission count is pinned so a deleted
# sample cannot pass by leaving a stale earlier line in its place.
readonly TOMBSTONE_CENSUS_EMISSIONS=3
# (2) Quiesce, emitted from the idle loop once the deferred-reclaim queues are
# empty or a 2000 ms backstop elapses, whichever comes first. Before #653 it was
# always the backstop, and `pending` was nonzero because production reclamation
# was dead by then; the field was a bounded attribution field for exactly that
# reason. It is now an exact zero, and it is the load-bearing half of this pin:
# `pending=0:parked=0:resident=0` is the return-to-zero evidence x86 has never
# had. `removed` is the two fixture rows plus all four production rows, because
# a row can only be removed by the join after it has been both reaped and
# retired. The reap-second/retire-second SPLIT is deliberately not pinned: with
# the drain running during the userspace phase, a row may retire before or after
# its reap and the arm that completes the join differs accordingly. Their SUM is
# invariant and is asserted below.
readonly TOMBSTONE_JOINED_REMOVALS=$(( TOMBSTONE_FIXTURE_REMOVALS + PRODUCTION_REAPED_ROWS ))
TOMBSTONE_QUIESCE_PATTERN="\\[TOMBSTONE_QUIESCE:resident=0:removed=${TOMBSTONE_JOINED_REMOVALS}:reap_second=[0-9]+:retire_second=[0-9]+:abandoned_unqueued=1:pending=0:parked=0\\]"
# (3) #653 delta (3) — the drain's own refusal counters, which existed before the
# fix and were printed by nothing: a whole-boot loss of production reclamation
# was inferable only three phases later from a tombstone census read alongside a
# queue depth. `context_violations=0` is exact (a drain entered under the process
# manager or inside a scheduler scope is a defect, not a rate). `nested=1` is
# exact and is the whole point of `injected=1`: the fix keeps the nested refusal
# as its fail-closed residual for a fatal fault inside an owned pass, so the arm
# must still be provably live, and the boot-test injection drives it once on
# purpose. A `nested=0:injected=0` line would mean the refusal path no longer
# executes at all and this pin would be vacuous. `selection_capped` is the
# production selection cap firing; its exact value is a property of how many
# receipts happened to be ready per pass rather than of the claim, so it is
# shape-pinned rather than value-pinned.
RECLAIM_DRAIN_PATTERN='\[RECLAIM_DRAIN:nested=1:context_violations=0:selection_capped=[0-9]+:injected=1\]'
SCHED_STRAND_ORACLE_PATTERN='\[SCHED_STRAND_ORACLE:x86:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[0-9]+:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+:nonprogress=[0-9]+:queued_on_nondispatching_cpu=[0-9]+:worst_queued_nondispatch_ms=[0-9]+:worst_cpu_scheduler_silence_ms=[0-9]+:worst_silence_cpu=[0-9]+\]'
CENSUS_WIDEN_ORACLE_LITERAL='[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:baseline_reported=0:axes=6:SKIP]'
# The boot-test oracle deliberately drives the detector exactly once; the forbidden exact marker is separately pinned absent below.
CREATION_LOCK_ORDER_INJECTED_LITERAL='[CREATION_LOCK_ORDER:INJECTED:PM_HELD]'
CREATION_LOCK_ORDER_VIOLATION_LITERAL='[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]'
# Thirteen oracle/counter lines are pinned by the success chain below; fields are exact except for the bounded boot-state-dependent KSTACK_OWNER fields documented above.
# Ten launched test programs, one smoke_hello_time (the RING3_SMOKE process
# kernel_main_continue creates after the twelve disk-loaded ones), one
# futex_handoff_oracle, 64 retire-cohort children, five loopback_wake_test
# processes (parent, reader, peer, load, watchdog), 16 exec-cohort children, one
# clonevm_exec_test process (renamed by its second-stage exec), its phase-1
# CLONE_VM child, two clone-admission oracle rows, one init designation
# oracle terminated-row refusal (arm A5), and the two rows P6a PR-2's tombstone
# join oracle stages — its fixture has to reach the real zombie state, so both
# rows pass through terminate_minimal, which is the tally's choke point. The
# init designation oracle's other synthetic rows are removed with remove_process
# and contribute nothing, while its two construction-failure arms create no row
# at all:
# 10 + 1 + 1 + 64 + 5 + 16 + 1 + 1 + 2 + 1 + 2 = 104. The exec-detach oracle contributes
# zero because its rows use the deferred-reclaim path rather than the
# Process::terminate / terminate_minimal tally choke point. This is a floor,
# checked >= by scripts/x86-gate-verdict.sh; the production-path arm execs the
# cohort's already-inserted parent and fails without launching a new userspace
# process; re-pin consciously.
#
# Re-derived for P6a PR-2, and the re-derivation closed a pre-existing one-row
# gap rather than only adding this PR's two: the enumeration read 101 while the
# gate's own runs measured 102 (two runs, 2026-08-23) because smoke_hello_time
# was never counted. 102 + this PR's two staged rows = 104, which is what the
# branch's own run measures.
readonly EXPECTED_USERSPACE_EXITS=104

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
            && grep -qF "$PT_CUSTODY_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:retirement_fence_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:reclaim_progress_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_retire_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$PT_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_exec_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$PT_EXEC_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:exec_detach_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$EXEC_DETACH_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:clone_admission_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CLONE_ADMISSION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:init_designation_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$INIT_DESIGNATION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:init_group_refusal_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$SCHED_STRAND_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CENSUS_WIDEN_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$KSTACK_OWNER_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CREATION_LOCK_ORDER_INJECTED_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$TOMBSTONE_JOIN_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF '[TOMBSTONE_QUIESCE:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF '[RECLAIM_DRAIN:' \
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
        if grep -qE '\[CENSUS_WIDEN_ORACLE:x86:[^]]*:FAIL\]' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            break
        fi
        # The scheduler publication seam emits this prefix if it publishes while
        # the process-manager lock is held on that CPU; fail early on any variant.
        if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' \
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
    test "$(grep -h -c '\[TEST:process:x86_exec_cohort:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:exec_detach_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:clone_admission_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:init_designation_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:init_group_refusal_oracle:PASS\]' \
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
    test "$(grep -h -F -c "$PT_CUSTODY_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$PT_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$PT_EXEC_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$EXEC_DETACH_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$CLONE_ADMISSION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$INIT_DESIGNATION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$SCHED_STRAND_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -ge 1
    test "$(grep -h -F -c "$CENSUS_WIDEN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$FUTEX_HANDOFF_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c 'CLONEVM_EXEC_TEST: post-exec rendezvous complete' \
        "$OUTPUT_DIR"/serial_*.txt | \
        awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$KSTACK_OWNER_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$CREATION_LOCK_ORDER_INJECTED_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$CREATION_LOCK_ORDER_VIOLATION_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 0
    test "$(grep -h -F -c "$TOMBSTONE_JOIN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # (1) The userspace-end census, pinned by conservation rather than by a race.
    # The poll loop deliberately waits on marker PRESENCE, not on these field
    # values: a field the fix makes timing-dependent must never be able to turn a
    # real regression into a 900-second poll timeout instead of a legible
    # assertion failure here.
    test "$(grep -h -F -c '[TOMBSTONE_CENSUS:' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" \
        -eq "$TOMBSTONE_CENSUS_EMISSIONS"
    TOMBSTONE_CENSUS_USERSPACE_END_LINE=$(grep -h -F '[TOMBSTONE_CENSUS:' \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    CENSUS_RESIDENT=$(printf '%s\n' "$TOMBSTONE_CENSUS_USERSPACE_END_LINE" | \
        sed -n 's/.*\[TOMBSTONE_CENSUS:resident=\([0-9][0-9]*\):.*/\1/p')
    CENSUS_REMOVED=$(printf '%s\n' "$TOMBSTONE_CENSUS_USERSPACE_END_LINE" | \
        sed -n 's/.*\[TOMBSTONE_CENSUS:resident=[0-9][0-9]*:removed=\([0-9][0-9]*\):.*/\1/p')
    test -n "$CENSUS_RESIDENT"
    test -n "$CENSUS_REMOVED"
    test "$CENSUS_RESIDENT" -le "$PRODUCTION_REAPED_ROWS"
    test "$CENSUS_REMOVED" -ge "$TOMBSTONE_FIXTURE_REMOVALS"
    test "$(( CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ))" \
        -eq "$PRODUCTION_REAPED_ROWS"
    # (2) Quiesce: return to zero, plus the join-arm sum the split cannot pin.
    test "$(grep -h -E -c "$TOMBSTONE_QUIESCE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    QUIESCE_LINE=$(grep -h -E "$TOMBSTONE_QUIESCE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    QUIESCE_REAP_SECOND=$(printf '%s\n' "$QUIESCE_LINE" | \
        sed -n 's/.*:reap_second=\([0-9][0-9]*\):.*/\1/p')
    QUIESCE_RETIRE_SECOND=$(printf '%s\n' "$QUIESCE_LINE" | \
        sed -n 's/.*:retire_second=\([0-9][0-9]*\):.*/\1/p')
    test "$(( QUIESCE_REAP_SECOND + QUIESCE_RETIRE_SECOND ))" -eq "$TOMBSTONE_JOINED_REMOVALS"
    # (3) The drain's refusal counters, with the injected arm proving the refusal
    # path still executes.
    test "$(grep -h -E -c "$RECLAIM_DRAIN_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # x86 has no production designated init, so the runtime refusal never fires
    # and the whole-boot walk is legitimately zero here; this is the `None`-arm
    # evidence, not the whole-boot-walk evidence, and it becomes non-zero only
    # when a phase gives x86 a real init.
    INIT_GROUP_WALK_COUNT=$(awk 'index($0, "[INIT_GROUP_WALK") { count++ } END { print count + 0 }' \
        "$OUTPUT_DIR"/serial_*.txt)
    test "$INIT_GROUP_WALK_COUNT" -eq 0
    EXPECTED_EXITS="$EXPECTED_USERSPACE_EXITS" \
        "$BREENIX_ROOT/scripts/x86-gate-verdict.sh" "$OUTPUT_DIR"/serial_*.txt
    COUNTER_LINE=$(grep -hE "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    KSTACK_OWNER_LINE=$(grep -hE "$KSTACK_OWNER_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$COUNTER_LINE"
    echo "$PT_CUSTODY_LITERAL"
    echo "$PT_COHORT_LITERAL"
    echo "$PT_EXEC_COHORT_LITERAL"
    echo "$EXEC_DETACH_ORACLE_LITERAL"
    echo "$CLONE_ADMISSION_ORACLE_LITERAL"
    echo "$INIT_DESIGNATION_ORACLE_LITERAL"
    echo "$INIT_GROUP_REFUSAL_ORACLE_LITERAL"
    SCHED_STRAND_ORACLE_LINE=$(grep -h -E "$SCHED_STRAND_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$SCHED_STRAND_ORACLE_LINE"
    CENSUS_WIDEN_ORACLE_LINE=$(grep -h -F "$CENSUS_WIDEN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$CENSUS_WIDEN_ORACLE_LINE"
    FUTEX_HANDOFF_ORACLE_LINE=$(grep -h -E "$FUTEX_HANDOFF_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$FUTEX_HANDOFF_ORACLE_LINE"
    echo "$EXEC_FAILED_RELEASE_PROD_LITERAL"
    echo "$KSTACK_OWNER_LINE"
    echo "$CREATION_LOCK_ORDER_INJECTED_LITERAL"
    echo "$TOMBSTONE_JOIN_ORACLE_LITERAL"
    echo "$TOMBSTONE_CENSUS_USERSPACE_END_LINE"
    echo "$QUIESCE_LINE"
    RECLAIM_DRAIN_LINE=$(grep -h -E "$RECLAIM_DRAIN_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$RECLAIM_DRAIN_LINE"
    if grep -qE '\[BOOT_TESTS:FAIL|KERNEL PANIC|panic!' \
        "$OUTPUT_DIR"/serial_*.txt; then
        exit 1
    fi
    if grep -qE '\[CENSUS_WIDEN_ORACLE:x86:[^]]*:FAIL\]' \
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
    # Absence is meaningful because KSTACK_OWNER_ORACLE pins a nonzero
    # sched_publications driver and the injected marker proves the detector fires.
    if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' \
        "$OUTPUT_DIR"/serial_*.txt; then
        exit 1
    fi
    echo "x86 frame-custody gate run $i: PASS"
done
