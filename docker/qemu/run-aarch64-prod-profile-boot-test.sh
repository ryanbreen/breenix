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

# -110 is -ETIMEDOUT. Exactly one occurrence proves the unarmed kernel honoured
# the driver's probe timeout and that the seam-absent path actually executed.
PROD_SEAM_ABSENT_LITERAL='[FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe=-110]'
# This boot_tests-only kernel marker must be wholly absent from the unarmed run.
KERNEL_ORACLE_LITERAL='[FUTEX_HANDOFF_ORACLE:'
# These boot_tests-only scheduler strand markers must also be absent from the
# shipped production profile.
SCHED_STRAND_ORACLE_LITERAL='[SCHED_STRAND_ORACLE:'
STRAND_INJECT_ORACLE_LITERAL='[STRAND_INJECT_ORACLE:'
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
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected'

OUTPUT_DIR="/tmp/breenix_aarch64_prod_profile"
SERIAL_FILE="$OUTPUT_DIR/serial.txt"
QEMU_PID=""

marker_count() {
    local serial_file="$1"
    local literal="$2"
    if [ ! -f "$serial_file" ]; then
        echo 0
        return
    fi
    grep -F -c "$literal" "$serial_file" 2>/dev/null || true
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
        failure_dir="/tmp/breenix_prod_profile_failures/$timestamp"
        while [ -e "$failure_dir" ]; do
            sleep 1
            timestamp=$(date -u +%Y%m%dT%H%M%SZ)
            failure_dir="/tmp/breenix_prod_profile_failures/$timestamp"
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

PROD_SEAM_ABSENT_COUNT=$(marker_count "$SERIAL_FILE" "$PROD_SEAM_ABSENT_LITERAL")
KERNEL_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$KERNEL_ORACLE_LITERAL")
SCHED_STRAND_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$SCHED_STRAND_ORACLE_LITERAL")
STRAND_INJECT_ORACLE_COUNT=$(marker_count "$SERIAL_FILE" "$STRAND_INJECT_ORACLE_LITERAL")
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
[ "$CRASH_COUNT" -eq 0 ] || {
    echo "FAIL: crash marker detected"
    exit 1
}

echo "PASS: production profile reached bsshd with the futex oracle seam absent"
echo "Observed: $(grep -F -m 1 "$PROD_SEAM_ABSENT_LITERAL" "$SERIAL_FILE")"
echo "Observed: $(grep -F -m 1 "$INIT_EXIT_LITERAL" "$SERIAL_FILE")"
echo "Observed: $(grep -F -m 1 "$BSSHD_LITERAL" "$SERIAL_FILE")"
echo "Observed kernel oracle marker count: $KERNEL_ORACLE_COUNT"
echo "Observed block EINTR oracle marker count: $BLOCK_EINTR_ORACLE_COUNT"
echo "Observed block EINTR oracle failure count: $BLOCK_EINTR_ORACLE_FAIL_COUNT"
echo "Observed poll TCP oracle marker count: $POLL_TCP_ORACLE_COUNT"
echo "Observed poll TCP oracle failure count: $POLL_TCP_ORACLE_FAIL_COUNT"
echo "Observed kernel poll timeout report count: $POLL_TCP_TIMEOUT_COUNT"
echo "Observed kernel lost-readiness report count: $POLL_TCP_READY_LOST_COUNT"
echo "Observed TTY oracle marker count: $TTY_ORACLE_COUNT"
echo "Observed TTY oracle failure count: $TTY_ORACLE_FAIL_COUNT"
echo "Observed crash marker count: $CRASH_COUNT"
cleanup 0
