#!/usr/bin/env bash
# F21 VirGL scanout bisect verdict.
#
# git-bisect exit semantics:
#   0   known-good display update (cornflower-blue-ish)
#   1   known-bad display update (solid red)
#   125 skip inconclusive/build/capture failures

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

SHA="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
RUN_DIR="${F21_RUN_DIR:-/tmp/f21-bisect}"
mkdir -p "$RUN_DIR"

RUN_LOG="$RUN_DIR/run-${SHA}.log"
CAPTURE_OUT="${F21_CAPTURE_OUT:-$RUN_DIR/capture-${SHA}.png}"
SERIAL_COPY="$RUN_DIR/serial-${SHA}.log"
SCRATCHPAD="${F21_SCRATCHPAD:-}"
VM_NAME=""
RUN_PID=""

log() {
    printf '[f21-bisect] %s\n' "$*"
}

record() {
    if [ -n "$SCRATCHPAD" ]; then
        printf '%s\n' "$*" >> "$SCRATCHPAD"
    fi
}

cleanup() {
    if [ -n "${VM_NAME:-}" ]; then
        prlctl stop "$VM_NAME" --kill >/dev/null 2>&1 || true
        prlctl delete "$VM_NAME" >/dev/null 2>&1 || true
    fi
    if [ -n "${RUN_PID:-}" ]; then
        kill "$RUN_PID" >/dev/null 2>&1 || true
    fi
    pkill -9 qemu-system-x86 >/dev/null 2>&1 || true
    killall -9 qemu-system-x86_64 >/dev/null 2>&1 || true
}
trap cleanup EXIT

skip() {
    log "SKIP: $*"
    record "- ${SHA}: SKIP — $*"
    exit 125
}

if ! command -v prlctl >/dev/null 2>&1; then
    skip "prlctl not available"
fi
if ! command -v python3 >/dev/null 2>&1; then
    skip "python3 not available"
fi

CAPTURE_SCRIPT="${F21_CAPTURE_SCRIPT:-}"
if [ -z "$CAPTURE_SCRIPT" ]; then
    if [ -x "$ROOT/scripts/parallels/capture-display.sh" ]; then
        CAPTURE_SCRIPT="$ROOT/scripts/parallels/capture-display.sh"
    elif [ -x /tmp/f21-capture-display.sh ]; then
        CAPTURE_SCRIPT=/tmp/f21-capture-display.sh
    else
        skip "capture-display.sh unavailable"
    fi
fi

rm -f "$RUN_LOG" "$CAPTURE_OUT" "${CAPTURE_OUT}.stats.json" "$SERIAL_COPY"
log "testing ${SHA}"
record "- ${SHA}: starting"

# Best-effort cleanup before each VM run to avoid stale locks.
for old_vm in $(prlctl list --all 2>/dev/null | awk '/breenix-/ {print $NF}'); do
    prlctl stop "$old_vm" --kill >/dev/null 2>&1 || true
    prlctl delete "$old_vm" >/dev/null 2>&1 || true
done
pkill -9 qemu-system-x86 >/dev/null 2>&1 || true
killall -9 qemu-system-x86_64 >/dev/null 2>&1 || true

RUN_ARGS=(--parallels)
if ./run.sh --help 2>/dev/null | grep -q -- '--parallels --test'; then
    RUN_ARGS+=(--test "${F21_PARALLELS_TEST_SECONDS:-90}")
else
    log "run.sh at ${SHA} lacks --test; using legacy --parallels path"
fi

./run.sh "${RUN_ARGS[@]}" >"$RUN_LOG" 2>&1 &
RUN_PID=$!

VM_START_TIMEOUT="${F21_VM_START_TIMEOUT:-900}"
for second in $(seq 1 "$VM_START_TIMEOUT"); do
    VM_NAME="$(awk '/^VM:/ {print $2}' "$RUN_LOG" 2>/dev/null | tail -1 || true)"
    if [ -n "$VM_NAME" ]; then
        log "VM started: ${VM_NAME}"
        break
    fi
    if ! kill -0 "$RUN_PID" >/dev/null 2>&1; then
        tail -120 "$RUN_LOG" >&2 || true
        skip "run.sh exited before VM start"
    fi
    sleep 1
done

if [ -z "$VM_NAME" ]; then
    tail -120 "$RUN_LOG" >&2 || true
    skip "timed out waiting for VM start"
fi

if grep -E '^(warning|error)(\[|:)' "$RUN_LOG" >/dev/null 2>&1; then
    grep -E '^(warning|error)(\[|:)' "$RUN_LOG" >&2 || true
    skip "compile-stage warnings/errors detected"
fi

CAPTURE_DELAY="${F21_CAPTURE_DELAY:-75}"
if ! BREENIX_CAPTURE_RETRY_SCHEDULE="$CAPTURE_DELAY" \
    BREENIX_CAPTURE_BASELINE_DIR="$RUN_DIR/baseline" \
    "$CAPTURE_SCRIPT" "$VM_NAME" "$CAPTURE_OUT"; then
    tail -120 "$RUN_LOG" >&2 || true
    skip "display capture failed"
fi

if [ -f /tmp/breenix-parallels-serial.log ]; then
    cp /tmp/breenix-parallels-serial.log "$SERIAL_COPY" || true
fi

STATS_FILE="${CAPTURE_OUT}.stats.json"
if [ ! -s "$STATS_FILE" ]; then
    skip "capture stats missing"
fi

verdict="$(
python3 - "$STATS_FILE" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)

rgb = data.get("dominant_rgb")
if not isinstance(rgb, list) or len(rgb) != 3:
    print("skip invalid-rgb")
    sys.exit(0)

r, g, b = [int(v) for v in rgb]
redish = float(data.get("redish_fraction", 0.0))
solid_red = bool(data.get("solid_red", False))

# The verified good commit captures CSS cornflower blue: (100, 149, 237).
# Keep a small tolerance around that exact value instead of using a brittle
# green > 150 cutoff that would reject the known-good capture.
blue_good = r < 150 and 130 <= g <= 180 and b > 200
red_bad = solid_red or (r > 200 and g < 80 and b < 80) or redish >= 0.95

if blue_good:
    print(f"good rgb={r},{g},{b}")
elif red_bad:
    print(f"bad rgb={r},{g},{b}")
else:
    print(f"skip rgb={r},{g},{b}")
PY
)"

log "capture stats: $(cat "$STATS_FILE")"
case "$verdict" in
    good\ *)
        log "GOOD: ${verdict#good }"
        record "- ${SHA}: GOOD — ${verdict#good }, capture=$CAPTURE_OUT"
        exit 0
        ;;
    bad\ *)
        log "BAD: ${verdict#bad }"
        record "- ${SHA}: BAD — ${verdict#bad }, capture=$CAPTURE_OUT"
        exit 1
        ;;
    *)
        skip "inconclusive ${verdict#skip }"
        ;;
esac

