#!/bin/bash
# Boot cycle test for fork+exit hang verification
# Runs N boot cycles and checks each for successful fork+exit tests

set -e

TOTAL=${1:-10}
PASS=0
FAIL=0
WAIT_SECS=55
VM="breenix-dev"
SERIAL="/tmp/breenix-parallels-serial.log"

for cycle in $(seq 1 "$TOTAL"); do
    echo "=== CYCLE $cycle / $TOTAL ==="

    # Force-stop VM
    prlctl stop "$VM" --kill 2>/dev/null || true
    for i in $(seq 1 15); do
        if prlctl status "$VM" 2>/dev/null | grep -q stopped; then break; fi
        sleep 1
    done

    # Truncate serial log, delete NVRAM, start
    > "$SERIAL"
    rm -f ~/Parallels/${VM}.pvm/NVRAM.dat
    prlctl start "$VM" >/dev/null 2>&1

    echo "  Started, waiting ${WAIT_SECS}s..."
    sleep "$WAIT_SECS"

    # Check VM state
    VM_STATE=$(prlctl status "$VM" 2>/dev/null | awk '{print $NF}')
    LOG_LINES=$(wc -l < "$SERIAL" 2>/dev/null || echo 0)

    # Check result
    if grep -q "all 5 iterations completed successfully" "$SERIAL" 2>/dev/null; then
        PASS=$((PASS + 1))
        echo "  RESULT: PASS ($PASS passes / $cycle cycles)"
    elif grep -q "SOFT LOCKUP" "$SERIAL" 2>/dev/null; then
        FAIL=$((FAIL + 1))
        echo "  RESULT: HANG (soft lockup)"
        grep -A 5 "SOFT LOCKUP" "$SERIAL" | head -10
    elif grep -q "TEST 1/5: fork+exit" "$SERIAL" 2>/dev/null; then
        if ! grep -q "TEST.*completed" "$SERIAL" 2>/dev/null; then
            FAIL=$((FAIL + 1))
            echo "  RESULT: HANG (fork+exit started, didn't complete)"
            tail -10 "$SERIAL"
        fi
    elif grep -q "idling" "$SERIAL" 2>/dev/null; then
        FAIL=$((FAIL + 1))
        echo "  RESULT: NO EXT2 (idling without tests)"
    elif [ "$LOG_LINES" -eq 0 ]; then
        FAIL=$((FAIL + 1))
        echo "  RESULT: NO BOOT (empty serial, VM=$VM_STATE)"
    else
        FAIL=$((FAIL + 1))
        echo "  RESULT: UNKNOWN ($LOG_LINES lines, VM=$VM_STATE)"
        tail -5 "$SERIAL"
    fi
done

# Final stop
prlctl stop "$VM" --kill 2>/dev/null || true

echo ""
echo "========================================"
echo "  RESULTS: $PASS / $TOTAL passed"
echo "  Failures: $FAIL"
echo "========================================"
