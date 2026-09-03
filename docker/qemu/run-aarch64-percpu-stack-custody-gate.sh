#!/bin/bash
# Automated home of the per-CPU exception-stack ownership probes (legs A-D).
#
# A CPU's per-CPU data must never name another CPU's exception stack as its
# kernel stack top. Leg A installs an offline CPU's stack top on a running CPU
# and watches for the save frame that follows; leg B is the control arm proving
# a CPU's OWN slot top and an ordinary heap-backed thread stack stay acceptable;
# leg C is the passive occupancy census; leg D asks whether thread id 0 is a
# live thread id. The gate boots a boot_tests kernel, so it also rejects
# boot-test failures.
#
# The gate additionally requires a [PERCPU_STACK_ALIEN: record — the refusal the
# ownership-checking setter emits when it declines a stack top belonging to
# another CPU — and, like every other aarch64 boot_tests gate, a non-failing
# [BLOCK_EINTR_ORACLE: line.
#
# It builds into its own target dir so the shared target/aarch64-breenix-kernel
# kernel other gates boot is never replaced by a probe-feature build.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE_TARGET_DIR="$BREENIX_ROOT/target/percpu-stack-custody-gate"
OUTPUT_DIR="/tmp/breenix_aarch64_percpu_stack_custody_gate"

# This proves the complete boot-tests sequence reported success.
BOOT_TESTS_PASS_LITERAL='[BOOT_TESTS:PASS]'
# This proves init's block EINTR oracle ran during the boot-tests sequence.
BLOCK_EINTR_ORACLE_LITERAL='[BLOCK_EINTR_ORACLE:'
# This proves init's block EINTR oracle did not self-report a failure.
BLOCK_EINTR_ORACLE_FAIL_LITERAL='[BLOCK_EINTR_ORACLE:FAIL'
# #568: this proves init's blocking-poll-on-connected-TCP oracle ran, and that
# it did not self-report a failure. Both halves are needed: presence alone
# passes a boot whose verdict was FAIL, and the FAIL grep alone passes a boot
# where the program never started.
POLL_TCP_ORACLE_LITERAL='[POLL_TCP_ORACLE:'
POLL_TCP_ORACLE_FAIL_LITERAL='[POLL_TCP_ORACLE:FAIL'
# #693: the kernel's own contradiction check. `[POLL_TCP_READY_LOST]` is
# emitted from `sys_poll` when a blocking poll hands back a fd without POLLIN
# although bytes were published into that connection inside the poll's own
# window and are still buffered. It does not depend on any userspace program's
# opinion, so it is pinned separately from the oracle verdict above.
# `[POLL_TCP_TIMEOUT]` comes out of the same function on each ordinary boot
# (the oracle's stage 1 and stage 4 are both built to time out) and is
# required so that "no lost-wake marker" is a reading rather than an
# assumption about a reporting path that might be dead.
POLL_TCP_TIMEOUT_LITERAL='[POLL_TCP_TIMEOUT]'
POLL_TCP_READY_LOST_LITERAL='[POLL_TCP_READY_LOST]'
# The refusing setter's record.
ALIEN_LITERAL='[PERCPU_STACK_ALIEN:'

LEG_A_MARKER='[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:'
LEG_B_MARKER='[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:'
LEG_C_MARKER='[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:'
LEG_D_MARKER='[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:'

cleanup() {
    if [ -n "${QEMU_PID:-}" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    echo "Serial: $OUTPUT_DIR/serial.txt"
}
trap cleanup EXIT

# Print every probe report line the boot did produce, so a failure is readable
# without opening the serial log.
dump_reports() {
    echo "Probe reports found:"
    if grep -aF '[PERCPU_STACK_CUSTODY_ORACLE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        :
    else
        echo "  (none)"
    fi
}

(cd "$BREENIX_ROOT" && CARGO_TARGET_DIR="$GATE_TARGET_DIR" cargo build --release \
    --features boot_tests,percpu_stack_custody_oracle \
    --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64)
KERNEL="$GATE_TARGET_DIR/aarch64-breenix-kernel/release/kernel-aarch64"

"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

timeout 200 qemu-system-aarch64 \
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

LEG_A_LINE=""
LEG_B_LINE=""
LEG_C_LINE=""
LEG_D_LINE=""
BOOT_TESTS_PASS_LINE=""
BLOCK_EINTR_ORACLE_LINE=""
POLL_TCP_ORACLE_LINE=""
ALIEN_LINE=""
FATAL_LINE=""

for _ in $(seq 1 180); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        LEG_A_LINE=$(grep -aF "$LEG_A_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        LEG_B_LINE=$(grep -aF "$LEG_B_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        LEG_C_LINE=$(grep -aF "$LEG_C_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        LEG_D_LINE=$(grep -aF "$LEG_D_MARKER" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        BOOT_TESTS_PASS_LINE=$(grep -aF "$BOOT_TESTS_PASS_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        BLOCK_EINTR_ORACLE_LINE=$(grep -aF "$BLOCK_EINTR_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        POLL_TCP_ORACLE_LINE=$(grep -aF "$POLL_TCP_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        ALIEN_LINE=$(grep -aF "$ALIEN_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        FATAL_LINE=$(grep -aiE 'KERNEL PANIC|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected' "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1 || true)
        if [ -n "$FATAL_LINE" ]; then
            echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
            echo "$FATAL_LINE"
            dump_reports
            exit 1
        fi
        if grep -qaF '[BOOT_TESTS:FAIL' "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOOT_TEST_FAIL_LINE=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -1 || true)
            echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
            echo "boot test failure: ${BOOT_TEST_FAIL_LINE:-[TEST:<missing>:FAIL:<missing>]}"
            dump_reports
            exit 1
        fi
        if [ -n "$LEG_A_LINE" ] && [ -n "$LEG_B_LINE" ] && [ -n "$LEG_C_LINE" ] && \
            [ -n "$LEG_D_LINE" ] && [ -n "$BOOT_TESTS_PASS_LINE" ] && [ -n "$ALIEN_LINE" ] && \
            [ -n "$BLOCK_EINTR_ORACLE_LINE" ] && [ -n "$POLL_TCP_ORACLE_LINE" ]; then
            break
        fi
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done

if ! grep -qaF "$BOOT_TESTS_PASS_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Missing marker: $BOOT_TESTS_PASS_LITERAL"
    dump_reports
    exit 1
fi

if ! grep -qaF "$BLOCK_EINTR_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Missing marker: $BLOCK_EINTR_ORACLE_LITERAL"
    dump_reports
    exit 1
fi
if grep -qaF "$BLOCK_EINTR_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Block EINTR oracle reported failure: $BLOCK_EINTR_ORACLE_FAIL_LITERAL"
    dump_reports
    exit 1
fi

if ! grep -qaF "$POLL_TCP_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Missing marker: $POLL_TCP_ORACLE_LITERAL"
    dump_reports
    exit 1
fi
if grep -qaF "$POLL_TCP_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Poll TCP oracle reported failure: $(grep -aF "$POLL_TCP_ORACLE_FAIL_LITERAL" "$OUTPUT_DIR/serial.txt" | tail -1)"
    dump_reports
    exit 1
fi

if ! grep -qaF "$POLL_TCP_TIMEOUT_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Missing marker: $POLL_TCP_TIMEOUT_LITERAL (kernel poll timeout report (#693) never emitted)"
    dump_reports
    exit 1
fi

if grep -qaF "$POLL_TCP_READY_LOST_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Kernel reported a lost TCP readiness publication (#693): $(grep -aF "$POLL_TCP_READY_LOST_LITERAL" "$OUTPUT_DIR/serial.txt" | tail -1)"
    dump_reports
    exit 1
fi

for LEG in A B C D; do
    case "$LEG" in
        A) MARKER="$LEG_A_MARKER"; LINE="$LEG_A_LINE" ;;
        B) MARKER="$LEG_B_MARKER"; LINE="$LEG_B_LINE" ;;
        C) MARKER="$LEG_C_MARKER"; LINE="$LEG_C_LINE" ;;
        D) MARKER="$LEG_D_MARKER"; LINE="$LEG_D_LINE" ;;
    esac
    if [ -z "$LINE" ]; then
        echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
        echo "Missing marker: $MARKER"
        dump_reports
        exit 1
    fi
    if ! echo "$LINE" | grep -q ':PASS]$'; then
        echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
        echo "$LINE"
        dump_reports
        exit 1
    fi
done

if [ -z "$ALIEN_LINE" ]; then
    echo "ARM64 PERCPU STACK CUSTODY GATE: FAILED"
    echo "Missing marker: $ALIEN_LITERAL"
    echo "No setter refused a stack top belonging to another CPU."
    dump_reports
    exit 1
fi

echo "ARM64 PERCPU STACK CUSTODY GATE: PASSED"
echo "$LEG_A_LINE"
echo "$LEG_B_LINE"
echo "$LEG_C_LINE"
echo "$LEG_D_LINE"
echo "$ALIEN_LINE"
echo "$BOOT_TESTS_PASS_LINE"
echo "$BLOCK_EINTR_ORACLE_LINE"
exit 0
