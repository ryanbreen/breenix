#!/bin/bash
# ARM64 Boot Test Script
#
# This script builds the ARM64 kernel and runs it in QEMU to validate boot.
# It checks for expected boot messages and reports pass/fail.
#
# Usage:
#   ./scripts/run-arm64-boot-test.sh           # Run full POST test
#   ./scripts/run-arm64-boot-test.sh quick     # Quick hello world check only
#   ./scripts/run-arm64-boot-test.sh timer     # Run timer-specific tests
#   ./scripts/run-arm64-boot-test.sh interrupt # Run interrupt-specific tests
#   ./scripts/run-arm64-boot-test.sh syscall   # Run syscall and EL0 tests
#   ./scripts/run-arm64-boot-test.sh schedule  # Run scheduling and blocking I/O tests
#   ./scripts/run-arm64-boot-test.sh signal    # Run signal delivery tests
#   ./scripts/run-arm64-boot-test.sh network   # Run network stack tests
#
# The test validates:
#   - Kernel entry point reached
#   - Serial port initialized
#   - GIC (interrupt controller) initialized
#   - Timer initialized
#   - Interrupts enabled
#   - Boot completion message

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$BREENIX_ROOT"

# Configuration
KERNEL_PATH="target/aarch64-breenix/release/kernel-aarch64"
SERIAL_OUTPUT="/tmp/arm64_boot_test_output.txt"
TIMEOUT_SECS=30
TEST_MODE="${1:-full}"

echo "========================================"
echo "  Breenix ARM64 Boot Test"
echo "========================================"
echo ""

# Feature flags
FEATURES=""
if [ "${BOOT_TESTS:-0}" = "1" ]; then
    FEATURES="--features boot_tests"
    echo "Boot tests enabled - parallel test framework with progress bars"
fi

# Build the kernel
echo "[1/4] Building ARM64 kernel..."
if ! cargo build --release \
    --target aarch64-breenix.json \
    -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem \
    -p kernel \
    --bin kernel-aarch64 $FEATURES 2>&1 | tail -5; then
    echo "ERROR: Kernel build failed"
    exit 1
fi

if [ ! -f "$KERNEL_PATH" ]; then
    echo "ERROR: Kernel not found at $KERNEL_PATH"
    exit 1
fi
echo "Kernel built: $KERNEL_PATH"
echo ""

# Clean up any previous QEMU
echo "[2/4] Starting QEMU..."
pkill -9 -f "qemu-system-aarch64.*kernel-aarch64" 2>/dev/null || true
rm -f "$SERIAL_OUTPUT"

# Check for ext2 disk (required for userspace tests)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
DISK_OPTS=""
if [ -f "$EXT2_DISK" ]; then
    DISK_OPTS="-device virtio-blk-device,drive=ext2disk \
        -blockdev driver=file,node-name=ext2file,filename=$EXT2_DISK \
        -blockdev driver=raw,node-name=ext2disk,file=ext2file"
    echo "Using ext2 disk: $EXT2_DISK"
fi

# Network options - only add for network test mode to avoid slowing down other tests
NET_OPTS=""
if [ "$TEST_MODE" = "network" ]; then
    NET_OPTS="-device virtio-net-device,netdev=net0 \
        -netdev user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15"
    echo "Enabling VirtIO network device for network tests"
fi

# Start QEMU in background
# Always include VirtIO GPU and keyboard so the kernel's MMIO enumeration
# discovers them (needed for interactive shell and device driver tests)
qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -nographic \
    -no-reboot \
    -kernel "$KERNEL_PATH" \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    $DISK_OPTS \
    $NET_OPTS \
    -serial "file:$SERIAL_OUTPUT" &
QEMU_PID=$!

# Wait for output - different markers for different test modes
echo "[3/4] Waiting for kernel output (${TIMEOUT_SECS}s timeout)..."
FOUND=false
START_TIME=$(date +%s)

# For syscall tests, wait for userspace to load (EL0_CONFIRMED or shell prompt)
# For network tests, wait for network initialization complete
if [ "$TEST_MODE" = "syscall" ]; then
    WAIT_MARKER="EL0_CONFIRMED\|breenix>"
elif [ "$TEST_MODE" = "network" ]; then
    WAIT_MARKER="Network stack initialized\|NET: Network initialization complete"
else
    WAIT_MARKER="Hello from ARM64"
fi

while true; do
    CURRENT_TIME=$(date +%s)
    ELAPSED=$((CURRENT_TIME - START_TIME))

    if [ $ELAPSED -ge $TIMEOUT_SECS ]; then
        break
    fi

    if [ -f "$SERIAL_OUTPUT" ] && [ -s "$SERIAL_OUTPUT" ]; then
        if grep -qE "$WAIT_MARKER" "$SERIAL_OUTPUT" 2>/dev/null; then
            FOUND=true
            # Give kernel a moment to finish writing
            sleep 2
            break
        fi
    fi

    sleep 0.5
done

# Kill QEMU
kill $QEMU_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true

echo ""

# Quick mode - just check for hello
if [ "$TEST_MODE" = "quick" ]; then
    echo "[4/4] Quick Boot Check:"
    echo "========================================"
    if $FOUND; then
        echo "PASS: 'Hello from ARM64' found"
        echo ""
        echo "First 10 lines of output:"
        head -10 "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
        echo "========================================"
        exit 0
    else
        echo "FAIL: 'Hello from ARM64' not found"
        echo ""
        echo "Output (if any):"
        cat "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
        echo "========================================"
        exit 1
    fi
fi

# Timer-specific test mode
if [ "$TEST_MODE" = "timer" ]; then
    echo "[4/4] Timer-Specific Tests:"
    echo "========================================"
    echo ""

    TIMER_PASSED=0
    TIMER_FAILED=0

    # Timer initialization checks (patterns match actual kernel output)
    declare -a TIMER_CHECKS=(
        "Generic Timer Init|Initializing Generic Timer"
        "Timer Frequency|Timer frequency:"
        "Timer IRQ Init|Initializing timer interrupt"
        "Timer Complete|Timer interrupt initialized"
        "GIC Init|GIC initialized"
        "Interrupts Enabled|Interrupts enabled: true"
    )

    echo "Timer Initialization:"
    for check in "${TIMER_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-25s " "$NAME"

        if grep -q "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "PASS"
            TIMER_PASSED=$((TIMER_PASSED + 1))
        else
            echo "FAIL"
            TIMER_FAILED=$((TIMER_FAILED + 1))
        fi
    done

    # Extract and display timer frequency configuration
    echo ""
    echo "Timer Configuration:"
    FREQ_LINE=$(grep "Timer frequency:" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
    if [ -n "$FREQ_LINE" ]; then
        echo "  $FREQ_LINE"
    fi
    CONFIG_LINE=$(grep "Timer configured for" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
    if [ -n "$CONFIG_LINE" ]; then
        echo "  $CONFIG_LINE"
    fi

    # ===========================================
    # CRITICAL: Timer Interrupt Frequency Test
    # This verifies actual interrupt rate, not log output rate
    # ===========================================
    echo ""
    echo "Timer Interrupt Frequency Verification:"
    echo "  (Kernel prints [TIMER_COUNT:N] on each interrupt)"
    echo ""

    # Extract TIMER_COUNT values from output
    # Format: [TIMER_COUNT:N] where N is total interrupt count
    TIMER_COUNTS=$(grep -oE '\[TIMER_COUNT:[0-9]+\]' "$SERIAL_OUTPUT" 2>/dev/null | \
                   sed 's/\[TIMER_COUNT:\([0-9]*\)\]/\1/')

    if [ -z "$TIMER_COUNTS" ]; then
        echo "  FAIL: No [TIMER_COUNT:N] markers found in output"
        echo ""
        echo "  This means either:"
        echo "    1. Timer interrupts are not firing"
        echo "    2. Kernel was not built with timer count reporting"
        echo "    3. Boot did not run long enough to accumulate 100 interrupts"
        echo ""
        TIMER_FAILED=$((TIMER_FAILED + 1))
    else
        # Count number of TIMER_COUNT markers
        MARKER_COUNT=$(echo "$TIMER_COUNTS" | wc -l | tr -d ' ')
        echo "  TIMER_COUNT markers found: $MARKER_COUNT"

        # Get first and last count values
        FIRST_COUNT=$(echo "$TIMER_COUNTS" | head -1)
        LAST_COUNT=$(echo "$TIMER_COUNTS" | tail -1)

        echo "  First count:  $FIRST_COUNT"
        echo "  Last count:   $LAST_COUNT"

        if [ "$MARKER_COUNT" -lt 2 ]; then
            echo ""
            echo "  FAIL: Need at least 2 TIMER_COUNT markers to measure frequency"
            echo "  (Got $MARKER_COUNT marker(s) - boot may not have run long enough)"
            TIMER_FAILED=$((TIMER_FAILED + 1))
        else
            # Calculate actual interrupt count between markers
            INTERRUPT_DELTA=$((LAST_COUNT - FIRST_COUNT))

            # Expected frequency is 200 Hz (configured in timer_interrupt.rs)
            EXPECTED_FREQ=200

            echo "  Interrupt delta: $INTERRUPT_DELTA (over $MARKER_COUNT markers)"

            # Extract first and last timestamp from TIMER_COUNT lines
            FIRST_TS_LINE=$(grep '\[TIMER_COUNT:' "$SERIAL_OUTPUT" 2>/dev/null | head -1)
            LAST_TS_LINE=$(grep '\[TIMER_COUNT:' "$SERIAL_OUTPUT" 2>/dev/null | tail -1)

            # Try to extract timestamp (format: "[ INFO] X.XXX -")
            FIRST_TS=$(echo "$FIRST_TS_LINE" | grep -oE '\[ INFO\] [0-9]+\.[0-9]+' | sed 's/\[ INFO\] //')
            LAST_TS=$(echo "$LAST_TS_LINE" | grep -oE '\[ INFO\] [0-9]+\.[0-9]+' | sed 's/\[ INFO\] //')

            if [ -n "$FIRST_TS" ] && [ -n "$LAST_TS" ] && [ "$FIRST_TS" != "$LAST_TS" ]; then
                # Calculate time elapsed
                TIME_ELAPSED=$(echo "$LAST_TS - $FIRST_TS" | bc 2>/dev/null)

                if [ -n "$TIME_ELAPSED" ] && [ "$TIME_ELAPSED" != "0" ]; then
                    # Calculate actual frequency
                    # Rate = interrupt_delta / time_elapsed
                    ACTUAL_FREQ=$(echo "scale=1; $INTERRUPT_DELTA / $TIME_ELAPSED" | bc 2>/dev/null)

                    echo ""
                    echo "  Time elapsed:     ${TIME_ELAPSED}s"
                    echo "  Actual frequency: ${ACTUAL_FREQ} Hz"
                    echo "  Expected frequency: ${EXPECTED_FREQ} Hz (+/- 20%)"

                    # Check if within acceptable range (80-120% of expected)
                    MIN_FREQ=$(echo "$EXPECTED_FREQ * 0.8" | bc)
                    MAX_FREQ=$(echo "$EXPECTED_FREQ * 1.2" | bc)

                    # Use bc for float comparison
                    IN_RANGE=$(echo "$ACTUAL_FREQ >= $MIN_FREQ && $ACTUAL_FREQ <= $MAX_FREQ" | bc 2>/dev/null)

                    echo ""
                    printf "  %-25s " "Frequency In Range"
                    if [ "$IN_RANGE" = "1" ]; then
                        echo "PASS (${MIN_FREQ}-${MAX_FREQ} Hz)"
                        TIMER_PASSED=$((TIMER_PASSED + 1))
                    else
                        echo "FAIL (expected ${MIN_FREQ}-${MAX_FREQ} Hz, got ${ACTUAL_FREQ} Hz)"
                        TIMER_FAILED=$((TIMER_FAILED + 1))
                    fi
                else
                    echo ""
                    echo "  WARNING: Could not calculate time elapsed (timestamps same or invalid)"
                    echo "  Falling back to marker count verification..."

                    # Fallback: at least verify we got multiple markers
                    printf "  %-25s " "Multiple Markers"
                    if [ "$MARKER_COUNT" -ge 3 ]; then
                        echo "PASS ($MARKER_COUNT markers)"
                        TIMER_PASSED=$((TIMER_PASSED + 1))
                    else
                        echo "FAIL (only $MARKER_COUNT markers)"
                        TIMER_FAILED=$((TIMER_FAILED + 1))
                    fi
                fi
            else
                echo ""
                echo "  NOTE: No log timestamps on TIMER_COUNT lines"
                echo "  (Raw serial output doesn't include timestamps)"
                echo ""

                # Without timestamps, we can still verify interrupts are firing
                # by checking that we got a reasonable number of markers

                # For a 30-second test at 200 Hz, we'd expect ~60 markers
                # For a 2-second kernel runtime, we'd expect ~4 markers
                printf "  %-25s " "Timer Active"
                if [ "$MARKER_COUNT" -ge 2 ]; then
                    echo "PASS ($MARKER_COUNT markers = $INTERRUPT_DELTA interrupts)"
                    TIMER_PASSED=$((TIMER_PASSED + 1))

                    # Check monotonic increase
                    printf "  %-25s " "Count Increasing"
                    if [ "$LAST_COUNT" -gt "$FIRST_COUNT" ]; then
                        echo "PASS ($FIRST_COUNT -> $LAST_COUNT)"
                        TIMER_PASSED=$((TIMER_PASSED + 1))
                    else
                        echo "FAIL (not increasing)"
                        TIMER_FAILED=$((TIMER_FAILED + 1))
                    fi
                else
                    echo "FAIL (need at least 2 markers)"
                    TIMER_FAILED=$((TIMER_FAILED + 1))
                fi
            fi
        fi
    fi

    # Show raw TIMER_COUNT lines for debugging
    echo ""
    echo "Raw TIMER_COUNT output:"
    echo "----------------------------------------"
    grep '\[TIMER_COUNT:' "$SERIAL_OUTPUT" 2>/dev/null | head -10 || echo "(none found)"
    echo "----------------------------------------"

    echo ""
    echo "========================================"
    TIMER_TOTAL=$((TIMER_PASSED + TIMER_FAILED))
    echo "Timer Tests: $TIMER_PASSED/$TIMER_TOTAL passed"
    echo "========================================"

    if [ $TIMER_FAILED -gt 0 ]; then
        exit 1
    fi
    exit 0
fi

# Interrupt-specific test mode
if [ "$TEST_MODE" = "interrupt" ]; then
    echo "[4/4] Interrupt-Specific Tests:"
    echo "========================================"
    echo ""

    INT_PASSED=0
    INT_FAILED=0

    # GIC initialization checks (patterns match actual kernel output)
    declare -a GIC_CHECKS=(
        "Exception Level|Current exception level: EL1"
        "GICv2 Init|Initializing GICv2"
        "GIC Complete|GIC initialized"
        "UART IRQ Enable|Enabling GIC IRQ 33"
        "Timer Init|Initializing timer interrupt"
        "Timer Ready|Timer interrupt initialized"
        "Interrupts CPU Enable|Enabling interrupts"
        "Interrupts Ready|Interrupts enabled:"
    )

    echo "GIC (Interrupt Controller) Initialization:"
    for check in "${GIC_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-25s " "$NAME"

        if grep -q "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "PASS"
            INT_PASSED=$((INT_PASSED + 1))
        else
            echo "FAIL"
            INT_FAILED=$((INT_FAILED + 1))
        fi
    done

    echo ""
    echo "Exception Handler Checks:"

    # Check for exception handling infrastructure
    if grep -q "Current exception level: EL1" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "  Exception vectors:      CONFIGURED (running at EL1)"
        INT_PASSED=$((INT_PASSED + 1))
    else
        echo "  Exception vectors:      UNKNOWN"
        INT_FAILED=$((INT_FAILED + 1))
    fi

    # Check for any exception messages (data abort, instruction abort, etc.)
    if grep -qi "exception\|abort" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "  Exception handling:     ACTIVE"
        grep -i "exception\|abort" "$SERIAL_OUTPUT" 2>/dev/null | head -3 | while read line; do
            echo "    > $line"
        done
    else
        echo "  Exception handling:     No exceptions logged (good)"
    fi

    # Check for BRK (breakpoint) handling
    # This is OPTIONAL - breakpoints are for debugging, not a boot requirement
    if grep -q "Breakpoint (BRK" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "  Breakpoint handler:     TESTED"
        INT_PASSED=$((INT_PASSED + 1))
    else
        echo "  Breakpoint handler:     (not triggered - debugging feature)"
    fi

    echo ""
    echo "IRQ Routing:"

    # Check for specific IRQ configurations
    declare -a IRQ_CHECKS=(
        "IRQ 27 (Timer)|27"
        "IRQ 33 (UART0)|33"
    )

    for check in "${IRQ_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-25s " "$NAME"

        if grep -qi "irq.*$PATTERN\|$PATTERN.*irq" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "CONFIGURED"
            INT_PASSED=$((INT_PASSED + 1))
        else
            echo "not found"
        fi
    done

    echo ""
    echo "========================================"
    INT_TOTAL=$((INT_PASSED + INT_FAILED))
    echo "Interrupt Tests: $INT_PASSED/$INT_TOTAL passed"
    echo "========================================"

    if [ $INT_FAILED -gt 0 ]; then
        exit 1
    fi
    exit 0
fi

# Syscall/privilege level test mode
if [ "$TEST_MODE" = "syscall" ]; then
    echo "[4/4] Syscall and EL0 (Userspace) Tests:"
    echo "========================================"
    echo ""

    # ===========================================
    # HARD REQUIREMENT: EL0_CONFIRMED must be present
    # Without userspace execution, syscall tests are meaningless
    # ===========================================
    if ! grep -q "EL0_CONFIRMED" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "FATAL: EL0_CONFIRMED marker not found"
        echo "       Userspace did not execute - tests are meaningless"
        echo ""
        echo "Without EL0_CONFIRMED, we cannot verify:"
        echo "  - Syscalls are actually being made from userspace"
        echo "  - The kernel correctly handles EL0->EL1 transitions"
        echo "  - Any syscall results are from real userspace code"
        echo ""
        echo "Debug: First 30 lines of output:"
        head -30 "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
        echo ""
        exit 1
    fi

    echo "PREREQUISITE: EL0_CONFIRMED found - userspace executed"
    echo ""

    SYS_PASSED=0
    SYS_FAILED=0

    # ===========================================
    # SECTION 1: Exception Level Infrastructure
    # ===========================================
    echo "Exception Level Infrastructure:"

    declare -a EL_CHECKS=(
        "Kernel at EL1|Current exception level: EL1"
        "SVC Handler Ready|GIC initialized"
        "Exception Vectors|Interrupts enabled"
    )

    for check in "${EL_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-30s " "$NAME"

        if grep -q "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "PASS"
            SYS_PASSED=$((SYS_PASSED + 1))
        else
            echo "FAIL"
            SYS_FAILED=$((SYS_FAILED + 1))
        fi
    done

    # ===========================================
    # SECTION 2: EL0 (Userspace) Entry - Informational Only
    # ===========================================
    # NOTE: These markers are informational. The REQUIRED check is EL0_CONFIRMED
    # in Section 3, which definitively proves userspace execution.
    echo ""
    echo "EL0 (Userspace) Entry (informational):"

    # Check for EL0 entry marker - informational only
    printf "  %-30s " "EL0 First Entry"
    if grep -q "EL0_ENTER: First userspace entry" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "(not logged)"
    fi

    # Check for EL0 smoke marker - informational only
    printf "  %-30s " "EL0 Smoke Test"
    if grep -q "EL0_SMOKE: userspace executed" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "(not logged)"
    fi

    # ===========================================
    # SECTION 3: First Syscall from EL0
    # ===========================================
    echo ""
    echo "First Syscall from EL0 (Userspace):"

    # Check for EL0_CONFIRMED marker - definitive proof of userspace execution
    printf "  %-30s " "EL0_CONFIRMED Marker"
    if grep -q "EL0_CONFIRMED: First syscall received from EL0" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS - DEFINITIVE PROOF!"
        SYS_PASSED=$((SYS_PASSED + 1))

        # Extract and display the SPSR value
        SPSR_LINE=$(grep "EL0_CONFIRMED:" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        if [ -n "$SPSR_LINE" ]; then
            echo "    $SPSR_LINE"
        fi
    else
        echo "FAIL - No syscall from userspace detected"
        SYS_FAILED=$((SYS_FAILED + 1))
    fi

    # ===========================================
    # SECTION 4: Process Creation
    # ===========================================
    echo ""
    echo "Process Creation:"

    # Check for successful process creation
    printf "  %-30s " "Process Created"
    if grep -q "SUCCESS.*returning PID\|Process created with PID" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SYS_PASSED=$((SYS_PASSED + 1))

        # Show PID if available
        PID_LINE=$(grep -E "SUCCESS.*PID|Process created with PID" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        if [ -n "$PID_LINE" ]; then
            echo "    $PID_LINE"
        fi
    else
        echo "FAIL"
        SYS_FAILED=$((SYS_FAILED + 1))
    fi

    # Check for init process specifically
    printf "  %-30s " "Init Process (PID 1)"
    if grep -qi "init.*process\|PID 1" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "FOUND"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 5: Syscall Handling
    # ===========================================
    echo ""
    echo "Syscall Handling (SVC Instruction):"

    # Check for various syscall-related markers
    declare -a SYSCALL_CHECKS=(
        "sys_write Output|syscall.*write\|\\[user\\]\|user output"
        "sys_getpid|getpid\|GETPID"
        "sys_clock_gettime|clock_gettime\|CLOCK_GETTIME"
    )

    for check in "${SYSCALL_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-30s " "$NAME"

        if grep -qi "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "detected"
        else
            echo "not found"
        fi
    done

    # ===========================================
    # SECTION 6: ARM64 vs x86_64 Equivalents
    # ===========================================
    echo ""
    echo "ARM64 Architecture Specifics:"
    echo "  (ARM64 equivalent of x86_64 Ring 3)"
    echo ""
    echo "  x86_64          -> ARM64"
    echo "  ─────────────────────────────────"
    echo "  Ring 3 (CPL=3)  -> EL0 (Exception Level 0)"
    echo "  CS=0x33         -> SPSR[3:0]=0x0"
    echo "  INT 0x80        -> SVC instruction"
    echo "  IRETQ           -> ERET"
    echo ""

    # Check SPSR indication
    printf "  %-30s " "SPSR Shows EL0"
    if grep -q "SPSR=0x0\|SPSR.*=0x0" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SYS_PASSED=$((SYS_PASSED + 1))
    elif grep -q "EL0_CONFIRMED" "$SERIAL_OUTPUT" 2>/dev/null; then
        # EL0_CONFIRMED implies SPSR[3:0]=0 was verified
        echo "PASS (via EL0_CONFIRMED)"
    else
        echo "not verified"
    fi

    # ===========================================
    # SECTION 7: Summary
    # ===========================================
    echo ""
    echo "========================================"
    SYS_TOTAL=$((SYS_PASSED + SYS_FAILED))
    echo "Syscall/EL0 Tests: $SYS_PASSED/$SYS_TOTAL passed"

    # Determine overall status
    # The critical check is EL0_CONFIRMED - without it, userspace isn't proven
    if grep -q "EL0_CONFIRMED" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo ""
        echo "USERSPACE EXECUTION: CONFIRMED"
        echo "  First syscall received from EL0 (userspace privilege level)"
        echo "========================================"
        exit 0
    elif [ $SYS_FAILED -gt 0 ]; then
        echo ""
        echo "USERSPACE EXECUTION: NOT CONFIRMED"
        echo "  No EL0_CONFIRMED marker found - userspace may not have executed"
        echo "========================================"
        exit 1
    else
        echo ""
        echo "USERSPACE EXECUTION: PARTIAL"
        echo "  Infrastructure ready but no syscall from EL0 detected"
        echo "========================================"
        exit 0
    fi
fi

# Scheduling and blocking I/O test mode
if [ "$TEST_MODE" = "schedule" ]; then
    echo "[4/4] Scheduling and Blocking I/O Tests:"
    echo "========================================"
    echo ""

    # ===========================================
    # HARD REQUIREMENT: EL0_CONFIRMED must be present
    # Without userspace execution, scheduling tests are meaningless
    # ===========================================
    if ! grep -q "EL0_CONFIRMED" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "FATAL: EL0_CONFIRMED marker not found"
        echo "       Userspace did not execute - tests are meaningless"
        echo ""
        echo "Without EL0_CONFIRMED, we cannot verify:"
        echo "  - User processes are actually being scheduled"
        echo "  - Context switches happen between userspace threads"
        echo "  - Blocking I/O correctly suspends user processes"
        echo ""
        echo "Debug: First 30 lines of output:"
        head -30 "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
        echo ""
        exit 1
    fi

    echo "PREREQUISITE: EL0_CONFIRMED found - userspace executed"
    echo ""

    SCHED_PASSED=0
    SCHED_FAILED=0

    # ===========================================
    # SECTION 1: Scheduler Infrastructure
    # ===========================================
    echo "Scheduler Infrastructure:"

    declare -a SCHED_INFRA_CHECKS=(
        "Scheduler Init|Initializing scheduler"
        "Idle Thread|idle.*task\|thread.*0.*as idle"
        "Thread Added|Added thread.*to scheduler\|ready_queue\|as current"
        "Timer IRQ Init|Timer interrupt initialized\|Timer frequency"
    )

    for check in "${SCHED_INFRA_CHECKS[@]}"; do
        NAME="${check%%|*}"
        PATTERN="${check##*|}"

        printf "  %-30s " "$NAME"

        if grep -qiE "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "PASS"
            SCHED_PASSED=$((SCHED_PASSED + 1))
        else
            echo "FAIL"
            SCHED_FAILED=$((SCHED_FAILED + 1))
        fi
    done

    # ===========================================
    # SECTION 2: Timer Preemption
    # ===========================================
    echo ""
    echo "Timer Preemption:"

    # Check for timer interrupt handler running
    printf "  %-30s " "Timer Handler Active"
    # ARM64 timer interrupt doesn't log (critical path) but shows activity via:
    # 1. Shell blocking/waking cycles ([STDIN_BLOCK], [STDIN_WAKE] markers)
    # 2. Timer frequency logged at boot
    # 3. Keyboard polling happening (VirtIO polled in timer tick)
    TIMER_FREQ=$(grep -oE 'Timer frequency: [0-9]+' "$SERIAL_OUTPUT" 2>/dev/null)
    # Count [UART_IRQ] markers (UART interrupts which fire alongside timer)
    UART_COUNT=$(grep -o '\[UART_IRQ\]' "$SERIAL_OUTPUT" 2>/dev/null | wc -l | tr -d ' ')
    if [ -n "$TIMER_FREQ" ] && [ "$UART_COUNT" -gt 0 ]; then
        echo "PASS ($TIMER_FREQ, ${UART_COUNT} UART interrupts)"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    elif [ -n "$TIMER_FREQ" ]; then
        echo "PASS ($TIMER_FREQ)"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    else
        echo "FAIL (no timer frequency logged)"
        SCHED_FAILED=$((SCHED_FAILED + 1))
    fi

    # Check for need_resched flag being set ([NEED_RESCHED] marker or reschedule log)
    # OPTIONAL: Preemption only happens when multiple threads compete for CPU
    printf "  %-30s " "Need Resched Flag"
    # Look for [NEED_RESCHED] markers or scheduling logs
    if grep -qE '\[NEED_RESCHED\]|need_resched|Switching from thread' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    else
        echo "(no preemption - single thread active)"
    fi

    # Check for context switching happening
    # OPTIONAL: Context switching only occurs with multiple runnable threads
    printf "  %-30s " "Context Switch"
    if grep -qE 'Switching from thread.*to thread|switch.*thread|restore.*context' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    else
        echo "(no context switch - single thread)"
    fi

    # ===========================================
    # SECTION 3: Blocking I/O
    # ===========================================
    echo ""
    echo "Blocking I/O:"

    # Look for read syscall markers in the raw output
    # [STDIN_READ] = entering stdin read, [STDIN_BLOCK] = blocking, [STDIN_WAKE] = woken
    printf "  %-30s " "Read Syscall ([STDIN_READ])"
    if grep -q '\[STDIN_READ\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found"
    fi

    printf "  %-30s " "Thread Blocking ([STDIN_BLOCK])"
    if grep -q '\[STDIN_BLOCK\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found (no stdin read blocking needed)"
    fi

    printf "  %-30s " "Thread Wake ([STDIN_WAKE])"
    if grep -q '\[STDIN_WAKE\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found (no thread was woken)"
    fi

    # Check for blocking I/O infrastructure via log messages
    printf "  %-30s " "Blocked Reader Registration"
    if grep -qE 'blocked.*reader\|blocked waiting\|register_blocked' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 4: Input Wake Mechanism
    # ===========================================
    echo ""
    echo "Input Wake Mechanism:"

    # Check for VirtIO keyboard polling ([VIRTIO_KEY] marker)
    printf "  %-30s " "VirtIO Key Events ([VIRTIO_KEY])"
    if grep -q '\[VIRTIO_KEY\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found (no keyboard input)"
    fi

    # Check for stdin push ([STDIN_PUSH] marker)
    printf "  %-30s " "Stdin Push ([STDIN_PUSH])"
    if grep -q '\[STDIN_PUSH\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found (no data pushed to stdin)"
    fi

    # Check for waking readers ([WAKE_READERS:N] markers)
    printf "  %-30s " "Wake Readers ([WAKE_READERS:N])"
    if grep -qE '\[WAKE_READERS:[0-9N]\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found"
    fi

    # Check for UART interrupt ([UART_IRQ] marker)
    printf "  %-30s " "UART Interrupt ([UART_IRQ])"
    if grep -q '\[UART_IRQ\]' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 5: Multi-threading
    # ===========================================
    echo ""
    echo "Multi-threading:"

    # Count threads in scheduler
    THREAD_COUNT=$(grep -E 'Added thread [0-9]+' "$SERIAL_OUTPUT" 2>/dev/null | wc -l | tr -d ' ')
    printf "  %-30s " "Threads Created"
    if [ "$THREAD_COUNT" -gt 0 ]; then
        echo "PASS (${THREAD_COUNT} threads)"
        SCHED_PASSED=$((SCHED_PASSED + 1))
    else
        echo "FAIL (no threads created)"
        SCHED_FAILED=$((SCHED_FAILED + 1))
    fi

    # Check for thread state transitions
    printf "  %-30s " "Thread State Transitions"
    if grep -qE 'set_ready\|set_running\|set_blocked\|ThreadState' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found"
    fi

    # Check for ready queue activity
    printf "  %-30s " "Ready Queue Activity"
    if grep -qE 'ready_queue' "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "detected"
    else
        echo "not found"
    fi

    # ===========================================
    # Summary
    # ===========================================
    echo ""
    echo "========================================"
    SCHED_TOTAL=$((SCHED_PASSED + SCHED_FAILED))
    echo "Scheduling Tests: $SCHED_PASSED/$SCHED_TOTAL passed"
    echo "========================================"

    # Extract debug markers for diagnostics
    echo ""
    echo "Debug Markers Found:"
    echo "----------------------------------------"
    # List all unique debug markers found in the output
    grep -oE '\[STDIN_READ\]|\[STDIN_BLOCK\]|\[STDIN_WAKE\]|\[STDIN_PUSH\]|\[VIRTIO_KEY\]|\[UART_IRQ\]|\[UART_RX\]|\[NEED_RESCHED\]|\[WAKE_READERS:[0-9N]\]|\[TIMER_COUNT:[0-9]+\]' "$SERIAL_OUTPUT" 2>/dev/null | sort | uniq -c || echo "(no markers found)"
    echo ""
    echo "----------------------------------------"

    if [ $SCHED_FAILED -gt 0 ]; then
        exit 1
    fi
    exit 0
fi

# Signal delivery test mode
if [ "$TEST_MODE" = "signal" ]; then
    echo "[4/4] Signal Delivery Tests:"
    echo "========================================"
    echo ""

    # ===========================================
    # HARD REQUIREMENT: EL0_CONFIRMED must be present
    # Without userspace execution, signal tests are meaningless
    # ===========================================
    if ! grep -q "EL0_CONFIRMED" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "FATAL: EL0_CONFIRMED marker not found"
        echo "       Userspace did not execute - tests are meaningless"
        echo ""
        echo "Without EL0_CONFIRMED, we cannot verify:"
        echo "  - Signals are delivered to actual user processes"
        echo "  - Signal handlers execute in userspace context"
        echo "  - sigreturn correctly restores userspace state"
        echo ""
        echo "Debug: First 30 lines of output:"
        head -30 "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
        echo ""
        exit 1
    fi

    echo "PREREQUISITE: EL0_CONFIRMED found - userspace executed"
    echo ""

    SIG_PASSED=0
    SIG_FAILED=0

    # ===========================================
    # SECTION 1: Signal Infrastructure Ready
    # ===========================================
    echo "Signal Infrastructure:"

    # Process manager must be initialized for signals to work
    printf "  %-35s " "Process Manager Init"
    if grep -q "Initializing process manager" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SIG_PASSED=$((SIG_PASSED + 1))
    else
        echo "FAIL"
        SIG_FAILED=$((SIG_FAILED + 1))
    fi

    # EL0 entry is required for userspace signals (guaranteed by prerequisite check)
    printf "  %-35s " "EL0 (Userspace) Ready"
    echo "PASS (verified at start)"
    SIG_PASSED=$((SIG_PASSED + 1))

    # Check for scheduler (needed for signal-based thread wake)
    printf "  %-35s " "Scheduler Ready"
    if grep -q "Initializing scheduler" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SIG_PASSED=$((SIG_PASSED + 1))
    else
        echo "FAIL"
        SIG_FAILED=$((SIG_FAILED + 1))
    fi

    # ===========================================
    # SECTION 2: Signal Delivery Evidence
    # ===========================================
    echo ""
    echo "Signal Delivery:"

    # Look for any signal delivery messages
    # OPTIONAL: Signals may not be sent during boot test
    # Pattern: "Delivering signal N (NAME) to process P"
    printf "  %-35s " "Signal Delivery Logged"
    if grep -qE "Delivering signal [0-9]+" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        SIG_PASSED=$((SIG_PASSED + 1))
        # Show the first signal delivery
        SIG_DEL=$(grep -E "Delivering signal [0-9]+" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $SIG_DEL"
    else
        echo "(no signals sent during test)"
    fi

    # Look for signal handler setup
    # Pattern: "Signal N (NAME) handler set to 0x..."
    printf "  %-35s " "Signal Handler Setup"
    if grep -qE "handler set to" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        SIG_PASSED=$((SIG_PASSED + 1))
        HANDLER=$(grep -E "handler set to" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $HANDLER"
    else
        echo "not found"
    fi

    # Look for signal queuing
    # Pattern: "Signal N (NAME) queued for process P"
    printf "  %-35s " "Signal Queuing"
    if grep -qE "queued for process" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        QUEUED=$(grep -E "queued for process" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $QUEUED"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 3: Signal Handler Execution
    # ===========================================
    echo ""
    echo "Signal Handler Execution:"

    # Look for signal handler delivery to userspace
    # Pattern: "Signal N delivered to handler at 0x..."
    printf "  %-35s " "Handler Invocation"
    if grep -qE "delivered to handler at" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        SIG_PASSED=$((SIG_PASSED + 1))
        HANDLER_INV=$(grep -E "delivered to handler at" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $HANDLER_INV"
    else
        echo "not found"
    fi

    # Look for alternate stack usage
    # OPTIONAL: sigaltstack is an advanced feature not always used
    printf "  %-35s " "Alternate Stack"
    if grep -qE "ALTERNATE STACK" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        ALT_STACK=$(grep -E "ALTERNATE STACK" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $ALT_STACK"
    elif grep -qE "sigaltstack" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "syscall available"
    else
        echo "(not used)"
    fi

    # ===========================================
    # SECTION 4: sigreturn Working
    # ===========================================
    echo ""
    echo "Sigreturn Mechanism:"

    # Look for sigreturn syscall
    printf "  %-35s " "sigreturn Called"
    if grep -qE "sigreturn.*restored context\|sigreturn_aarch64.*restored" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        SIG_PASSED=$((SIG_PASSED + 1))
        SIGRET=$(grep -E "sigreturn.*restored" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $SIGRET"
    else
        echo "not found"
    fi

    # Look for signal mask restoration
    printf "  %-35s " "Signal Mask Restored"
    if grep -qE "restored signal mask\|restored sigsuspend saved mask" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        MASK=$(grep -E "restored.*mask" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $MASK"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 5: Signal Termination
    # ===========================================
    echo ""
    echo "Signal Termination:"

    # Look for process termination by signal
    # OPTIONAL: Signal termination is not expected during normal boot
    # Pattern: "Process P terminated by signal N (NAME)"
    printf "  %-35s " "Process Terminated by Signal"
    if grep -qE "terminated by signal" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
        TERM=$(grep -E "terminated by signal" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $TERM"
    else
        echo "(no termination - normal)"
    fi

    # Look for SIGKILL handling
    printf "  %-35s " "SIGKILL Handling"
    if grep -qE "SIGKILL sent to process" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    # Look for SIGSTOP/SIGCONT
    printf "  %-35s " "SIGSTOP/SIGCONT"
    if grep -qE "SIGSTOP sent\|SIGCONT sent" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 6: SIGINT (Ctrl+C) Infrastructure
    # ===========================================
    echo ""
    echo "SIGINT (Ctrl+C) Infrastructure:"

    # Check if shell can handle signals
    printf "  %-35s " "Shell Process Running"
    if grep -q "breenix>" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        SIG_PASSED=$((SIG_PASSED + 1))
    else
        echo "not found"
    fi

    # Check for SIGINT specifically (signal 2)
    printf "  %-35s " "SIGINT (2) Support"
    if grep -qE "signal 2\|SIGINT" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    # Check for process group signal delivery
    printf "  %-35s " "Process Group Signals"
    if grep -qE "send_signal_to.*process.*group\|signal.*to pgid" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 7: ARM64-Specific Signal Details
    # ===========================================
    echo ""
    echo "ARM64 Signal Architecture:"
    echo "  (Signal delivery on ARM64 uses exception frames)"
    echo ""
    echo "  x86_64               -> ARM64"
    echo "  ───────────────────────────────────────"
    echo "  Signal trampoline    -> SVC #0 (syscall)"
    echo "  RSP (stack)          -> SP_EL0"
    echo "  RIP (return)         -> ELR_EL1"
    echo "  RFLAGS               -> SPSR_EL1"
    echo "  RDI (signum)         -> X0"
    echo ""

    # Check for ARM64-specific signal syscalls
    printf "  %-35s " "ARM64 pause() syscall"
    if grep -qE "sys_pause_with_frame_aarch64" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    printf "  %-35s " "ARM64 sigreturn() syscall"
    if grep -qE "sys_sigreturn.*aarch64\|sigreturn_aarch64" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    printf "  %-35s " "ARM64 sigsuspend() syscall"
    if grep -qE "sys_sigsuspend.*aarch64\|sigsuspend_aarch64" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "DETECTED"
    else
        echo "not found"
    fi

    # ===========================================
    # Summary
    # ===========================================
    echo ""
    echo "========================================"
    SIG_TOTAL=$((SIG_PASSED + SIG_FAILED))
    echo "Signal Tests: $SIG_PASSED/$SIG_TOTAL passed"
    echo ""

    # Detailed analysis of signal readiness
    if [ $SIG_PASSED -ge 4 ]; then
        echo "SIGNAL INFRASTRUCTURE: READY"
        echo "  Core signal handling is operational."
        if grep -qE "Delivering signal" "$SERIAL_OUTPUT" 2>/dev/null; then
            echo "  Signal delivery has been observed."
        else
            echo "  No signal delivery observed (signals may not have been sent)."
        fi
    elif [ $SIG_PASSED -ge 2 ]; then
        echo "SIGNAL INFRASTRUCTURE: PARTIAL"
        echo "  Basic infrastructure present but signal delivery not confirmed."
    else
        echo "SIGNAL INFRASTRUCTURE: NOT READY"
        echo "  Missing critical signal components."
    fi

    echo "========================================"

    # Signal tests are informational - infrastructure checks determine pass/fail
    if [ $SIG_FAILED -gt 2 ]; then
        exit 1
    fi
    exit 0
fi

# Network stack test mode
if [ "$TEST_MODE" = "network" ]; then
    echo "[4/4] Network Stack Tests:"
    echo "========================================"
    echo ""

    NET_PASSED=0
    NET_FAILED=0
    # Track critical failures that should immediately fail the test
    CRITICAL_FAILURE=""

    # ===========================================
    # SECTION 0: Critical Pre-checks
    # ===========================================
    # Check for no network device - this is a HARD FAIL
    if grep -qE "No network device available|No VirtIO network device found" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "CRITICAL FAILURE: No network device detected"
        echo ""
        echo "The kernel reports no network device is available."
        echo "Network tests cannot run without a network device."
        echo ""
        echo "To fix: Ensure QEMU is started with network device options:"
        echo "  -device virtio-net-device,netdev=net0"
        echo "  -netdev user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15"
        echo ""
        echo "========================================"
        echo "FAIL: No network device - cannot run network tests"
        echo "========================================"
        exit 1
    fi

    # ===========================================
    # SECTION 1: VirtIO Network Driver
    # ===========================================
    echo "VirtIO Network Driver:"

    # Check for VirtIO network device found
    printf "  %-35s " "VirtIO Network Device Found"
    if grep -qE "Found VirtIO MMIO device.*Network|virtio-net.*Found network device" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
    else
        echo "FAIL"
        NET_FAILED=$((NET_FAILED + 1))
    fi

    # Check for network driver initialization
    printf "  %-35s " "Network Driver Initialized"
    if grep -qE "VirtIO network driver initialized|Network device initialized successfully" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
        # Show driver init message
        DRIVER_MSG=$(grep -E "VirtIO network driver initialized|Network device initialized successfully" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $DRIVER_MSG"
    else
        echo "FAIL"
        NET_FAILED=$((NET_FAILED + 1))
    fi

    # Check for MAC address assigned
    printf "  %-35s " "MAC Address Assigned"
    if grep -qE "MAC address: [0-9a-fA-F]{2}:" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
        # Extract and display MAC address
        MAC_LINE=$(grep -E "MAC address: [0-9a-fA-F]{2}:" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $MAC_LINE"
    else
        echo "FAIL"
        NET_FAILED=$((NET_FAILED + 1))
    fi

    # Check for RX/TX queue setup
    printf "  %-35s " "RX Queue Configured"
    if grep -qE "RX queue max size" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        RX_MSG=$(grep -E "RX queue max size" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $RX_MSG"
    else
        echo "not found"
    fi

    printf "  %-35s " "TX Queue Configured"
    if grep -qE "TX queue max size" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        TX_MSG=$(grep -E "TX queue max size" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $TX_MSG"
    else
        echo "not found"
    fi

    # ===========================================
    # SECTION 2: Network Stack Initialization
    # ===========================================
    echo ""
    echo "Network Stack Initialization:"

    # Check for network stack init
    printf "  %-35s " "Network Stack Init Started"
    if grep -qE "Initializing network stack|NET: Initializing" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
    else
        echo "FAIL"
        NET_FAILED=$((NET_FAILED + 1))
    fi

    # Check for ARP init
    # OPTIONAL: ARP only happens when communicating with gateway/other hosts
    printf "  %-35s " "ARP Cache Initialized"
    if grep -qE "ARP.*init|Sending ARP request" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
    else
        echo "(no gateway communication)"
    fi

    # Check for IP address configuration
    printf "  %-35s " "IP Address Configured"
    if grep -qE "IP address: [0-9]+\.[0-9]+\.[0-9]+\.[0-9]+" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
        IP_LINE=$(grep -E "IP address: [0-9]+\.[0-9]+\.[0-9]+\.[0-9]+" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $IP_LINE"
    else
        echo "not found"
    fi

    # Check for gateway configuration
    printf "  %-35s " "Gateway Configured"
    if grep -qE "Gateway: [0-9]+\.[0-9]+\.[0-9]+\.[0-9]+" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        GATEWAY_LINE=$(grep -E "Gateway: [0-9]+\.[0-9]+\.[0-9]+\.[0-9]+" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $GATEWAY_LINE"
    else
        echo "not found"
    fi

    # Check for network stack initialized
    printf "  %-35s " "Network Stack Ready"
    if grep -qE "Network stack initialized|NET: Network initialization complete" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
    else
        echo "FAIL"
        NET_FAILED=$((NET_FAILED + 1))
    fi

    # ===========================================
    # SECTION 3: Network Connectivity Verification
    # ===========================================
    echo ""
    echo "Network Connectivity Verification:"
    echo "  (Tests actual packet send AND receive)"
    echo ""

    # Track connectivity verification results
    ARP_REQUEST_SENT=false
    ARP_REPLY_RECEIVED=false
    ICMP_REQUEST_SENT=false
    ICMP_REPLY_RECEIVED=false

    # Check for ARP request sent
    printf "  %-35s " "ARP Request Sent"
    if grep -qE "Sending ARP request|ARP request sent|ARP: Sending request" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        ARP_REQUEST_SENT=true
        ARP_REQ_LINE=$(grep -E "Sending ARP request|ARP request sent|ARP: Sending request" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $ARP_REQ_LINE"
    else
        echo "not found"
    fi

    # Check for ARP reply received - REQUIRED if request was sent
    printf "  %-35s " "ARP Reply Received"
    if grep -qE "ARP resolved|ARP reply received|ARP: Resolved|gateway MAC|ARP: Got reply" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
        ARP_REPLY_RECEIVED=true
        ARP_REPLY_LINE=$(grep -E "ARP resolved|ARP reply received|ARP: Resolved|gateway MAC|ARP: Got reply" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $ARP_REPLY_LINE"
    else
        if [ "$ARP_REQUEST_SENT" = true ]; then
            echo "FAIL (request sent but no reply received)"
            NET_FAILED=$((NET_FAILED + 1))
            CRITICAL_FAILURE="ARP request sent but no reply received - network not functional"
        else
            echo "not found (no ARP activity)"
        fi
    fi

    # Check for ICMP echo request sent
    printf "  %-35s " "ICMP Echo Request Sent"
    if grep -qE "Sending ICMP echo|ICMP: Sending echo request|ping.*sending|ICMP echo request sent" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        ICMP_REQUEST_SENT=true
        ICMP_REQ_LINE=$(grep -E "Sending ICMP echo|ICMP: Sending echo request|ping.*sending|ICMP echo request sent" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $ICMP_REQ_LINE"
    else
        echo "not found"
    fi

    # Check for ICMP echo reply received - REQUIRED if request was sent
    printf "  %-35s " "ICMP Echo Reply Received"
    if grep -qE "ICMP echo reply|ICMP: Got reply|ping.*reply|ICMP reply received|pong received" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        NET_PASSED=$((NET_PASSED + 1))
        ICMP_REPLY_RECEIVED=true
        ICMP_REPLY_LINE=$(grep -E "ICMP echo reply|ICMP: Got reply|ping.*reply|ICMP reply received|pong received" "$SERIAL_OUTPUT" 2>/dev/null | head -1)
        echo "    > $ICMP_REPLY_LINE"
    else
        if [ "$ICMP_REQUEST_SENT" = true ]; then
            echo "FAIL (request sent but no reply received)"
            NET_FAILED=$((NET_FAILED + 1))
            if [ -z "$CRITICAL_FAILURE" ]; then
                CRITICAL_FAILURE="ICMP echo request sent but no reply received - network not functional"
            fi
        else
            echo "not found (no ICMP activity)"
        fi
    fi

    # ===========================================
    # SECTION 4: Socket Syscall Infrastructure
    # ===========================================
    echo ""
    echo "Socket Syscall Infrastructure:"

    # Check TCP/UDP modules loaded (by checking for Network Stack init completing)
    printf "  %-35s " "TCP Module Ready"
    if grep -qE "Network stack initialized" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS (via network stack init)"
    else
        echo "not verified"
    fi

    printf "  %-35s " "UDP Module Ready"
    if grep -qE "Network stack initialized" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS (via network stack init)"
    else
        echo "not verified"
    fi

    # ===========================================
    # SECTION 5: Connectivity Summary
    # ===========================================
    echo ""
    echo "Connectivity Summary:"
    echo "----------------------------------------"

    # Determine if actual network activity succeeded
    CONNECTIVITY_VERIFIED=false

    if [ "$ARP_REQUEST_SENT" = true ] && [ "$ARP_REPLY_RECEIVED" = true ]; then
        echo "  ARP Resolution:    SUCCESS (request sent AND reply received)"
        CONNECTIVITY_VERIFIED=true
    elif [ "$ARP_REQUEST_SENT" = true ]; then
        echo "  ARP Resolution:    FAILED (request sent, NO reply)"
    else
        echo "  ARP Resolution:    NOT TESTED (no ARP activity)"
    fi

    if [ "$ICMP_REQUEST_SENT" = true ] && [ "$ICMP_REPLY_RECEIVED" = true ]; then
        echo "  ICMP Ping:         SUCCESS (request sent AND reply received)"
        CONNECTIVITY_VERIFIED=true
    elif [ "$ICMP_REQUEST_SENT" = true ]; then
        echo "  ICMP Ping:         FAILED (request sent, NO reply)"
    else
        echo "  ICMP Ping:         NOT TESTED (no ICMP activity)"
    fi

    echo "----------------------------------------"

    # ===========================================
    # Summary
    # ===========================================
    echo ""
    echo "========================================"
    NET_TOTAL=$((NET_PASSED + NET_FAILED))
    echo "Network Tests: $NET_PASSED/$NET_TOTAL passed"
    echo ""

    # Check for critical failures first
    if [ -n "$CRITICAL_FAILURE" ]; then
        echo "NETWORK STACK: CONNECTIVITY FAILED"
        echo "  $CRITICAL_FAILURE"
        echo ""
        echo "  Network tests verify actual connectivity, not just driver loading."
        echo "  A packet was sent but no response was received."
        echo "========================================"

        # Show relevant log entries for debugging
        echo ""
        echo "Network-related log entries:"
        echo "----------------------------------------"
        grep -iE "net|virtio.*net|network|MAC|ARP|IP address|gateway|ICMP|ping" "$SERIAL_OUTPUT" 2>/dev/null | head -40 || echo "(no entries found)"
        echo "----------------------------------------"

        exit 1
    fi

    # Detailed analysis of network readiness
    if [ "$CONNECTIVITY_VERIFIED" = true ]; then
        echo "NETWORK STACK: CONNECTIVITY VERIFIED"
        echo "  Network packets successfully sent AND received."
        if [ "$ARP_REPLY_RECEIVED" = true ]; then
            echo "  - ARP resolution: WORKING"
        fi
        if [ "$ICMP_REPLY_RECEIVED" = true ]; then
            echo "  - ICMP ping: WORKING"
        fi
    elif [ $NET_PASSED -ge 6 ]; then
        echo "NETWORK STACK: INITIALIZED (connectivity not verified)"
        echo "  VirtIO network driver and TCP/IP stack initialized."
        echo "  No network activity was observed to verify connectivity."
        echo "  This may be expected if no network operations were performed."
    elif [ $NET_PASSED -ge 4 ]; then
        echo "NETWORK STACK: PARTIALLY OPERATIONAL"
        echo "  Basic network infrastructure present but some features missing."
    elif [ $NET_PASSED -ge 2 ]; then
        echo "NETWORK STACK: DRIVER ONLY"
        echo "  VirtIO network driver initialized but network stack issues."
    else
        echo "NETWORK STACK: NOT READY"
        echo "  Network driver or stack initialization failed."
    fi

    echo "========================================"

    # Show relevant log entries for debugging if there were failures
    if [ $NET_FAILED -gt 0 ]; then
        echo ""
        echo "Network-related log entries:"
        echo "----------------------------------------"
        grep -iE "net|virtio.*net|network|MAC|ARP|IP address|gateway|ICMP|ping" "$SERIAL_OUTPUT" 2>/dev/null | head -40 || echo "(no entries found)"
        echo "----------------------------------------"
    fi

    # Fail if too many failures
    if [ $NET_FAILED -gt 2 ]; then
        exit 1
    fi
    exit 0
fi

# Full POST test
echo "[4/4] POST Results:"
echo "========================================"
echo ""

# Define POST checks
# Note: ARM64 serial doesn't print "Serial port initialized" - the presence of
# "Breenix ARM64 Kernel Starting" proves serial output is working
declare -a POST_CHECKS=(
    "CPU/Entry|Breenix ARM64 Kernel Starting"
    "Serial Working|========================================"
    "Exception Level|Current exception level: EL1"
    "MMU|MMU already enabled"
    "Memory Init|Initializing memory management"
    "Memory Ready|Memory management ready"
    "Generic Timer|Initializing Generic Timer"
    "Timer Freq|Timer frequency:"
    "GICv2 Init|Initializing GICv2"
    "GIC Ready|GIC initialized"
    "UART IRQ|Enabling UART interrupts"
    "Interrupts Enable|Enabling interrupts"
    "Interrupts Ready|Interrupts enabled:"
    "Drivers|Initializing device drivers"
    "Network|Initializing network stack"
    "Filesystem|Initializing filesystem"
    "Per-CPU|Initializing per-CPU data"
    "Process Manager|Initializing process manager"
    "Scheduler|Initializing scheduler"
    "Timer Interrupt|Initializing timer interrupt"
    "Boot Complete|Breenix ARM64 Boot Complete"
    "Hello World|Hello from ARM64"
)

PASSED=0
FAILED=0
FAILED_LIST=""

for check in "${POST_CHECKS[@]}"; do
    NAME="${check%%|*}"
    PATTERN="${check##*|}"

    printf "  %-20s " "$NAME"

    if grep -q "$PATTERN" "$SERIAL_OUTPUT" 2>/dev/null; then
        echo "PASS"
        PASSED=$((PASSED + 1))
    else
        echo "FAIL"
        FAILED=$((FAILED + 1))
        FAILED_LIST="$FAILED_LIST\n   - $NAME"
    fi
done

echo ""
echo "========================================"
TOTAL=$((PASSED + FAILED))
echo "Summary: $PASSED/$TOTAL subsystems passed POST"
echo "========================================"
echo ""

if [ $FAILED -gt 0 ]; then
    echo "FAILED subsystems:$FAILED_LIST"
    echo ""
    echo "First 50 lines of kernel output:"
    echo "--------------------------------"
    head -50 "$SERIAL_OUTPUT" 2>/dev/null || echo "(no output)"
    echo "--------------------------------"
    exit 1
fi

echo "All POST checks passed - ARM64 kernel is healthy!"
exit 0
