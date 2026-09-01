#!/bin/bash
#
# #728 ext2 lock-discipline gate.
#
# Boots a kernel built with `--features boot_tests,ext2_lock_race`, whose
# in-kernel leg (kernel/src/fs/ext2_lock_race.rs) deterministically
# constructs the #728 shape for BOTH mounted filesystems: a "holder" kthread
# acquires root_fs_read()/home_fs_read() and deliberately parks *while still
# holding the guard* (a scratch Completion that is never completed, so the
# wait always runs its full three-second deadline — no real device I/O or
# fault injection needed to force this), while contender kthreads
# concurrently attempt root_fs_write()/home_fs_write() and mkdir on success.
#
# Unfixed ext2 lock code cannot survive this: a contended acquisition
# busy-spins with preemption disabled, denying the CPU the timer ISR would
# otherwise use to dispatch the holder's own completion once it fires --
# hence the kernel's own EXT2_LOCK_SPIN_STALL marker
# (kernel/src/fs/ext2/mod.rs) and, downstream, its own soft-lockup detector.
# Fixed code parks contenders instead, resolves the race, and the leg prints
# a verdict line for each filesystem plus one COMPLETE tally.
#
# ---------------------------------------------------------------------------
# Why the verdict is read two ways
# ---------------------------------------------------------------------------
# On the pathological case this leg constructs, EVERY CPU ends up occupied by
# a non-yielding contender -- including the CPU running this leg's own
# driver thread, which is itself blocked in kthread_join() waiting for the
# holder. Nothing is left to print a "the test hung" verdict; the boot
# simply goes silent. So the RED signal is NOT "COMPLETE never printed" (a
# raw hang is also what "slow" looks like) -- it is the presence of
# EXT2_LOCK_SPIN_STALL, which fires from *inside* the still-executing spin
# itself, or the kernel's own soft-lockup detector, either of which the boot
# can produce even while otherwise wedged. A green run requires ALL of:
# no stall marker, no soft lockup, the leg's own COMPLETE tally with
# fail=0, and (non-anti-vacuity runs) the boot's normal liveness markers
# after it.
#
# ---------------------------------------------------------------------------
# Anti-vacuity
# ---------------------------------------------------------------------------
# The oracle's own red/green split was proven by hand across this fix's
# commits, not re-derived by this script every run (a script that reverted
# kernel source on every invocation would be its own hazard). The record, as
# actually observed (not aspired to) at the time this header was last edited:
#   - Observer-only commit (spin instrumented, no park path) + this same
#     harness: EXT2_LOCK_SPIN_STALL fires (x3, aarch64 -smp 4; x1, x86
#     -smp 1, both disks attached), the kernel's own soft-lockup detector
#     fires, and the boot never reaches its own liveness markers again -- on
#     BOTH arches, BOTH filesystems.
#   - The fix commit + the identical harness, aarch64: verdict=PASS for both
#     filesystems, COMPLETE:pass=2:fail=0 (both disks attached), zero stall
#     markers, and the boot continues live (heartbeats) long after.
#   - The fix commit + the identical harness, x86: NOT CAPTURED as a
#     COMPLETE/verdict line. Every GREEN attempt reached the leg (holder +
#     contender kthreads spawned, actively scheduled back and forth for as
#     long as observed, zero EXT2_LOCK_SPIN_STALL) but none reached
#     `[LOCKRACE:COMPLETE:...]` within the time budget spent -- x86's
#     `testing`-profile boot sits behind a slow pre-existing boot_tests
#     battery under unaccelerated TCG before the leg's own call site even
#     runs. This is reported honestly as "x86 RED captured, x86 GREEN not
#     captured" (see docs/planning/green-program/nic-bus/serials/728-prove/
#     x86-oracle/), not as a pass record. Do not restate it as one.
# Reproduce by hand: `git checkout <harness-fix commit> -- kernel/src/fs`
# reverts only the lock-discipline commit while keeping this same harness,
# rebuild, rerun this script -- it must redden the same way.
#
# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
#   docker/qemu/run-ext2-lock-race-gate.sh                # aarch64 (default)
#   docker/qemu/run-ext2-lock-race-gate.sh --x86           # x86_64 (beast)
#   docker/qemu/run-ext2-lock-race-gate.sh --no-build       # reuse the built kernel
#
# x86's full `testing` profile runs the same ~10+ minute userspace/teardown
# suite every other x86 boot_tests gate sits behind before reaching this
# leg's own call site; X86_POLL_BOUND defaults to 1800s to give it room.

set -euo pipefail
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

OUTPUT_DIR=""
report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    echo "ext2 lock-race gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "$OUTPUT_DIR" ] && compgen -G "$OUTPUT_DIR/serial*.txt" >/dev/null 2>&1; then
        echo "--- serial tail (last 120 lines per file, $OUTPUT_DIR) ---"
        tail -n 120 "$OUTPUT_DIR"/serial*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

ARCH="aarch64"
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --x86|--x86_64) ARCH="x86"; shift ;;
        --aarch64) ARCH="aarch64"; shift ;;
        --no-build) BUILD=0; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

FEATURES="boot_tests,ext2_lock_race"

cd "$BREENIX_ROOT"

echo "========================================="
echo "#728 ext2 lock-race gate"
echo "  arch:     $ARCH"
echo "  features: $FEATURES"
echo "========================================="

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
BUILD_LOG="/tmp/ext2-lock-race-gate-build.log"
if [ "$ARCH" = "aarch64" ]; then
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
    if [ "$BUILD" -eq 1 ]; then
        echo "[gate] building aarch64 kernel..."
        # The soft-float kernel target is mandatory; building the NEON target
        # here would re-arm #528 (see scripts/check-kernel-no-neon.sh).
        cargo build --release --features "$FEATURES" \
            --target aarch64-breenix-kernel.json \
            -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
            -p kernel --bin kernel-aarch64 >"$BUILD_LOG" 2>&1
    fi
    test -f "$KERNEL"
    "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL" >/dev/null
    EXT2_ROOT_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    test -f "$EXT2_ROOT_DISK"
    EXT2_HOME_DISK="$BREENIX_ROOT/target/ext2-home-aarch64.img"
    if [ ! -f "$EXT2_HOME_DISK" ]; then
        cp "$EXT2_ROOT_DISK" "$EXT2_HOME_DISK"
    fi
else
    if [ "$BUILD" -eq 1 ]; then
        echo "[gate] building x86_64 kernel..."
        cargo build --release --features "$FEATURES,testing,external_test_bins" \
            --bin qemu-uefi >"$BUILD_LOG" 2>&1
        BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
            --features "$FEATURES,testing,external_test_bins" --bin qemu-uefi >/dev/null
        # Repack both disks every run. Both are gitignored build outputs, so a
        # cached image silently boots the previous branch's binaries (#564).
        rm -f target/test_binaries.img
        cargo run -p xtask -- create-test-disk >/dev/null
        rm -f target/ext2.img
        ./scripts/create_ext2_disk.sh >/dev/null
    fi
    UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
    test -n "$UEFI_IMG"
    EXT2_HOME_DISK="$BREENIX_ROOT/target/ext2-home.img"
    if [ ! -f "$EXT2_HOME_DISK" ]; then
        cp "$BREENIX_ROOT/target/ext2.img" "$EXT2_HOME_DISK"
    fi
fi

# Zero-warning build, with one documented exclusion: cargo's
# "packages contain code that will be rejected by a future version of Rust"
# notice is emitted for the rustup-vendored `core` crate that -Z build-std
# compiles, not for anything in this repository. It is present on an unmodified
# tree and cannot be fixed here. Every other warning is a gate failure.
if [ "$BUILD" -eq 1 ] && [ -f "$BUILD_LOG" ]; then
    if grep -E "^(warning|error)" "$BUILD_LOG" \
        | grep -vF "contain code that will be rejected by a future version of Rust" \
        | grep -q .; then
        echo "ext2 lock-race gate: FAIL (build produced warnings/errors, see $BUILD_LOG)"
        grep -E "^(warning|error)" "$BUILD_LOG" \
            | grep -vF "contain code that will be rejected by a future version of Rust" | head -20
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Boot
# ---------------------------------------------------------------------------
OUTPUT_DIR="/tmp/breenix_ext2_lock_race_gate_$ARCH"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

if [ "$ARCH" = "aarch64" ]; then
    cp "$EXT2_ROOT_DISK" "$OUTPUT_DIR/ext2-root-writable.img"
    cp "$EXT2_HOME_DISK" "$OUTPUT_DIR/ext2-home-writable.img"
    timeout "${AARCH64_BOOT_TIMEOUT:-90}" qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2root \
        -drive if=none,id=ext2root,format=raw,file="$OUTPUT_DIR/ext2-root-writable.img" \
        -device virtio-blk-device,drive=ext2home \
        -drive if=none,id=ext2home,format=raw,file="$OUTPUT_DIR/ext2-home-writable.img" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    LIVENESS_PATTERN='(\[heartbeat\]|\[EXEC_SMOKE:TARGET_OK\]|\[bcheck\] Complete:|\[bwm\] Display:)'
else
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
    timeout "${X86_BOOT_TIMEOUT:-1800}" qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=homedisk,format=raw,readonly=on,file=$EXT2_HOME_DISK" \
        -device virtio-blk-pci,drive=homedisk,disable-modern=on,disable-legacy=off \
        -machine "pc,accel=${BREENIX_QEMU_ACCEL:-tcg}" -cpu "${BREENIX_QEMU_CPU:-qemu64}" -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial_user.txt" \
        -serial "file:$OUTPUT_DIR/serial_kernel.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    LIVENESS_PATTERN='USERSPACE TEST COMPLETE'
fi

# Poll for the leg's terminal marker (or a red signal that fires without it —
# see the header comment on why COMPLETE alone is not the red/green split)
# and, on a green run, the boot's own liveness marker after it.
STALL_SEEN=0
LOCKUP_SEEN=0
COMPLETE_SEEN=0
LIVE=0
POLL_BOUND=150
[ "$ARCH" = "x86" ] && POLL_BOUND="${X86_POLL_BOUND:-1800}"
for _ in $(seq 1 "$POLL_BOUND"); do
    if grep -qa "EXT2_LOCK_SPIN_STALL" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        STALL_SEEN=1
    fi
    if grep -qaE "soft lockup detected|SOFT LOCKUP DETECTED" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        LOCKUP_SEEN=1
    fi
    if grep -qa "\[LOCKRACE:COMPLETE:" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        COMPLETE_SEEN=1
    fi
    if grep -qaE "$LIVENESS_PATTERN" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        LIVE=1
    fi
    if grep -qaE "KERNEL PANIC" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        break
    fi
    if [ "$STALL_SEEN" -eq 1 ] || [ "$LOCKUP_SEEN" -eq 1 ]; then
        # Red signal fired. No point waiting out the rest of the timeout —
        # a boot that reaches this state does not reliably recover.
        break
    fi
    if [ "$COMPLETE_SEEN" -eq 1 ] && [ "$LIVE" -eq 1 ]; then
        break
    fi
    sleep 2
done
kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true

SERIAL_ALL="$OUTPUT_DIR/serial-all.txt"
cat "$OUTPUT_DIR"/serial*.txt >"$SERIAL_ALL" 2>/dev/null || true

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------
fail() {
    echo "ext2 lock-race gate ($ARCH): FAIL - $1"
    echo "--- LOCKRACE / stall lines ---"
    grep -a "LOCKRACE\|EXT2_LOCK_SPIN_STALL\|soft lockup\|SOFT LOCKUP" "$SERIAL_ALL" || echo "(none)"
    exit 1
}

if grep -qa "KERNEL PANIC" "$SERIAL_ALL"; then
    fail "kernel panic during or after the race leg"
fi
if grep -qaE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$SERIAL_ALL"; then
    fail "CPU exception during or after the race leg"
fi
if [ "$STALL_SEEN" -eq 1 ]; then
    fail "EXT2_LOCK_SPIN_STALL observed — a contended acquisition spun instead of parking (#728 live)"
fi
if [ "$LOCKUP_SEEN" -eq 1 ]; then
    fail "kernel soft-lockup detector fired during the race leg"
fi
if [ "$COMPLETE_SEEN" -ne 1 ]; then
    fail "the leg never reached its COMPLETE marker (hang with no stall/lockup signal caught, or it never ran)"
fi

COMPLETE_LINE="$(grep -a "\[LOCKRACE:COMPLETE:" "$SERIAL_ALL" | head -1)"
LEG_PASS="$(echo "$COMPLETE_LINE" | sed -n 's/.*pass=\([0-9]*\):fail=\([0-9]*\)\].*/\1/p')"
LEG_FAIL="$(echo "$COMPLETE_LINE" | sed -n 's/.*pass=\([0-9]*\):fail=\([0-9]*\)\].*/\2/p')"
echo "[gate] $COMPLETE_LINE"
grep -a "\[LOCKRACE:.*:race:verdict=" "$SERIAL_ALL" | sed 's/^/[gate]   /'

if [ -z "$LEG_PASS" ] || [ "$LEG_FAIL" != "0" ]; then
    fail "the leg reported $LEG_FAIL failing filesystem(s) (pass=$LEG_PASS)"
fi
[ "$LIVE" -eq 1 ] || fail "the boot did not reach its liveness marker after the race leg"

echo "ext2 lock-race gate ($ARCH): PASSED - $LEG_PASS filesystem(s) raced clean, kernel live after"
exit 0
