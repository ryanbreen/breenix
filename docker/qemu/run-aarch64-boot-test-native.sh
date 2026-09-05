#!/bin/bash
# Native ARM64 boot test (runs QEMU directly on host)
# Much faster than Docker version but only works on macOS ARM64
#
# The retry mechanism provides robustness for local testing against
# transient host resource contention. If retries are frequently needed,
# investigate for potential regressions.
#
# Usage: ./run-aarch64-boot-test-native.sh

set -e

MAX_RETRIES=5
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #825: two concurrent runs of this gate on the same host each hardcoded the
# identical /tmp/breenix_aarch64_boot_native path, so one run's rm -rf/mkdir
# could delete and rewrite the serial another run's poll loop was mid-boot
# scoring. Defaulting to /tmp keeps a caller that leaves it unset byte-identical; a
# concurrent-lane launcher sets this to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when run_single_test runs (the same F6
# guard PR #801 gave the x86 gate scripts for #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "ARM64 BOOT TEST: FAILED (BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP)"; exit 1 ;;
esac
INIT_GROUP_REFUSAL_ORACLE_LITERAL='[INIT_GROUP_REFUSAL_ORACLE:aarch64:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]'

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    # require_boot_tests_kernel() below refuses a kernel that lacks
    # --features boot_tests, so the hint printed here must build one --
    # the same reasoning as run-aarch64-boot-test-strict.sh's matching
    # "No ARM64 kernel found" arm.
    echo "  cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    echo ""
    echo "========================================="
    echo "ARM64 BOOT TEST: FAILED (no ARM64 kernel found)"
    echo "========================================="
    exit 1
fi

# Durable #528 guard: the kernel MUST be soft-float. Fail fast if it was built
# with the NEON hardfloat target (aarch64-breenix.json) — that re-arms #528.
# (set -e aborts the test if the guard trips.)
"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

# Durable feature-profile guard, the same shape as
# run-aarch64-boot-test-strict.sh's require_boot_tests_kernel(). This gate
# pins INIT_GROUP_REFUSAL_ORACLE_LITERAL and the INIT_GROUP_WALK marker below.
# claim-lint:ok: both are emitted only from functions marked
# #[cfg(feature = "boot_tests")] -- init_group_refusal_oracle_test() at
# kernel/src/tracing/providers/teardown.rs:5125-5126 and
# emit_init_group_walk() at kernel/src/tracing/providers/teardown.rs:1422-1423
# -- inside a registry+executor module tree gated on the same feature at
# kernel/src/test_framework/mod.rs:60-66, so a kernel compiled without that
# feature does not contain the code that prints either marker. A kernel built
# without --features boot_tests therefore cannot print them, so every one of
# the MAX_RETRIES boots below would time out on "marker missing" -- a
# structural mismatch between this gate and the kernel's build profile, not a
# kernel red.
#
# The missing-marker arm below prints this script's own PASSED/FAILED verdict
# banner before exiting instead of a bare, unformatted exit.
# claim-lint:ok: the ratchet at
# tests/strand_handoff_structure.rs::boot_tests_gates_refuse_a_wrong_profile_kernel
# ("native gate" entry) proves the missing-marker arm has exactly one
# `exit 1` line and reddens if that count changes (see this branch's
# NATIVE-GATE-GUARD-2026-09-05.md, "Mutation proof the added census line is
# load-bearing"). It does NOT independently check for the banner echoes
# above that line -- deleting them alone leaves the ratchet green -- so the
# banner's presence is a maintained convention matching the other two
# preflight arms below, not itself a ratcheted invariant.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of marker literals rather than one sentinel: a single marker
    # changing profile must not be able to disarm this guard quietly.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[CENSUS_WIDEN_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[TOMBSTONE_JOIN_ORACLE:' '[BOOT_TESTS:'; do
        if ! grep -aqF "$marker" "$kernel" 2>/dev/null; then
            missing="$missing $marker"
        fi
    done

    if [ -n "$missing" ]; then
        echo "Error: $kernel was not built with --features boot_tests."
        echo "  Missing boot_tests-only marker literal(s):$missing"
        echo "  This gate pins INIT_GROUP_REFUSAL_ORACLE_LITERAL and the INIT_GROUP_WALK"
        echo "  marker, both boot_tests-only, so every boot below would fail on"
        echo "  'marker missing' after $MAX_RETRIES retries -- not a kernel red."
        echo "  Rebuild with:"
        echo "    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
        echo ""
        echo "========================================="
        echo "ARM64 BOOT TEST: FAILED (kernel is not a boot_tests build)"
        echo "========================================="
        exit 1
    fi
}

require_boot_tests_kernel "$KERNEL"

# Find ext2 disk (required for userspace)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    echo ""
    echo "========================================="
    echo "ARM64 BOOT TEST: FAILED (ext2 disk not found)"
    echo "========================================="
    exit 1
fi

# Check serial output for crash markers. Prints the crash type and returns 0
# if a crash is found, 1 if clean.
check_crash_markers() {
    local serial_file="$1"
    [ -f "$serial_file" ] || return 1
    if grep -qiE "(KERNEL PANIC|panic!)" "$serial_file" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$serial_file" 2>/dev/null; then
        echo "CPU exception"
        return 0
    fi
    if grep -qiE "soft lockup detected" "$serial_file" 2>/dev/null; then
        echo "Soft lockup"
        return 0
    fi
    if grep -qE "\[EXEC_LOCK_ORDER:VIOLATION" "$serial_file" 2>/dev/null; then
        echo "Exec lock-order violation"
        return 0
    fi
    # This profile does not run the boot-test oracle that emits the injected
    # marker, so it pins only the forbidden [CREATION_LOCK_ORDER:VIOLATION:PM_HELD].
    if grep -qE "\[CREATION_LOCK_ORDER:VIOLATION" "$serial_file" 2>/dev/null; then
        echo "Creation lock-order violation"
        return 0
    fi
    if grep -qE "\[EXEC_SMOKE:(EXEC_FAILED|TARGET_ARGV_FAIL|SPAWN_FAILED)" "$serial_file" 2>/dev/null; then
        echo "Exec smoke failure"
        return 0
    fi
    return 1
}

run_single_test() {
    local OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_boot_native"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 30s timeout.
    # Breenix ARM64 expects a GICv3 CPU interface, matching Parallels.
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
    #
    # BREENIX_AHCI=1 switches from virtio-blk to AHCI (SATA) disk to reproduce
    # the AHCI interrupt storm that kills CPU 0's timer on Parallels.
    local DISK_DEVICE_OPTS
    if [ "${BREENIX_AHCI:-0}" = "1" ]; then
        DISK_DEVICE_OPTS="-device ahci,id=ahci0 -device ide-hd,drive=ext2,bus=ahci0.0"
    else
        DISK_DEVICE_OPTS="-device virtio-blk-device,drive=ext2"
    fi
    timeout 30 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        $DISK_DEVICE_OPTS \
        -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" &
    local QEMU_PID=$!

    # Wait for USERSPACE boot completion and the init-driven exec smoke (24s timeout)
    # Accept any of:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    #   "[bcheck] Complete:" - bcheck self-test suite finished (headless/no-VirGL mode)
    #   "[heartbeat]" - the default ARM64 init service executed in userspace
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local USERSPACE_COMPLETE=false
    local EXEC_SMOKE_COMPLETE=false
    local EXEC_FIRST_COMMIT=false
    local BOOT_COMPLETE=false
    local CRASH_TYPE=""
    for i in $(seq 1 12); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -qE "(breenix>|bsh |\[bwm\] Display:|\[bcheck\] Complete:|\[heartbeat\])" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                USERSPACE_COMPLETE=true
            fi
            if grep -q "\[EXEC_SMOKE:TARGET_OK\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                EXEC_SMOKE_COMPLETE=true
            fi
            if grep -q "\[EXEC_LOCK_ORDER:FIRST_COMMIT\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                EXEC_FIRST_COMMIT=true
            fi
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
            if $USERSPACE_COMPLETE && $EXEC_SMOKE_COMPLETE && $EXEC_FIRST_COMMIT; then
                BOOT_COMPLETE=true
                break
            fi
        fi
        sleep 2
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    # Even if boot appeared successful, scan for crash markers that may have
    # appeared after the shell prompt (e.g., a child process crashed).
    if $BOOT_COMPLETE; then
        if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
            local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            echo "FAIL: $CRASH_TYPE after boot ($LINES lines)"
            return 1
        fi
        if grep -qF '[BOOT_TESTS:FAIL' "$OUTPUT_DIR/serial.txt" 2>/dev/null \
            || grep -qE '\[TESTS_COMPLETE:[^]]*:FAILED:[1-9][0-9]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOOT_TEST_FAIL_LINE=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' \
                "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -1 || true)
            echo "FAIL: boot test failure: ${BOOT_TEST_FAIL_LINE:-[TEST:<missing>:FAIL:<missing>]}"
            return 1
        fi
        if ! grep -qF "[BLOCK_EINTR_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: block EINTR oracle marker missing"
            return 1
        fi
        if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: block EINTR oracle reported failure"
            return 1
        fi
        # #568: the blocking-poll-on-connected-TCP oracle runs from init before
        # exec smoke, whose marker this gate already requires, so it has always
        # completed by the time this check runs. Pinned as a pair for the same
        # reason as its #575 peer above.
        if ! grep -qF "[POLL_TCP_ORACLE:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: poll TCP oracle marker missing"
            return 1
        fi
        if grep -qF "[POLL_TCP_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: poll TCP oracle reported failure ($(grep -aoF -m1 "[POLL_TCP_ORACLE:FAIL" "$OUTPUT_DIR/serial.txt"))"
            return 1
        fi
        # #693: the kernel's own contradiction check. `[POLL_TCP_READY_LOST]` is
        # emitted from `sys_poll` when a blocking poll hands back a fd without
        # POLLIN although bytes were published into that connection inside the
        # poll's own window and are still buffered. It does not depend on any
        # userspace program's opinion, so it is pinned separately from the oracle
        # verdict above. `[POLL_TCP_TIMEOUT]` comes out of the same function on
        # each ordinary boot (the oracle's stage 1 and stage 4 are both built to
        # time out) and is required so that "no lost-wake marker" is a reading
        # rather than an assumption about a reporting path that might be dead.
        if ! grep -qF "[POLL_TCP_TIMEOUT]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: kernel poll timeout report (#693) never emitted"
            return 1
        fi
        if grep -qF "[POLL_TCP_READY_LOST]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: kernel reported a lost TCP readiness publication (#693): $(grep -aF -m1 "[POLL_TCP_READY_LOST]" "$OUTPUT_DIR/serial.txt")"
            return 1
        fi
        if ! grep -qF -x "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: init-group refusal oracle counter marker missing"
            return 1
        fi
        # This gate kills QEMU shortly after exec smoke, so it pins the early probe
        # pair only; the full-system and service-sequence gates pin the quiesce pair.
        if ! grep -qF "[INIT_GROUP_REFUSAL:aarch64:phase=early:probe1=-22:probe2=-22:expected=-22]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: init-group early refusal marker missing"
            return 1
        fi
        if ! grep -qE '^\[INIT_GROUP_WALK:aarch64:rows=[0-9]+:init_tgid_rows=1:foreign_tgid_rows=0:refused=2:verdict=PASS\]$' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: init-group early walk marker missing"
            return 1
        fi
        if grep -qE '\[INIT_GROUP_WALK:.*verdict=FAIL' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: init-group walk reported failure"
            return 1
        fi
        if grep -qF "[INIT_GROUP_CHILD_RAN]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: refused init-group child ran"
            return 1
        fi
        echo "SUCCESS"
        return 0
    else
        local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
        if [ -n "$CRASH_TYPE" ]; then
            echo "FAIL: $CRASH_TYPE ($LINES lines)"
        else
            if ! $USERSPACE_COMPLETE; then
                echo "FAIL: userspace not observed ($LINES lines)"
            fi
            if ! $EXEC_SMOKE_COMPLETE; then
                echo "FAIL: exec smoke not observed ($LINES lines)"
            fi
            if ! $EXEC_FIRST_COMMIT; then
                echo "FAIL: exec first commit not observed ($LINES lines)"
            fi
        fi
        return 1
    fi
}

echo "========================================="
echo "ARM64 Boot Test (Native QEMU)"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

for attempt in $(seq 1 $MAX_RETRIES); do
    echo "Attempt $attempt/$MAX_RETRIES..."
    if run_single_test; then
        echo ""
        echo "========================================="
        echo "ARM64 BOOT TEST: PASSED"
        echo "========================================="
        exit 0
    fi
    if [ $attempt -lt $MAX_RETRIES ]; then
        echo "Retrying..."
        sleep 1
    fi
done

echo ""
echo "========================================="
echo "ARM64 BOOT TEST: FAILED (after $MAX_RETRIES attempts)"
echo "========================================="
echo ""
echo "NOTE: If this test frequently requires retries or fails repeatedly,"
echo "there may be a regression. Check recent changes to boot code."
echo ""
echo "Last output:"
tail -10 "$BREENIX_GATE_TMP/breenix_aarch64_boot_native/serial.txt" 2>/dev/null || echo "(no output)"
exit 1
