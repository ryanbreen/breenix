#!/bin/bash
#
# ext2/VFS fault-injection gate.
#
# Every other boot gate in this tree proves the filesystem works when the block
# device answers correctly. This one is the other half: it boots a kernel built
# with `--features fs_fault_inject`, whose in-kernel leg
# (kernel/src/fs/fault_inject.rs) drives the ext2 read path through three
# block-layer fault shapes, and holds each to a stated expectation.
#
#   short_read       device returns Ok having filled only the first 16 bytes of
#                    the superblock sector  ->  the mount must be REFUSED
#   eio_data_block   device returns Err(IoError) for the root directory's own
#                    first data block       ->  the error must reach the caller,
#                                               and the same read must succeed
#                                               again once the device recovers
#   corrupt_inode    device returns Ok with the inode record rewritten, in two
#                    arms (an implausible size, and wild block pointers)
#                                           ->  both must produce Err
#
# and, common to all of them: no panic, no hang, and the kernel still live
# afterwards -- proven by requiring the boot to reach its normal completion
# markers AFTER the leg, not merely by the leg's own say-so.
#
# ---------------------------------------------------------------------------
# Anti-vacuity
# ---------------------------------------------------------------------------
# A leg that stopped injecting would print PASS forever. `--disarm <shape>`
# rebuilds with that shape's arming compiled to a no-op and requires:
#
#   * the disarmed shape reports verdict=FAIL with detail=fault-not-observed*,
#   * every OTHER shape still reports verdict=PASS (the mutation is single), and
#   * the gate's own armed verdict goes red.
#
# So a green run of this gate means the faults were injected AND detected; the
# two are the same observation, because "not injected" and "not detected" both
# surface as the same FAIL line.
#
# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
#   docker/qemu/run-fs-fault-gate.sh                       # aarch64, armed
#   docker/qemu/run-fs-fault-gate.sh --x86                 # x86_64, armed (beast)
#   docker/qemu/run-fs-fault-gate.sh --disarm short_read   # anti-vacuity leg
#   docker/qemu/run-fs-fault-gate.sh --disarm eio
#   docker/qemu/run-fs-fault-gate.sh --disarm corrupt_inode
#   docker/qemu/run-fs-fault-gate.sh --no-build            # reuse the built kernel

set -euo pipefail
# errtrace, so the ERR trap below is inherited into functions. Without an
# explicit trap, `set -e` kills this script silently and the only diagnosis
# channel for a red gate is reading raw serial by hand (#668).
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/#865/R181: serializes QEMU boots host-wide, one lock domain per QEMU
# binary -- see the ARCH branch below, which acquires the aarch64 or x86
# domain depending on which leg is running. #865 closed the gap this
# comment used to disclose (the x86 leg reaching beast with no lock of its
# own): the shared qemu_host_lock_release call after the poll loop below is
# a no-op on a leg that skipped its own acquire, so no ARCH-conditional
# release was needed to add the x86 leg's own acquire.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"

# #797: concurrent lanes sharing one host (e.g. the beast Incus container,
# reached here via --x86) each invoking this script hardcode the identical
# /tmp/breenix_fs_fault_gate* path, so one lane's rm -rf/mkdir can clobber
# another lane's in-flight run. Defaulting to /tmp keeps every existing
# caller byte-identical; a concurrent-lane launcher sets this to a per-clone
# directory instead. Must be absolute -- a relative value would resolve
# against whatever directory happens to be current when each command runs,
# and the ERR trap below can fire before OUTPUT_DIR is even assigned its
# real value (review finding F6 on #797).
# claim-lint:ok: #797, diff-empty against origin/main once substituted -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
#
# The VALUE is not judged here -- that check is spent in the BASE-DIR
# PREFLIGHT block right after the ERR trap is installed below (#802/#805
# idiom, widened to this gate: a bare `exit` here would run before the trap
# exists and could end the gate with no verdict line at all).
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"

OUTPUT_DIR=""
report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    echo "fs fault gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "$OUTPUT_DIR" ] && compgen -G "$OUTPUT_DIR/serial*.txt" >/dev/null 2>&1; then
        echo "--- serial tail (last 120 lines per file, $OUTPUT_DIR) ---"
        tail -n 120 "$OUTPUT_DIR"/serial*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

# `return N` inside a function called as a plain statement fires the ERR
# trap with `$?` equal to `N` (unlike a bare `exit N`, which escapes it --
# see the "Verified" section of
# docs/planning/green-program/gates/GATE-VERDICT-DISCIPLINE-2026-09-05.md).
# This lets the two usage-error sites below preserve their pre-existing
# `exit 2` contract (distinct from a runtime FAIL's `exit 1`) without a bare
# `exit` statement; report_gate_failure's own `exit "$exit_code"` re-raise
# stays the only literal `exit` in this script.
redden() {
    return "$1"
}

# --- BASE-DIR PREFLIGHT (#797 F6, routed through the verdict path by the
# #802/#805 idiom widened to this gate) ---
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "fs fault gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac

ARCH="aarch64"
DISARM=""
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --x86|--x86_64) ARCH="x86"; shift ;;
        --aarch64) ARCH="aarch64"; shift ;;
        --disarm) DISARM="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        *) echo "unknown argument: $1" >&2; redden 2 ;;
    esac
done

DISARM_FEATURE=""
case "$DISARM" in
    "") ;;
    short_read)     DISARM_FEATURE="fs_fault_disarm_short_read" ;;
    eio)            DISARM_FEATURE="fs_fault_disarm_eio" ;;
    corrupt_inode)  DISARM_FEATURE="fs_fault_disarm_corrupt_inode" ;;
    *) echo "unknown shape: $DISARM (expected short_read, eio or corrupt_inode)" >&2; redden 2 ;;
esac

# The arms the leg must report, in the order it reports them. This is the
# gate's contract with the kernel: a shape that stops being driven disappears
# from the serial and is caught here, and a new arm has to be added here
# deliberately rather than sliding in unscored.
REQUIRED_ARMS=(
    "baseline_mount"
    "baseline_read"
    "short_read"
    "eio_data_block"
    "eio_recovery"
    "corrupt_inode:arm=size"
    "corrupt_inode:arm=blocks"
    "liveness"
)

# Which arms a given disarm is allowed to redden. eio disarms the fault, so the
# recovery arm still passes (the device was never made to fail); the corrupt
# shape has two arms and disarming it reddens both.
disarm_expected_red() {
    case "$DISARM" in
        short_read)    echo "short_read" ;;
        eio)           echo "eio_data_block" ;;
        corrupt_inode) echo "corrupt_inode:arm=size corrupt_inode:arm=blocks" ;;
    esac
}

cd "$BREENIX_ROOT"

FEATURES="boot_tests,fs_fault_inject"
if [ -n "$DISARM_FEATURE" ]; then
    FEATURES="$FEATURES,$DISARM_FEATURE"
fi

echo "========================================="
echo "ext2/VFS fault-injection gate"
echo "  arch:     $ARCH"
echo "  features: $FEATURES"
if [ -n "$DISARM" ]; then
    echo "  mode:     anti-vacuity (disarm $DISARM)"
else
    echo "  mode:     armed"
fi
echo "========================================="

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
BUILD_LOG="/tmp/fs-fault-gate-build.log"
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
    EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    test -f "$EXT2_DISK"
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
        echo "fs fault gate: FAIL (build produced warnings/errors, see $BUILD_LOG)"
        grep -E "^(warning|error)" "$BUILD_LOG" \
            | grep -vF "contain code that will be rejected by a future version of Rust" | head -20
        false
    fi
fi

# ---------------------------------------------------------------------------
# Boot
# ---------------------------------------------------------------------------
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_fs_fault_gate${DISARM:+_disarm_$DISARM}_$ARCH"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

if [ "$ARCH" = "aarch64" ]; then
    cp "$EXT2_DISK" "$OUTPUT_DIR/ext2-writable.img"
    qemu_host_lock_acquire
    timeout 60 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$OUTPUT_DIR/ext2-writable.img" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    # F2: registers this PID with the aarch64 host lock's own EXIT trap so a
    # SIGTERM/SIGINT delivered to just this process during the poll below
    # still kills QEMU instead of orphaning it with the lock free.
    qemu_host_lock_track_pid "$QEMU_PID"
    # The boot must reach its normal userspace completion markers AFTER the leg;
    # that is the leg's liveness proof, not the leg's own final line.
    LIVENESS_PATTERN='(\[heartbeat\]|\[EXEC_SMOKE:TARGET_OK\]|\[bcheck\] Complete:|\[bwm\] Display:)'
else
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
    qemu_host_lock_acquire qemu-system-x86_64
    timeout "${X86_BOOT_TIMEOUT:-1800}" qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -machine "pc,accel=${BREENIX_QEMU_ACCEL:-tcg}" -cpu "${BREENIX_QEMU_CPU:-qemu64}" -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial_user.txt" \
        -serial "file:$OUTPUT_DIR/serial_kernel.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    # F2: registers this PID with the x86 host lock's own EXIT trap, the
    # same reason the aarch64 branch above does.
    qemu_host_lock_track_pid "$QEMU_PID"
    LIVENESS_PATTERN='USERSPACE TEST COMPLETE'
fi

# Poll for the leg's terminal marker and, after it, the boot's own liveness
# marker. Both must appear; the leg finishing is not evidence the kernel is
# still usable.
LEG_DONE=0
LIVE=0
# The aarch64 profile reaches its liveness marker in tens of seconds. The x86
# full-test profile runs a ten-minute userspace suite (networking, loopback, COW)
# before its terminal marker, and takes materially longer when the host is busy:
# a 900-second bound passed on an idle beast and timed out on a contended one,
# with the leg itself green in both. The deadline is generous rather than tight,
# because a slow-but-healthy boot scored as failed is a false red.
POLL_BOUND=150
[ "$ARCH" = "x86" ] && POLL_BOUND="${X86_POLL_BOUND:-900}"
for _ in $(seq 1 "$POLL_BOUND"); do
    if grep -qa "\[FSFAULT:.*:COMPLETE:" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        LEG_DONE=1
    fi
    if grep -qaE "$LIVENESS_PATTERN" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        LIVE=1
    fi
    if grep -qaE "KERNEL PANIC" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        break
    fi
    if [ "$LEG_DONE" -eq 1 ] && [ "$LIVE" -eq 1 ]; then
        break
    fi
    # An anti-vacuity run is scored entirely on the leg's own arms -- it asserts
    # that a disarmed shape reddens, not that the boot completes -- so it need
    # not sit through the rest of a ten-minute userspace suite.
    if [ -n "$DISARM" ] && [ "$LEG_DONE" -eq 1 ]; then
        break
    fi
    sleep 2
done
kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
qemu_host_lock_release

SERIAL_ALL="$OUTPUT_DIR/serial-all.txt"
cat "$OUTPUT_DIR"/serial*.txt >"$SERIAL_ALL" 2>/dev/null || true

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------
fail() {
    echo "fs fault gate ($ARCH${DISARM:+, disarm $DISARM}): FAIL - $1"
    echo "--- FSFAULT lines ---"
    grep -a "\[FSFAULT:" "$SERIAL_ALL" || echo "(none)"
    false
}

arm_verdict() {
    # Prints PASS / FAIL / MISSING for one arm.
    local arm="$1" line
    line="$(grep -a "\[FSFAULT:[^:]*:${arm}:" "$SERIAL_ALL" | head -1 || true)"
    if [ -z "$line" ]; then echo "MISSING"; return; fi
    case "$line" in
        *verdict=PASS*) echo "PASS" ;;
        *verdict=FAIL*) echo "FAIL" ;;
        *) echo "MISSING" ;;
    esac
}

# Crash checks come FIRST. A fault that kills the kernel also stops the leg
# reaching COMPLETE, and "never reached COMPLETE" would name the symptom while
# the panic line scrolled past -- which is exactly the diagnosis-channel failure
# #668 was about. Measured: removing the ext2 i_size bound makes the
# corrupt_inode size arm panic, and this ordering is what says so.
if grep -qa "KERNEL PANIC" "$SERIAL_ALL"; then
    fail "kernel panic during or after the fault leg"
fi
if grep -qaE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected)" "$SERIAL_ALL"; then
    fail "CPU exception or soft lockup during or after the fault leg"
fi
if [ "$LEG_DONE" -ne 1 ]; then
    fail "the leg never reached its COMPLETE marker (hang, or it never ran)"
fi

COMPLETE_LINE="$(grep -a "\[FSFAULT:.*:COMPLETE:" "$SERIAL_ALL" | head -1)"
LEG_PASS="$(echo "$COMPLETE_LINE" | sed -n 's/.*:COMPLETE:pass=\([0-9]*\):fail=\([0-9]*\)\].*/\1/p')"
LEG_FAIL="$(echo "$COMPLETE_LINE" | sed -n 's/.*:COMPLETE:pass=\([0-9]*\):fail=\([0-9]*\)\].*/\2/p')"
OBSERVED_PASS="$(grep -ac "\[FSFAULT:.*verdict=PASS" "$SERIAL_ALL" || true)"
OBSERVED_FAIL="$(grep -ac "\[FSFAULT:.*verdict=FAIL" "$SERIAL_ALL" || true)"
echo "[gate] $COMPLETE_LINE"
echo "[gate] observed verdict lines: PASS=$OBSERVED_PASS FAIL=$OBSERVED_FAIL"

# The tally the kernel reports and the lines it actually printed must agree; a
# silently-skipped arm would otherwise show up as neither.
if [ "$LEG_PASS" != "$OBSERVED_PASS" ] || [ "$LEG_FAIL" != "$OBSERVED_FAIL" ]; then
    fail "the leg's own tally ($LEG_PASS/$LEG_FAIL) disagrees with the lines it printed ($OBSERVED_PASS/$OBSERVED_FAIL)"
fi

EXPECTED_RED="$(disarm_expected_red)"
BAD=0
for arm in "${REQUIRED_ARMS[@]}"; do
    verdict="$(arm_verdict "$arm")"
    expected="PASS"
    for red in $EXPECTED_RED; do
        [ "$arm" = "$red" ] && expected="FAIL"
    done
    printf '[gate]   %-24s %s (expected %s)\n' "$arm" "$verdict" "$expected"
    [ "$verdict" != "$expected" ] && BAD=1
done

# Both legs converge here rather than each taking its own `exit 0`: an early
# success exit does not print anything a fallthrough wouldn't, but it is
# still an `exit` this gate's widened verdict-discipline ratchet forbids
# outside the trap's own re-raise (#802/#805 idiom). Falling through to the
# end of the script (status 0 from the last command) is what
# run-x86-prod-profile-boot-test.sh's own PASS path already does.
if [ -n "$DISARM" ]; then
    # Anti-vacuity leg: the disarmed shape MUST have reddened, with the leg
    # naming the reason as a fault that never happened, and nothing else may
    # have moved.
    [ "$BAD" -eq 0 ] || fail "disarming '$DISARM' did not produce exactly the expected red arms"
    for red in $EXPECTED_RED; do
        grep -a "\[FSFAULT:[^:]*:${red}:" "$SERIAL_ALL" | grep -qa "detail=fault-not-observed" \
            || fail "arm '$red' went red for some reason other than the missing fault"
    done
    [ "$LEG_FAIL" -ge 1 ] || fail "the leg reported no failures at all with '$DISARM' disarmed"
    echo "fs fault gate ($ARCH): ANTI-VACUITY PASSED - disarming '$DISARM' reddens exactly that shape"
else
    [ "$BAD" -eq 0 ] || fail "one or more arms did not report the required verdict"
    [ "$LEG_FAIL" -eq 0 ] || fail "the leg reported $LEG_FAIL failing arm(s)"
    [ "$LIVE" -eq 1 ] || fail "the boot did not reach its liveness marker after the fault leg"
    echo "fs fault gate ($ARCH): PASSED - ${#REQUIRED_ARMS[@]} arms green, kernel live after every injected fault"
fi
