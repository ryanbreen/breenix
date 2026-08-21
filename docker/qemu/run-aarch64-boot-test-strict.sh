#!/bin/bash
# Strict ARM64 boot test - runs multiple iterations and requires ALL to pass
# Used for CI to catch regressions. Does NOT retry failed boots.
#
# Unlike run-aarch64-boot-test-native.sh which uses retries (masking failures),
# this test counts every boot attempt. A single failure means the test fails.
# A boot is accepted only after both userspace liveness and exec smoke completion.
# Serial output from every failed iteration is preserved in a never-cleared directory.
#
# Usage: ./run-aarch64-boot-test-strict.sh [iterations]
#        Default: 20 iterations
#
# Exit codes:
#   0 - All iterations passed
#   1 - One or more iterations failed

set -e

ITERATIONS=${1:-20}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# construct_residual is the counted frame residue of the two construction-failure arms read off a measured green run, and it is architecture-specific (4 on x86, 2 on aarch64) because the two page-table constructors record different table-frame counts.
INIT_DESIGNATION_ORACLE_LITERAL='[INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=2:construct_residual=2:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]'
INIT_GROUP_REFUSAL_ORACLE_LITERAL='[INIT_GROUP_REFUSAL_ORACLE:aarch64:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]'
# driven=2 proves both handoff seams ran; stage1/2 return, wake, and park fields
# expose D1/D2. stage3_elapsed_ok=1 proves no early timeout return, while
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms is the measured duration; residual/balance prove cleanup.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
# resolved_production may be zero once #605's early-slot-consumption defect is fixed; deterministic resolved_exercised proves the resolver ran.
SCHED_STRAND_ORACLE_PATTERN='\[SCHED_STRAND_ORACLE:aarch64:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[1-9][0-9]*:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+\]'
STRAND_INJECT_ORACLE_PATTERN='\[STRAND_INJECT_ORACLE:aarch64:legA_exercised=1:legA_recovered=1:legB_exercised=1:legB_recovered=1:stranded=0\]'
CENSUS_WIDEN_ORACLE_PATTERN='\[CENSUS_WIDEN_ORACLE:aarch64:baseline_reported=0:armed_reported=1:tid=[1-9][0-9]*:shape=running:nonprogress_axis=1:PASS\]'

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    # This gate pins boot_tests-only markers (the oracle counters and both strand
    # oracles); a kernel built without the feature fails it spuriously.
    echo "  cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Durable #528 guard: the kernel MUST be soft-float. Fail fast if it was built
# with the NEON hardfloat target (aarch64-breenix.json) — that re-arms #528.
# (set -e aborts the gate if the guard trips.)
#
# This gate is the kernel-merge gate and it was the ONLY aarch64 gate without
# this guard: the full-system, production-profile and service-sequence gates all
# carried it, so a NEON-target kernel could be merged through the one gate that
# decides merges while the others would have caught it. Found while pinning the
# feature-profile guard below, which asserts the two preflights run in order.
"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

# Durable feature-profile guard. This gate pins markers that ONLY a
# `--features boot_tests` kernel emits, so a kernel built in any other profile
# fails every boot on "marker missing" and the run reads as a kernel regression.
#
# `cargo` keeps one cached artifact per feature set and hardlinks the requested
# one into this single output path in about 0.06 s, with no recompilation and no
# output worth reading. ANY `cargo test` in the same session therefore replaces
# this binary silently — `cargo test --test kernel_no_neon_guard` builds the
# kernel with NO features by design — and the next gate boots the wrong kernel.
# Measured: an acceptance battery that ran the structural suites and then this
# gate scored 0/6, every boot on "Futex handoff oracle marker missing or failed",
# against a production kernel that was never asked to emit it. Refuse instead.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of marker literals rather than one sentinel: a single marker
    # changing profile must not be able to disarm this guard quietly.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[CENSUS_WIDEN_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[BOOT_TESTS:'; do
        if ! grep -aqF "$marker" "$kernel" 2>/dev/null; then
            missing="$missing $marker"
        fi
    done

    if [ -n "$missing" ]; then
        echo "Error: $kernel was not built with --features boot_tests."
        echo "  Missing boot_tests-only marker literal(s):$missing"
        echo "  This gate pins those markers, so every boot would fail on 'marker missing'."
        echo "  Rebuild with:"
        echo "    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
        echo "  NOTE: any 'cargo test' in this session rebuilds the kernel WITHOUT boot_tests and"
        echo "  silently swaps this binary in a fraction of a second. Build after testing, not before."
        exit 1
    fi
}

require_boot_tests_kernel "$KERNEL"

# Find ext2 disk (required for userspace)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

# Track results
SUCCESSES=0
FAILURES=0
FAILED_ITERATIONS=""

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

# Score a finished boot ENTIRELY from the serial file it produced.
#
# The poll loop in run_single_test only decides WHEN TO STOP WAITING; it must
# never decide the verdict. Its booleans latch on a grep that runs at most once
# every 1.5s, so a marker that lands between the last grep and the kill is
# present in the file while the boolean is still false — and the old code scored
# that latched false as a boot failure. Everything the gate rejects is rejected
# here, from the file, after QEMU is gone; nothing is loosened.
#
# Prints the failure reason and returns 1 when the boot is unacceptable; prints
# nothing and returns 0 when it is acceptable.
score_serial() {
    local serial_file="$1"
    local crash_type

    if [ ! -f "$serial_file" ]; then
        echo "Userspace not detected"
        return 1
    fi
    if crash_type=$(check_crash_markers "$serial_file"); then
        echo "$crash_type"
        return 1
    fi
    if ! grep -qE "(breenix>|bsh |\[bwm\] Display:|\[bcheck\] Complete:|\[heartbeat\])" \
        "$serial_file" 2>/dev/null; then
        echo "Userspace not detected"
        return 1
    fi
    if ! grep -qF "[EXEC_SMOKE:TARGET_OK]" "$serial_file" 2>/dev/null; then
        echo "Exec smoke did not complete"
        return 1
    fi
    if ! grep -qF "[EXEC_LOCK_ORDER:FIRST_COMMIT]" "$serial_file" 2>/dev/null; then
        echo "Exec commit marker missing"
        return 1
    fi
    if ! grep -qF "[BLOCK_EINTR_ORACLE:" "$serial_file" 2>/dev/null; then
        echo "Block EINTR oracle marker missing"
        return 1
    fi
    if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        echo "Block EINTR oracle reported failure"
        return 1
    fi
    if ! grep -qE "$FUTEX_HANDOFF_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Futex handoff oracle marker missing or failed"
        return 1
    fi
    # FORBIDDEN PATTERNS, scanned over the WHOLE serial, before the presence
    # checks below. The census is cumulative and emitted on a fixed cadence from
    # t≈3s, so every boot that survives three seconds always contains a clean
    # `stranded=0` line; a presence check alone therefore cannot fail on a strand
    # that first appears at t=10s. These two greps are what make ruling (b)'s
    # "hard-fails on stranded>0" true on the kernel-merge gate. They add failure
    # conditions and remove none.
    if grep -qE '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$serial_file" 2>/dev/null; then
        echo "Scheduler strand census reported stranded work ($(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' "$serial_file" | tail -1))"
        return 1
    fi
    if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" 2>/dev/null; then
        echo "Scheduler strand injection oracle reported stranded work ($(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" | tail -1))"
        return 1
    fi
    if ! grep -qE "$SCHED_STRAND_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Scheduler strand census oracle marker missing or failed"
        return 1
    fi
    if ! grep -qE "$STRAND_INJECT_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Scheduler strand injection oracle marker missing or failed"
        return 1
    fi
    if ! grep -qE "$CENSUS_WIDEN_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
        echo "Census widening mutation oracle marker missing or failed"
        return 1
    fi
    if ! grep -qF "[INIT_DESIGNATION:aarch64:designated_pid=1:reserved_collisions=0]" "$serial_file" 2>/dev/null; then
        echo "Init designation marker missing"
        return 1
    fi
    if ! grep -qF -x "$INIT_DESIGNATION_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Init designation oracle counter marker missing"
        return 1
    fi
    if ! grep -qF -x "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" "$serial_file" 2>/dev/null; then
        echo "Init-group refusal oracle counter marker missing"
        return 1
    fi
    # This gate kills QEMU shortly after exec smoke, so it pins the early probe
    # pair only; the full-system and service-sequence gates pin the quiesce pair.
    if ! grep -qF "[INIT_GROUP_REFUSAL:aarch64:phase=early:probe1=-22:probe2=-22:expected=-22]" "$serial_file" 2>/dev/null; then
        echo "Init-group early refusal marker missing"
        return 1
    fi
    if ! grep -qE '^\[INIT_GROUP_WALK:aarch64:rows=[0-9]+:init_tgid_rows=1:foreign_tgid_rows=0:refused=2:verdict=PASS\]$' "$serial_file" 2>/dev/null; then
        echo "Init-group early walk marker missing"
        return 1
    fi
    if grep -qE '\[INIT_GROUP_WALK:.*verdict=FAIL' "$serial_file" 2>/dev/null; then
        echo "Init-group walk reported failure"
        return 1
    fi
    if grep -qF "[INIT_GROUP_CHILD_RAN]" "$serial_file" 2>/dev/null; then
        echo "Refused init-group child ran"
        return 1
    fi
    return 0
}

# Scoring-only entry point: score an already-captured serial log and exit. This
# exists so the scoring rules can be exercised against a preserved serial without
# booting, which is how the "a serial containing every success marker scores as a
# success" property is proven.
if [ -n "${BREENIX_STRICT_SCORE_ONLY:-}" ]; then
    if SCORE_REASON=$(score_serial "$BREENIX_STRICT_SCORE_ONLY"); then
        echo "SCORE: PASS - $BREENIX_STRICT_SCORE_ONLY"
        exit 0
    else
        echo "SCORE: FAIL - $SCORE_REASON ($BREENIX_STRICT_SCORE_ONLY)"
        exit 1
    fi
fi

report_failure() {
    local iteration="$1"
    local reason="$2"
    local serial_file="$3"
    local failure_dir="/tmp/breenix_aarch64_strict_failures"
    local timestamp
    local preserved_serial
    local lines

    mkdir -p "$failure_dir"
    timestamp=$(date -u +%Y%m%dT%H%M%SZ)
    preserved_serial="$failure_dir/${timestamp}-boot${iteration}.txt"
    if [ -f "$serial_file" ]; then
        cp "$serial_file" "$preserved_serial"
        lines=$(wc -l < "$serial_file" 2>/dev/null | tr -d ' ' || echo 0)
    else
        # QEMU never opened the serial file: preserve the empty artifact anyway so
        # "zero serial bytes" (the #569 silent-hang signature) is on the record.
        : > "$preserved_serial"
        lines=0
    fi
    echo "  [FAIL] Boot $iteration: $reason ($lines lines); serial: $preserved_serial"
}

run_single_test() {
    local iteration=$1
    local OUTPUT_DIR="/tmp/breenix_aarch64_strict_$iteration"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 20s timeout.
    # Breenix ARM64 expects a GICv3 CPU interface, matching Parallels.
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
    timeout 20 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4 \
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
    local QEMU_PID=$!

    # Wait for userspace liveness AND exec smoke completion (18s max, checking every 1.5s)
    # Accept any of these as the liveness condition:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    #   "[bcheck] Complete:" - bcheck self-test suite finished (headless/no-VirGL mode)
    #   "[heartbeat]" - the default ARM64 init service executed in userspace
    # Also require "[EXEC_SMOKE:TARGET_OK]" as the exec completion condition.
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local CRASH_TYPE=""
    # Named POLL, not i: the caller's loop variable is also i, and an unscoped
    # inner i made the summary report the poll counter instead of the boot number.
    local POLL
    #
    # THE STOP CONDITION IS score_serial ITSELF, not a narrower pair of liveness
    # patterns.
    #
    # This loop used to break as soon as a userspace liveness pattern and
    # [EXEC_SMOKE:TARGET_OK] were both present, kill QEMU, and only then score the
    # serial. Those two land at roughly 0.5 s and 4.4 s of uptime; every other
    # marker score_serial requires — the futex handoff oracle (~5.8 s), the block
    # EINTR oracle (~5.8 s), the strand census and the strand injection oracle —
    # is emitted afterwards. The gate therefore killed the VM before the evidence
    # it scores could exist and failed every boot on "marker missing", including
    # on main: a stop condition narrower than the scoring criteria is a gate that
    # cannot pass. It also made the forbidden-pattern scans below unreachable — a
    # late strand cannot appear in a serial that was truncated at 4.4 s.
    #
    # Polling score_serial keeps the two in sync by construction: whatever the
    # scoring criteria grow to require, the loop waits for it. This only ever
    # extends the capture window. It accepts nothing score_serial would reject —
    # the verdict below is still a fresh score of the serial QEMU left behind —
    # and the crash-marker break and the wall-clock bound are unchanged.
    for POLL in $(seq 1 12); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
            if score_serial "$OUTPUT_DIR/serial.txt" >/dev/null 2>&1; then
                break
            fi
        fi
        sleep 1.5
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    # The poll booleans above are a stop condition, not a verdict. Score the boot
    # from the serial file QEMU actually left behind.
    local FAIL_DETAIL
    if FAIL_DETAIL=$(score_serial "$OUTPUT_DIR/serial.txt"); then
        echo "  [OK] Boot $iteration: SUCCESS"
        return 0
    fi

    report_failure "$iteration" "$FAIL_DETAIL" "$OUTPUT_DIR/serial.txt"
    return 1
}

echo "========================================="
echo "ARM64 Strict Boot Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Iterations: $ITERATIONS"
echo "Requirement: 100% success rate (all $ITERATIONS must pass)"
echo ""
echo "Running tests..."
echo ""

START_TIME=$(date +%s)

for i in $(seq 1 $ITERATIONS); do
    if run_single_test $i; then
        SUCCESSES=$((SUCCESSES + 1))
    else
        FAILURES=$((FAILURES + 1))
        FAILED_ITERATIONS="$FAILED_ITERATIONS $i"
    fi
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "========================================="
echo "RESULTS"
echo "========================================="
echo "Total iterations: $ITERATIONS"
echo "Successes: $SUCCESSES"
echo "Failures: $FAILURES"
echo "Success rate: $(( (SUCCESSES * 100) / ITERATIONS ))%"
echo "Duration: ${DURATION}s"

if [ $FAILURES -eq 0 ]; then
    echo ""
    echo "========================================="
    echo "PASS: $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    exit 0
else
    echo ""
    echo "Failed iterations:$FAILED_ITERATIONS"
    echo ""
    echo "========================================="
    echo "FAIL: Only $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    echo ""
    echo "This indicates a regression or timing bug that needs investigation."
    echo "Serial output from failed boots can be found in /tmp/breenix_aarch64_strict_N/"
    exit 1
fi
