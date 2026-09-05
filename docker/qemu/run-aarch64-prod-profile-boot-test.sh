#!/bin/bash
# ARM64 production-profile boot negative control for #584/B1.
#
# This proves the production aarch64 profile shipped by
# scripts/parallels/build-efi.sh is not wedged by boot_tests-only futex oracle
# plumbing: the unarmed driver must time out its probe, init must resume, and
# bsshd must start.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# #825: two concurrent runs of this gate (e.g. two worktrees on the same
# host) each hardcoded the identical /tmp/breenix_aarch64_prod_profile path,
# so one run's rm -rf/mkdir could delete and rewrite the serial another run
# was mid-boot writing to, and both booted from the same ext2-writable.img
# either could be rewriting. Defaulting to /tmp keeps every existing caller
# byte-identical; a concurrent-lane launcher sets this to a per-worktree
# directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when it is read (the same F6 guard PR
# #801 gave the x86 gate scripts for #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2; exit 1 ;;
esac

# -110 is -ETIMEDOUT. Exactly one occurrence proves the unarmed kernel honoured
# the driver's probe timeout and that the seam-absent path actually executed.
PROD_SEAM_ABSENT_LITERAL='[FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe=-110]'
# This boot_tests-only kernel marker must be wholly absent from the unarmed run.
KERNEL_ORACLE_LITERAL='[FUTEX_HANDOFF_ORACLE:'
# These boot_tests-only scheduler strand markers must also be absent from the
# shipped production profile.
SCHED_STRAND_ORACLE_LITERAL='[SCHED_STRAND_ORACLE:'
STRAND_INJECT_ORACLE_LITERAL='[STRAND_INJECT_ORACLE:'
# #796's contention oracle is boot_tests-only for a reason that matters on this
# profile: it holds the process-manager lock on a peer CPU on purpose. This is
# the 4th boot_tests-only marker asserted absent here, and it is asserted for the
# same reason as the other 3: a count of 0 on the shipped profile is a reading,
# where a silent absence would be an assumption.
FCNTL_PM_ORACLE_LITERAL='[FCNTL_PM_CONTENTION_ORACLE:'
# This proves init resumed after waiting for the self-limiting driver.
INIT_EXIT_LITERAL='[init] futex_handoff_oracle exited pid='
# This proves init's earlier oracle also completes on the unarmed profile.
BLOCK_EINTR_ORACLE_LITERAL='[BLOCK_EINTR_ORACLE:'
# This proves init's earlier oracle did not self-report a failure.
BLOCK_EINTR_ORACLE_FAIL_LITERAL='[BLOCK_EINTR_ORACLE:FAIL'
# #568: init's blocking-poll-on-connected-TCP oracle runs on this profile too --
# it is launched from init, not from a boot_tests-only seam -- so the production
# profile is where a lost poll wake would show up on a shipped kernel. Pinned as
# a pair: presence, then absence-of-FAIL.
POLL_TCP_ORACLE_LITERAL='[POLL_TCP_ORACLE:'
POLL_TCP_ORACLE_FAIL_LITERAL='[POLL_TCP_ORACLE:FAIL'
# #693: the kernel's own report from `sys_poll`. READY_LOST is the contradiction
# -- readiness published inside a blocking poll's window, still buffered, and
# not reported -- and TIMEOUT is the ordinary line the same function emits on
# each boot, pinned so that a dead reporting path cannot pass for a clean one.
POLL_TCP_READY_LOST_LITERAL='[POLL_TCP_READY_LOST]'
POLL_TCP_TIMEOUT_LITERAL='[POLL_TCP_TIMEOUT]'
# Green-program arc 4: the TTY evidence leg. /bin/tty_oracle is launched from
# init, not from a boot_tests-only seam, so the production profile is where the
# shipped kernel's PTY / line-discipline / termios surface is actually driven.
# Pinned as a pair, like the oracles above: presence, then absence-of-FAIL.
TTY_ORACLE_LITERAL='[TTY_ORACLE:COMPLETE:'
TTY_ORACLE_FAIL_LITERAL='[TTY_ORACLE:FAIL'
# This proves init progressed through to the production SSH service.
BSSHD_LITERAL='bsshd: listening'
# Any one of these literals means the production boot crashed or locked up.
# claim-lint:ok: 0 of 3 boots at this head match any of them --
# docs/planning/green-program/aarch64-testing/serials/asid-ratchet/04-prod-boot1.txt
# and its 2 siblings
# #786 follow-on: the TTBR0 ASID census, emitted before userspace and at every
# process exit. `untagged` counts publishes into `saved_process_cr3`/`next_cr3`
# of a process root whose ASID field is not the userspace ASID -- the value the
# `.Lrestore_saved_ttbr` arm of `syscall_entry.S` would install verbatim, which
# is what put returns to EL0 on ASID 0 for five hours of `main`. Three
# assertions, not one: the line must be present, no line may report a non-zero
# `untagged`, and at least one line must report a non-zero `tagged`, because a
# census that counted no process-root publish at all would report `untagged=0`
# for the same reason a dead counter does.
# claim-lint:ok: this gate goes red on the raw-operand mutation with
# 12 of 14 census lines reporting untagged>0 --
# docs/planning/green-program/aarch64-testing/serials/asid-ratchet/02-runtime-anti-vacuity-prod-gate.txt
ASID_CENSUS_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[0-9]+:tagged=[0-9]+:kernel=[0-9]+:cleared=[0-9]+\]'
ASID_CENSUS_UNTAGGED_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[1-9][0-9]*:'
ASID_CENSUS_PUBLISHED_PATTERN='\[TTBR0_ASID_CENSUS:untagged=[0-9]+:tagged=[1-9][0-9]*:'
# Slice 3d: the pinned-placement census. Three assertions rather than one, for
# the reason the ASID block above gives: the line must be present, no line may
# report a field above zero, and the one-shot first-hold marker must be absent
# -- the census is emitted on a period, so a hold after the last emission would
# otherwise be invisible while the marker fires whenever the first one happens.
# A census line is scored by comparing it against the all-zero literal rather
# than by matching each field, so a field added to the line later is gated on
# the day it appears rather than on the day someone remembers to widen a regex.
# claim-lint:ok: 3 of 3 strict boots and 3 of 3 production boots at this head
# read the all-zero literal, and the forced-hold leg reddens this gate --
# docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-x3.txt,
# 02-prod-boot1.txt and its 2 siblings, 05-runtime-anti-vacuity-strict-gate.txt
PINNED_CENSUS_PATTERN='\[PINNED_HOME_CPU_UNAVAILABLE:count=[0-9]+:publish_discarded=[0-9]+:hold_pen_migrated=[0-9]+:delivered=[0-9]+\]'
PINNED_CENSUS_ZERO_LITERAL='[PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0]'
PINNED_FIRST_HOLD_LITERAL='[PINNED_HOME_CPU_UNAVAILABLE:first:'
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected'

OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_prod_profile"
SERIAL_FILE="$OUTPUT_DIR/serial.txt"
QEMU_PID=""

# Scoring-only entry point, the same shape run-aarch64-boot-test-strict.sh has
# carried as BREENIX_STRICT_SCORE_ONLY. R157/ASID-01: without it the only thing
# a test could say about this gate's verdict rules was that the script CONTAINS
# some pattern strings, which stays true after every assertion using them is
# deleted. With it, the verdict block below is what runs -- unchanged, on the
# serial named here -- so deleting an assertion changes the exit status a test
# can read.
# claim-lint:ok: 3 of 3 assertions in this gate were deleted as a mutation and the test
# caught it; the leg is section 12 of docs/planning/green-
# program/aarch64-testing/TTBR0-ASID-RATCHET-2026-09-05.md
#
# The boot is skipped, not the scoring: everything between here and the verdict
# is guarded on this variable being empty, and the verdict block itself is not
# guarded at all.
SCORE_ONLY_SERIAL="${BREENIX_PROD_SCORE_ONLY:-}"
if [ -n "$SCORE_ONLY_SERIAL" ]; then
    SERIAL_FILE="$SCORE_ONLY_SERIAL"
    if [ ! -f "$SERIAL_FILE" ]; then
        echo "FAIL: BREENIX_PROD_SCORE_ONLY names no readable serial: $SERIAL_FILE"
        exit 1
    fi
fi

marker_count() {
    local serial_file="$1"
    local literal="$2"
    if [ ! -f "$serial_file" ]; then
        echo 0
        return
    fi
    grep -F -c "$literal" "$serial_file" 2>/dev/null || true
}

pattern_count() {
    local serial_file="$1"
    local pattern="$2"
    if [ ! -f "$serial_file" ]; then
        echo 0
        return
    fi
    grep -aE -c "$pattern" "$serial_file" 2>/dev/null || true
}

# Census lines that differ from the zero literal. Scored by comparison rather
# than by a per-field pattern, so a field added to the line later is gated the
# day it appears rather than the day someone remembers to widen a regex.
# claim-lint:ok: 5 of 5 legs of
# both_aarch64_gates_fail_on_a_pinned_placement_refusal run this gate's own
# verdict code, and 2 of those 5 vary a census field
pinned_nonzero_count() {
    local serial_file="$1"
    if [ ! -f "$serial_file" ]; then
        echo 0
        return
    fi
    grep -aoE "$PINNED_CENSUS_PATTERN" "$serial_file" 2>/dev/null | grep -cvxF "$PINNED_CENSUS_ZERO_LITERAL" || true
}

crash_count() {
    local serial_file="$1"
    if [ ! -f "$serial_file" ]; then
        echo 0
        return
    fi
    grep -iE -c "$CRASH_MARKERS_PATTERN" "$serial_file" 2>/dev/null || true
}

print_observed_values() {
    local serial_file="$1"
    echo "Observed seam-absent marker count: $(marker_count "$serial_file" "$PROD_SEAM_ABSENT_LITERAL")"
    echo "Observed kernel oracle marker count: $(marker_count "$serial_file" "$KERNEL_ORACLE_LITERAL")"
    echo "Observed scheduler strand oracle marker count: $(marker_count "$serial_file" "$SCHED_STRAND_ORACLE_LITERAL")"
    echo "Observed strand injection oracle marker count: $(marker_count "$serial_file" "$STRAND_INJECT_ORACLE_LITERAL")"
    echo "Observed fcntl contention oracle marker count: $(marker_count "$serial_file" "$FCNTL_PM_ORACLE_LITERAL")"
    echo "Observed init-resumed marker count: $(marker_count "$serial_file" "$INIT_EXIT_LITERAL")"
    echo "Observed block EINTR oracle marker count: $(marker_count "$serial_file" "$BLOCK_EINTR_ORACLE_LITERAL")"
    echo "Observed block EINTR oracle failure count: $(marker_count "$serial_file" "$BLOCK_EINTR_ORACLE_FAIL_LITERAL")"
    echo "Observed poll TCP oracle marker count: $(marker_count "$serial_file" "$POLL_TCP_ORACLE_LITERAL")"
    echo "Observed poll TCP oracle failure count: $(marker_count "$serial_file" "$POLL_TCP_ORACLE_FAIL_LITERAL")"
    echo "Observed kernel poll timeout report count: $(marker_count "$serial_file" "$POLL_TCP_TIMEOUT_LITERAL")"
    echo "Observed kernel lost-readiness report count: $(marker_count "$serial_file" "$POLL_TCP_READY_LOST_LITERAL")"
    echo "Observed TTY oracle marker count: $(marker_count "$serial_file" "$TTY_ORACLE_LITERAL")"
    echo "Observed TTY oracle failure count: $(marker_count "$serial_file" "$TTY_ORACLE_FAIL_LITERAL")"
    echo "Observed bsshd marker count: $(marker_count "$serial_file" "$BSSHD_LITERAL")"
    echo "Observed TTBR0 ASID census marker count: $(pattern_count "$serial_file" "$ASID_CENSUS_PATTERN")"
    echo "Observed TTBR0 ASID census untagged-publish line count: $(pattern_count "$serial_file" "$ASID_CENSUS_UNTAGGED_PATTERN")"
    echo "Observed pinned-placement census marker count: $(pattern_count "$serial_file" "$PINNED_CENSUS_PATTERN")"
    echo "Observed pinned-placement non-zero census line count: $(pinned_nonzero_count "$serial_file")"
    echo "Observed crash marker count: $(crash_count "$serial_file")"
    if [ -f "$serial_file" ]; then
        grep -iE "$CRASH_MARKERS_PATTERN" "$serial_file" 2>/dev/null || true
    fi
}

cleanup() {
    local status="$1"
    local timestamp
    local failure_dir

    trap - EXIT
    set +e
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi

    if [ "$status" -ne 0 ]; then
        timestamp=$(date -u +%Y%m%dT%H%M%SZ)
        failure_dir="$BREENIX_GATE_TMP/breenix_prod_profile_failures/$timestamp"
        while [ -e "$failure_dir" ]; do
            sleep 1
            timestamp=$(date -u +%Y%m%dT%H%M%SZ)
            failure_dir="$BREENIX_GATE_TMP/breenix_prod_profile_failures/$timestamp"
        done
        mkdir -p "$failure_dir"
        if [ -f "$SERIAL_FILE" ]; then
            cp "$SERIAL_FILE" "$failure_dir/serial.txt"
        else
            : > "$failure_dir/serial.txt"
        fi
        echo "Preserved failing serial: $failure_dir/serial.txt"
        print_observed_values "$failure_dir/serial.txt"
    fi
    exit "$status"
}
if [ -z "$SCORE_ONLY_SERIAL" ]; then
trap 'cleanup $?' EXIT

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"
: > "$SERIAL_FILE"

REBUILD_USERSPACE=false
for arg in "$@"; do
    case "$arg" in
        --rebuild-userspace) REBUILD_USERSPACE=true ;;
        *)
            echo "FAIL: unknown argument: $arg"
            exit 1
            ;;
    esac
done

echo "Building the shipped ARM64 production kernel profile..."
# The absence of --features is the point: adding one would make this gate
# measure a different profile than the image scripts/parallels/build-efi.sh ships.
if ! (cd "$BREENIX_ROOT" && cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64); then
    echo "FAIL: production-profile kernel build failed"
    exit 1
fi

KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "FAIL: production kernel missing at $KERNEL"
    exit 1
fi

# Durable #528 guard: the shipped kernel must remain on the soft-float target.
if ! "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"; then
    echo "FAIL: production kernel failed the no-NEON guard"
    exit 1
fi

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if $REBUILD_USERSPACE; then
    "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64
    "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64
elif [ ! -f "$EXT2_DISK" ]; then
    echo "FAIL: ext2 disk not found at $EXT2_DISK"
    echo "Re-run with --rebuild-userspace to build userspace and create it."
    exit 1
fi

if [ ! -f "$EXT2_DISK" ]; then
    echo "FAIL: ext2 disk was not created at $EXT2_DISK"
    exit 1
fi

EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

echo "Booting the ARM64 production profile..."
timeout 120 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
    -kernel "$KERNEL" \
    -display none -no-reboot \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -serial file:"$OUTPUT_DIR/serial.txt" &
QEMU_PID=$!

POLL=0
while [ "$POLL" -lt 120 ]; do
    if [ -f "$SERIAL_FILE" ]; then
        if grep -F -q "$BSSHD_LITERAL" "$SERIAL_FILE" 2>/dev/null; then
            break
        fi
        if grep -qiE "$CRASH_MARKERS_PATTERN" "$SERIAL_FILE" 2>/dev/null; then
            break
        fi
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    POLL=$((POLL + 1))
    sleep 1
done

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""
fi

PROD_SEAM_ABSENT_COUNT=$(marker_count "$SERIAL_FILE" "$PROD_SEAM_ABSENT_LITERAL")
KERNEL_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$KERNEL_ORACLE_LITERAL")
SCHED_STRAND_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$SCHED_STRAND_ORACLE_LITERAL")
STRAND_INJECT_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$STRAND_INJECT_ORACLE_LITERAL")
FCNTL_PM_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$FCNTL_PM_ORACLE_LITERAL")
INIT_EXIT_COUNT=$(marker_count "$SERIAL_FILE" "$INIT_EXIT_LITERAL")
BLOCK_EINTR_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$BLOCK_EINTR_ORACLE_LITERAL")
BLOCK_EINTR_ORACLE_FAIL_COUNT=$(marker_count "$SERIAL_FILE" "$BLOCK_EINTR_ORACLE_FAIL_LITERAL")
POLL_TCP_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$POLL_TCP_ORACLE_LITERAL")
POLL_TCP_ORACLE_FAIL_COUNT=$(marker_count "$SERIAL_FILE" "$POLL_TCP_ORACLE_FAIL_LITERAL")
POLL_TCP_READY_LOST_COUNT=$(marker_count "$SERIAL_FILE" "$POLL_TCP_READY_LOST_LITERAL")
POLL_TCP_TIMEOUT_COUNT=$(marker_count "$SERIAL_FILE" "$POLL_TCP_TIMEOUT_LITERAL")
TTY_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$TTY_ORACLE_LITERAL")
TTY_ORACLE_FAIL_COUNT=$(marker_count "$SERIAL_FILE" "$TTY_ORACLE_FAIL_LITERAL")
BSSHD_COUNT=$(marker_count "$SERIAL_FILE" "$BSSHD_LITERAL")
ASID_CENSUS_COUNT=$(pattern_count "$SERIAL_FILE" "$ASID_CENSUS_PATTERN")
ASID_CENSUS_UNTAGGED_COUNT=$(pattern_count "$SERIAL_FILE" "$ASID_CENSUS_UNTAGGED_PATTERN")
ASID_CENSUS_PUBLISHED_COUNT=$(pattern_count "$SERIAL_FILE" "$ASID_CENSUS_PUBLISHED_PATTERN")
PINNED_CENSUS_COUNT=$(pattern_count "$SERIAL_FILE" "$PINNED_CENSUS_PATTERN")
PINNED_CENSUS_NONZERO_COUNT=$(pinned_nonzero_count "$SERIAL_FILE")
PINNED_FIRST_HOLD_COUNT=$(marker_count "$SERIAL_FILE" "$PINNED_FIRST_HOLD_LITERAL")
CRASH_COUNT=$(crash_count "$SERIAL_FILE")

if grep -qF '[BOOT_TESTS:FAIL' "$SERIAL_FILE" 2>/dev/null; then
    BOOT_TEST_FAIL_LINE=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' \
        "$SERIAL_FILE" 2>/dev/null | head -1 || true)
    echo "FAIL: boot test failure: ${BOOT_TEST_FAIL_LINE:-[TEST:<missing>:FAIL:<missing>]}"
    exit 1
fi

[ "$PROD_SEAM_ABSENT_COUNT" -eq 1 ] || {
    echo "FAIL: seam-absent timeout marker count must be exactly one"
    exit 1
}
[ "$KERNEL_ORACLE_COUNT" -eq 0 ] || {
    echo "FAIL: boot_tests-only kernel oracle marker was present"
    exit 1
}
[ "$SCHED_STRAND_ORACLE_COUNT" -eq 0 ] || {
    echo "FAIL: boot_tests-only scheduler strand oracle marker was present"
    exit 1
}
[ "$STRAND_INJECT_ORACLE_COUNT" -eq 0 ] || {
    echo "FAIL: boot_tests-only strand injection oracle marker was present"
    exit 1
}
[ "$FCNTL_PM_ORACLE_COUNT" -eq 0 ] || {
    echo "FAIL: boot_tests-only fcntl contention oracle marker was present"
    exit 1
}
[ "$INIT_EXIT_COUNT" -ge 1 ] || {
    echo "FAIL: init never resumed past futex_handoff_oracle"
    exit 1
}
[ "$BLOCK_EINTR_ORACLE_COUNT" -ge 1 ] || {
    echo "FAIL: Block EINTR oracle marker missing"
    exit 1
}
[ "$BLOCK_EINTR_ORACLE_FAIL_COUNT" -eq 0 ] || {
    echo "FAIL: Block EINTR oracle reported failure"
    exit 1
}
[ "$POLL_TCP_ORACLE_COUNT" -ge 1 ] || {
    echo "FAIL: Poll TCP oracle marker missing"
    exit 1
}
[ "$POLL_TCP_ORACLE_FAIL_COUNT" -eq 0 ] || {
    echo "FAIL: Poll TCP oracle reported failure: $(grep -aF "$POLL_TCP_ORACLE_FAIL_LITERAL" "$SERIAL_FILE" | tail -1)"
    exit 1
}
[ "$POLL_TCP_TIMEOUT_COUNT" -ge 1 ] || {
    echo "FAIL: Kernel poll timeout report (#693) never emitted"
    exit 1
}
[ "$POLL_TCP_READY_LOST_COUNT" -eq 0 ] || {
    echo "FAIL: Kernel reported a lost TCP readiness publication (#693): $(grep -aF "$POLL_TCP_READY_LOST_LITERAL" "$SERIAL_FILE" | tail -1)"
    exit 1
}
[ "$TTY_ORACLE_COUNT" -ge 1 ] || {
    echo "FAIL: TTY oracle marker missing - the shipped profile drove no TTY traffic"
    exit 1
}
[ "$TTY_ORACLE_FAIL_COUNT" -eq 0 ] || {
    echo "FAIL: TTY oracle reported failure: $(grep -aF "$TTY_ORACLE_FAIL_LITERAL" "$SERIAL_FILE" | tail -1)"
    exit 1
}
[ "$BSSHD_COUNT" -ge 1 ] || {
    echo "FAIL: bsshd never reached its listening state"
    exit 1
}
[ "$ASID_CENSUS_COUNT" -ge 1 ] || {
    echo "FAIL: TTBR0 ASID census marker missing"
    exit 1
}
[ "$ASID_CENSUS_UNTAGGED_COUNT" -eq 0 ] || {
    echo "FAIL: TTBR0 ASID census reported an untagged process-root publish: $(grep -aoE "$ASID_CENSUS_PATTERN" "$SERIAL_FILE" | grep -E ':untagged=[1-9]' | tail -1)"
    exit 1
}
[ "$ASID_CENSUS_PUBLISHED_COUNT" -ge 1 ] || {
    echo "FAIL: TTBR0 ASID census never counted a process-root publish, so untagged=0 says nothing"
    exit 1
}
[ "$PINNED_CENSUS_COUNT" -ge 1 ] || {
    echo "FAIL: pinned-placement census marker missing"
    exit 1
}
[ "$PINNED_CENSUS_NONZERO_COUNT" -eq 0 ] || {
    echo "FAIL: pinned-placement census reported a field above zero: $(grep -aoE "$PINNED_CENSUS_PATTERN" "$SERIAL_FILE" | grep -vxF "$PINNED_CENSUS_ZERO_LITERAL" | tail -1)"
    exit 1
}
[ "$PINNED_FIRST_HOLD_COUNT" -eq 0 ] || {
    echo "FAIL: a pinned worker's wake was held for want of a dispatching home CPU: $(grep -aF -m1 "$PINNED_FIRST_HOLD_LITERAL" "$SERIAL_FILE")"
    exit 1
}
[ "$CRASH_COUNT" -eq 0 ] || {
    echo "FAIL: crash marker detected"
    exit 1
}

echo "PASS: production profile reached bsshd with the futex oracle seam absent"
echo "Observed: $(grep -F -m 1 "$PROD_SEAM_ABSENT_LITERAL" "$SERIAL_FILE")"
echo "Observed: $(grep -F -m 1 "$INIT_EXIT_LITERAL" "$SERIAL_FILE")"
echo "Observed: $(grep -F -m 1 "$BSSHD_LITERAL" "$SERIAL_FILE")"
echo "Observed kernel oracle marker count: $KERNEL_ORACLE_COUNT"
echo "Observed fcntl contention oracle marker count: $FCNTL_PM_ORACLE_COUNT"
echo "Observed block EINTR oracle marker count: $BLOCK_EINTR_ORACLE_COUNT"
echo "Observed block EINTR oracle failure count: $BLOCK_EINTR_ORACLE_FAIL_COUNT"
echo "Observed poll TCP oracle marker count: $POLL_TCP_ORACLE_COUNT"
echo "Observed poll TCP oracle failure count: $POLL_TCP_ORACLE_FAIL_COUNT"
echo "Observed kernel poll timeout report count: $POLL_TCP_TIMEOUT_COUNT"
echo "Observed kernel lost-readiness report count: $POLL_TCP_READY_LOST_COUNT"
echo "Observed TTY oracle marker count: $TTY_ORACLE_COUNT"
echo "Observed TTY oracle failure count: $TTY_ORACLE_FAIL_COUNT"
echo "Observed TTBR0 ASID census marker count: $ASID_CENSUS_COUNT"
echo "Observed TTBR0 ASID census untagged-publish line count: $ASID_CENSUS_UNTAGGED_COUNT"
echo "Observed: $(grep -aoE "$ASID_CENSUS_PATTERN" "$SERIAL_FILE" | tail -1)"
echo "Observed pinned-placement census marker count: $PINNED_CENSUS_COUNT"
echo "Observed pinned-placement non-zero census line count: $PINNED_CENSUS_NONZERO_COUNT"
echo "Observed: $(grep -aoE "$PINNED_CENSUS_PATTERN" "$SERIAL_FILE" | tail -1)"
echo "Observed crash marker count: $CRASH_COUNT"
cleanup 0
