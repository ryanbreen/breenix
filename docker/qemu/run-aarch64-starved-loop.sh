#!/usr/bin/env bash
# #586: exercise the strict boot oracle under CPU contention. This delegates
# QEMU ownership to the strict gate, including qemu-host-lock.sh acquisition,
# PID cleanup, and gate-boot-facts.sh measurements. CPU hogs are not QEMU peers:
# competing holders of the exclusive QEMU lock would serialize, not starve it.
# The PR 3 landing runs the strict gate directly; this contention probe is separate evidence.
# Usage: run-aarch64-starved-loop.sh [cycles] [hogs]
# A green strict boot alone does not satisfy this gate: at least one idle wake
# test must recover from a measured starved window using an extension.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CYCLES=${1:-10}
HOGS=${2:-14}
for value in "$CYCLES" "$HOGS"; do
    if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
        echo 'GATE: FAIL (cycles and hogs must be positive integers)'
        exit 2
    fi
done
BREENIX_GATE_TMP=${BREENIX_GATE_TMP:-/tmp}
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo 'GATE: FAIL (BREENIX_GATE_TMP must be absolute)'; exit 2 ;;
esac
mkdir -p "$BREENIX_GATE_TMP"
RUN_DIR=$(mktemp -d "$BREENIX_GATE_TMP/breenix_aarch64_starved.XXXXXX")
echo "STARVED_LOOP: cycles=$CYCLES hogs=$HOGS evidence=$RUN_DIR"
HOG_PIDS=()
STRICT_PID=""
TEE_PID=""
cleanup() {
    local pid
    if [ -n "$STRICT_PID" ]; then
        kill "$STRICT_PID" 2>/dev/null || true
        wait "$STRICT_PID" 2>/dev/null || true
    fi
    if [ -n "$TEE_PID" ]; then
        kill "$TEE_PID" 2>/dev/null || true
        wait "$TEE_PID" 2>/dev/null || true
    fi
    for pid in "${HOG_PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for ((i=0; i<HOGS; i++)); do
    python3 -c 'while True: pass' &
    HOG_PIDS+=("$!")
done
# One invocation preserves strict's structure preflight and exit semantics.
# The unique directory retains each boot's serial, including failed boots.
mkfifo "$RUN_DIR/gate.pipe"
tee "$RUN_DIR/gate.txt" < "$RUN_DIR/gate.pipe" &
TEE_PID=$!
BREENIX_GATE_TMP="$RUN_DIR" bash "$SCRIPT_DIR/run-aarch64-boot-test-strict.sh" "$CYCLES" > "$RUN_DIR/gate.pipe" 2>&1 &
STRICT_PID=$!
STRICT_RC=0
wait "$STRICT_PID" || STRICT_RC=$?
STRICT_PID=""
wait "$TEE_PID"
TEE_PID=""
rm "$RUN_DIR/gate.pipe"
cleanup
HOG_PIDS=()
python3 - "$RUN_DIR" "$CYCLES" "$STRICT_RC" <<'PY'
import pathlib
import re
import sys
root, cycles, strict_rc = pathlib.Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
recovered = 0
for cycle in range(1, cycles + 1):
    serial = root / f'breenix_aarch64_strict_{cycle}' / 'serial.txt'
    text = serial.read_text(errors='replace') if serial.exists() else ''
    records = re.findall(r'\[LOOPBACK_WAKE_BUDGET:([^\]\s]+)\]', text)
    idle = [dict(field.split('=', 1) for field in record.split(':'))
            for record in records if ':test=when_idle:' in ':' + record + ':']
    passed = '[TEST:network:loopback_recv_wake_when_idle:PASS]' in text
    extended = len(idle) == 1 and idle[0].get('verdict') == 'starved' and int(idle[0].get('extensions', '0')) > 0
    recovered += bool(passed and extended)
    print(f'STARVED_LOOP_BOOT:boot={cycle}:recovered={int(passed and extended)}:serial={serial}')
print(f'STARVED_LOOP:boots={cycles}:recovered={recovered}:strict_exit={strict_rc}')
sys.exit(0 if strict_rc == 0 and recovered > 0 else 1)
PY
