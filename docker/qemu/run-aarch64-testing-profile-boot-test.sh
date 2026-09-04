#!/bin/bash
# ARM64 `--features testing` profile boot gate.
#
# Round 7 ran this profile by hand and scored it with a grep list that had no
# soft-lockup term in it, so a serial carrying a five-second
# `!!! SOFT LOCKUP DETECTED !!!` dump was reported as a clean PASS (review
# finding R7-004). This script is that scoring, written down, with the lockup
# term in it.
#
# A boot that locks up is not scored clean here. The lockup is either
#   728-signature  -- at least one `EXT2_LOCK_SPIN_STALL` line precedes the
#                     lockup dump, which is the open #728 ext2 read-park
#                     livelock: attributed, reported in the verdict, and not
#                     this gate's red
#   UNATTRIBUTED   -- a lockup with no preceding stall line: a red in each case
# and either way the verdict line says which.
#
# Userspace panics and EL0 aborts are COUNTED and printed, not scored: the
# userspace test catalog deliberately faults (fcntl/nonblock/pipe2 negative
# tests). A panic inside the kernel, or an abort taken at EL1, is a red.
#
# Usage:
#   run-aarch64-testing-profile-boot-test.sh [N]          build, boot N times, score
#   run-aarch64-testing-profile-boot-test.sh --classify F score an existing serial F
#
# The two modes share one scoring function, so a serial committed as evidence is
# read back by the same code that scored it live.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

BOOT_SECONDS="${BREENIX_TESTING_PROFILE_BOOT_SECONDS:-45}"
OUTPUT_ROOT="${BREENIX_TESTING_PROFILE_OUTPUT_DIR:-/tmp/breenix_aarch64_testing_profile}"
FAILURE_ROOT="${BREENIX_TESTING_PROFILE_FAILURE_DIR:-/tmp/breenix_testing_profile_failures}"

# The loader marker and the catalog line: both must appear in each boot.
LOADED_LINE_RE='\[test\] Loaded [0-9]+/[0-9]+ test binaries'
MARKER_LINE='[test] Test processes loaded - will run via timer interrupts'
# The lockup dump, and the #728 signature that attributes one.
LOCKUP_LINE='!!! SOFT LOCKUP DETECTED !!!'
EXT2_STALL_LINE='EXT2_LOCK_SPIN_STALL'
# Kernel-side reds.
KERNEL_PANIC_RE='panicked at kernel/src/'
WEDGE_FAIL_LINE='block_wedge_oracle:FAIL'
STRAND_FIRST_LINE='SCHED_STRAND_FIRST'

# Counters, filled in by classify_serial.
REDS=0
LOCKUP_728=0
LOCKUP_UNATTRIBUTED=0
MARKER_OK=0
PANIC_TOTAL=0
CLASSIFIED=0

count_fixed() { grep -a -F -c "$2" "$1" 2>/dev/null || true; }
count_re()    { grep -a -E -c "$2" "$1" 2>/dev/null || true; }

# classify_serial <serial-file> <label>
# Prints one verdict line and updates the run counters. Returns 1 if the boot
# is red.
classify_serial() {
    local SERIAL="$1"
    local LABEL="$2"

    local LOADED MARKER LOCKUPS STALLS KPANIC WEDGE STRAND UPANIC EL1_ABORT EL0_ABORT
    LOADED=$(count_re "$SERIAL" "$LOADED_LINE_RE")
    MARKER=$(count_fixed "$SERIAL" "$MARKER_LINE")
    LOCKUPS=$(count_fixed "$SERIAL" "$LOCKUP_LINE")
    STALLS=$(count_fixed "$SERIAL" "$EXT2_STALL_LINE")
    KPANIC=$(count_re "$SERIAL" "$KERNEL_PANIC_RE")
    WEDGE=$(count_fixed "$SERIAL" "$WEDGE_FAIL_LINE")
    STRAND=$(count_fixed "$SERIAL" "$STRAND_FIRST_LINE")
    UPANIC=$(count_re "$SERIAL" "thread '.*' panicked at ")
    EL1_ABORT=$(count_re "$SERIAL" '\[(DATA|INSTRUCTION)_ABORT\].*from_el0=0')
    EL0_ABORT=$(count_re "$SERIAL" '\[(DATA|INSTRUCTION)_ABORT\].*from_el0=1')

    # Markers that CANNOT occur on this profile, and what it means if they do.
    #
    # `boot_continuation` returns before `launch_init_from_elf` under
    # `#[cfg(feature = "testing")]`, so the testing profile does not launch init
    # and does not compile the boot_tests suite. Two consequences are checkable:
    # init's `[BLOCK_EINTR_ORACLE:` marker and any `[BOOT_TESTS:FAIL` line must
    # both be absent. Either one appearing means the profile split moved, and
    # this gate is then scoring a different kernel than it claims to.
    local BOOT_TESTS_FAIL INIT_ORACLE
    BOOT_TESTS_FAIL=$(grep -a -F -c '[BOOT_TESTS:FAIL' "$SERIAL" 2>/dev/null || true)
    if [ "$BOOT_TESTS_FAIL" -gt 0 ]; then
        FAIL_REASON="the testing profile printed a boot_tests failure marker"
        echo "       $FAIL_REASON"
    fi
    INIT_ORACLE=$(count_fixed "$SERIAL" '[BLOCK_EINTR_ORACLE:')

    # Attribution of a lockup: does at least one ext2 stall line PRECEDE the
    # first lockup line? Line numbers, not mere presence -- a stall printed
    # after the dump would not explain it.
    local LOCKUP_CLASS="none"
    local STALLS_BEFORE=0
    local LOCKUP_LN
    if [ "$LOCKUPS" -gt 0 ]; then
        LOCKUP_LN=$(grep -a -n -F "$LOCKUP_LINE" "$SERIAL" | head -1 | cut -d: -f1)
        STALLS_BEFORE=$(grep -a -n -F "$EXT2_STALL_LINE" "$SERIAL" \
            | cut -d: -f1 | awk -v l="$LOCKUP_LN" '$1 < l' | wc -l | tr -d ' ')
        if [ "$STALLS_BEFORE" -gt 0 ]; then
            LOCKUP_CLASS="728-signature"
            LOCKUP_728=$((LOCKUP_728 + 1))
        else
            LOCKUP_CLASS="UNATTRIBUTED"
            LOCKUP_UNATTRIBUTED=$((LOCKUP_UNATTRIBUTED + 1))
        fi
    fi

    local RED_REASONS=""
    [ "$MARKER" -eq 0 ] && RED_REASONS="$RED_REASONS loader-marker-missing"
    [ "$LOADED" -eq 0 ] && RED_REASONS="$RED_REASONS loaded-line-missing"
    [ "$LOCKUP_CLASS" = "UNATTRIBUTED" ] && RED_REASONS="$RED_REASONS unattributed-lockup"
    [ "$KPANIC" -gt 0 ] && RED_REASONS="$RED_REASONS kernel-panic"
    [ "$WEDGE" -gt 0 ] && RED_REASONS="$RED_REASONS block-wedge-fail"
    [ "$STRAND" -gt 0 ] && RED_REASONS="$RED_REASONS strand-first"
    [ "$EL1_ABORT" -gt 0 ] && RED_REASONS="$RED_REASONS el1-abort"
    [ "$BOOT_TESTS_FAIL" -gt 0 ] && RED_REASONS="$RED_REASONS boot-tests-failure-marker"
    [ "$INIT_ORACLE" -gt 0 ] && RED_REASONS="$RED_REASONS init-oracle-marker-in-testing-profile"

    [ "$MARKER" -gt 0 ] && [ "$LOADED" -gt 0 ] && MARKER_OK=$((MARKER_OK + 1))
    PANIC_TOTAL=$((PANIC_TOTAL + UPANIC))
    CLASSIFIED=$((CLASSIFIED + 1))

    local BODY
    BODY="marker=$MARKER loaded=$LOADED lockup=$LOCKUP_CLASS stalls_before_lockup=$STALLS_BEFORE ext2_stalls=$STALLS kernel_panic=$KPANIC wedge_fail=$WEDGE strand_first=$STRAND el1_abort=$EL1_ABORT userspace_panic=$UPANIC userspace_abort=$EL0_ABORT boot_tests_fail=$BOOT_TESTS_FAIL init_oracle=$INIT_ORACLE"

    if [ -n "$RED_REASONS" ]; then
        REDS=$((REDS + 1))
        echo "[FAIL] $LABEL:$RED_REASONS -- $BODY"
        return 1
    fi
    echo "[OK] $LABEL: $BODY"
    return 0
}

print_summary() {
    echo ""
    echo "Testing profile, $CLASSIFIED boots:"
    echo "  marker + loaded: $MARKER_OK/$CLASSIFIED"
    echo "  post-loader lockup, #728-signature: $LOCKUP_728/$CLASSIFIED"
    echo "  post-loader lockup, UNATTRIBUTED: $LOCKUP_UNATTRIBUTED/$CLASSIFIED"
    echo "  userspace panics (counted, not scored): $PANIC_TOTAL"
    echo "  reds: $REDS/$CLASSIFIED"
}

# --- offline mode: score serials that already exist ------------------------
if [ "${1:-}" = "--classify" ]; then
    shift
    if [ "$#" -eq 0 ]; then
        echo "FAIL: --classify needs at least one serial file"
        exit 2
    fi
    for f in "$@"; do
        if [ ! -f "$f" ]; then
            echo "FAIL: no such serial: $f"
            exit 2
        fi
        classify_serial "$f" "$(basename "$f")" || true
    done
    print_summary
    [ "$REDS" -eq 0 ] || exit 1
    exit 0
fi

ITERATIONS="${1:-3}"

echo "Building the ARM64 testing profile..."
if ! (cd "$BREENIX_ROOT" && cargo build --release --features testing \
        --target aarch64-breenix-kernel.json -Z build-std=core,alloc \
        -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64); then
    echo "FAIL: testing-profile kernel build failed"
    exit 1
fi

KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "FAIL: testing-profile kernel missing at $KERNEL"
    exit 1
fi

# Durable #528 guard: this profile ships on the soft-float target like the others.
if ! "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"; then
    echo "FAIL: testing-profile kernel failed the no-NEON guard"
    exit 1
fi

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "FAIL: ext2 disk not found at $EXT2_DISK"
    echo "Build it with: userspace/programs/build.sh --arch aarch64 && scripts/create_ext2_disk.sh --arch aarch64"
    exit 1
fi

for i in $(seq 1 "$ITERATIONS"); do
    OUTPUT_DIR="$OUTPUT_ROOT/$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    SERIAL="$OUTPUT_DIR/serial.txt"
    : > "$SERIAL"
    cp "$EXT2_DISK" "$OUTPUT_DIR/ext2-writable.img"

    timeout "$BOOT_SECONDS" qemu-system-aarch64 \
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
        -serial file:"$SERIAL" > "$OUTPUT_DIR/qemu-stdout.log" 2>&1 || true

    if ! classify_serial "$SERIAL" "Testing boot $i"; then
        mkdir -p "$FAILURE_ROOT"
        FAILED_COPY="$FAILURE_ROOT/$(date -u +%Y%m%dT%H%M%SZ)-boot$i.txt"
        cp "$SERIAL" "$FAILED_COPY"
        echo "       preserved serial: $FAILED_COPY"
    fi
done

print_summary

if [ "$REDS" -eq 0 ]; then
    if [ "$LOCKUP_728" -gt 0 ]; then
        echo "PASS-WITH-ATTRIBUTED-LOCKUP: $CLASSIFIED/$CLASSIFIED boots reached the loader marker; $LOCKUP_728 locked up afterwards with the #728 signature"
    else
        echo "PASS: $CLASSIFIED/$CLASSIFIED boots reached the loader marker with no lockup"
    fi
    exit 0
fi

echo "FAIL: $REDS/$CLASSIFIED testing-profile boots were red"
exit 1
