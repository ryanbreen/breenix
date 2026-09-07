#!/usr/bin/env bash
# Repeat strict boots under host contention; a recovered starvation receipt is required.
# Usage: run-aarch64-starved-loop.sh [cycles=10] [hogs=14]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CYCLES="${1:-10}"
HOGS="${2:-14}"
for value in "$CYCLES" "$HOGS"; do
    [[ "$value" =~ ^[0-9]+$ ]] || { echo 'cycles/hogs must be integers'; exit 2; }
done
(( CYCLES > 0 )) || exit 2
: "${BREENIX_GATE_TMP:?set a lane-unique absolute directory}"
[[ "$BREENIX_GATE_TMP" = /* ]] || exit 2
OUT="${BREENIX_STARVED_OUTPUT:-$BREENIX_GATE_TMP/starved-$(date +%Y%m%dT%H%M%S)}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
pids=()
cleanup() {
    local pid
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# The delegated strict gate owns the QEMU lock, QEMU PID and GATE_BOOT_FACTS.
for ((hog=0; hog<HOGS; hog++)); do
    python3 -c 'while True: pass' &
    pids+=("$!")
done
revision="$(git -C "$ROOT" rev-parse HEAD)"
printf 'revision=%s cycles=%s hogs=%s\n' "$revision" "$CYCLES" "$HOGS" | tee "$OUT/run.txt"
failed=0
recovered=0
for ((cycle=1; cycle<=CYCLES; cycle++)); do
    cycle_dir="$OUT/cycle-$cycle"
    mkdir -p "$cycle_dir"
    rc=0
    BREENIX_GATE_TMP="$cycle_dir" bash "$ROOT/docker/qemu/run-aarch64-boot-test-strict.sh" 1 > "$cycle_dir/gate.txt" 2>&1 || rc=$?
    serial="$cycle_dir/breenix_aarch64_strict_1/serial.txt"
    receipt=0
    if [[ -f "$serial" ]] && grep -qE '\[LOOPBACK_WAKE_BUDGET:.*:test=when_idle:.*:extensions=[1-9][0-9]*:.*:verdict=starved\]' "$serial" && grep -qF '[TEST:network:loopback_recv_wake_when_idle:PASS]' "$serial"; then
        receipt=1
        recovered=$((recovered+1))
    fi
    (( rc == 0 )) || failed=$((failed+1))
    printf '[STARVED_LOOP_FACTS:revision=%s:cycle=%s:hogs=%s:gate_exit=%s:recovered=%s]\n' "$revision" "$cycle" "$HOGS" "$rc" "$receipt" | tee -a "$OUT/run.txt"
done
printf '[STARVED_LOOP_RESULT:cycles=%s:failed=%s:recovered=%s]\n' "$CYCLES" "$failed" "$recovered" | tee -a "$OUT/run.txt"
(( failed == 0 && recovered > 0 ))
