#!/usr/bin/env bash
# F21 VirGL scanout bisect verdict.
#
# git-bisect exit semantics:
#   0   known-good display update (cornflower-blue-ish or rendered non-red)
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
# #860: this invocation's own Parallels VM. git-bisect runs this script as a
# fresh process per step, so a plain shell variable cannot carry a VM name
# across steps -- VM_STATE_FILE (below) is the cross-invocation memory that
# replaces the old pattern-sweep for that purpose.
VM_NAME=""
RUN_PID=""
# #860: persists the name of the one Parallels VM this script's own lineage
# (this RUN_DIR) most recently started, so the next invocation can reap
# exactly that VM -- and only that VM -- if a prior step exited without a
# clean stop (e.g. killed before its EXIT trap could run). Scoped to
# RUN_DIR, which is itself per-lineage (F21_RUN_DIR), so two concurrent
# bisect runs using different RUN_DIRs never share, read, or clear each
# other's state file.
# claim-lint:ok: #860 -- structural, not a measured claim: two shell
# variables computed from two different F21_RUN_DIR values name two
# different paths, so this file's own read/write/rm calls below cannot
# reach each other's file.
VM_STATE_FILE="$RUN_DIR/last-vm-name"

log() {
    printf '[f21-bisect] %s\n' "$*"
}

record() {
    if [ -n "$SCRATCHPAD" ]; then
        printf '%s\n' "$*" >> "$SCRATCHPAD"
    fi
}

# #849: this cleanup used to also run `pkill -9 qemu-system-x86` and
# `killall -9 qemu-system-x86_64` -- a bare host-wide pattern match that
# could kill a DIFFERENT, unrelated x86 gate's own QEMU process running
# concurrently on this host. This script never launches qemu-system-x86_64
# itself (its own resources are the named Parallels VM below, stopped by
# name, and $RUN_PID, this script's own `./run.sh` child, stopped by PID),
# so those two lines had no legitimate target of their own to reach.
# claim-lint:ok: #849
cleanup() {
    if [ -n "${VM_NAME:-}" ]; then
        prlctl stop "$VM_NAME" --kill >/dev/null 2>&1 || true
        prlctl delete "$VM_NAME" >/dev/null 2>&1 || true
        # #860: this invocation cleanly stopped/deleted the one VM it
        # recorded as its own, so clear VM_STATE_FILE rather than leaving a
        # now-deleted VM's name behind for the next invocation to (harmlessly
        # but pointlessly) attempt to reap.
        if [ -n "${VM_STATE_FILE:-}" ]; then
            rm -f "$VM_STATE_FILE" 2>/dev/null || true
        fi
    fi
    if [ -n "${RUN_PID:-}" ]; then
        kill "$RUN_PID" >/dev/null 2>&1 || true
    fi
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

# Best-effort cleanup before each VM run to avoid stale locks left by a
# PRIOR invocation of this same script -- git-bisect runs this script as a
# fresh process per step, so there is no in-process $VM_NAME to fall back
# on the way the EXIT trap above uses for the CURRENT run.
#
# #860: this used to be `prlctl list --all | awk '/breenix-/ {...}'` fed
# into a stop+delete loop -- a bare host-wide name-pattern sweep that
# reaped EVERY VM matching `breenix-`, not only one this script's own
# lineage started. That could kill a different, concurrent Parallels-based
# gate's VM, or a human's own long-running `breenix-dev` VM, out from
# under it, purely because the name happened to match -- the same
# kill-by-name-pattern hazard class #829/#849 fixed for
# qemu-system-x86_64/qemu-system-aarch64 processes and Docker containers,
# one resource type over. Replaced with a reap of exactly the one VM name
# VM_STATE_FILE remembers this lineage having started (written below, the
# moment this run's own VM_NAME is discovered, and cleared by a clean
# EXIT-trap stop) -- never a pattern match, so a VM this script did not
# itself start is never a candidate no matter what its name is. This
# script never launches qemu-system-x86_64 itself (Parallels is a
# hypervisor, not QEMU), so the #849-removed `pkill -9 qemu-system-x86` /
# `killall -9 qemu-system-x86_64` here could likewise only ever have
# reached a DIFFERENT, unrelated x86 gate's own QEMU process.
# claim-lint:ok: #849,#860
if [ -f "$VM_STATE_FILE" ]; then
    PREV_VM_NAME="$(cat "$VM_STATE_FILE" 2>/dev/null || true)"
    if [ -n "$PREV_VM_NAME" ]; then
        log "reaping this lineage's own stale VM from a prior step: ${PREV_VM_NAME}"
        prlctl stop "$PREV_VM_NAME" --kill >/dev/null 2>&1 || true
        prlctl delete "$PREV_VM_NAME" >/dev/null 2>&1 || true
    fi
    rm -f "$VM_STATE_FILE" 2>/dev/null || true
fi

RUN_ARGS=(--parallels)
RUN_HELP="$(./run.sh --help 2>/dev/null || true)"
if [[ "$RUN_HELP" == *"--parallels --test"* ]]; then
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
        # #860: record this run's own VM name the moment it is known, so a
        # hard kill of this script (before the EXIT trap can run) still
        # leaves the next invocation exactly this one name to reap -- never
        # a pattern to sweep by.
        # claim-lint:ok: #860 -- structural: the reap block below reads
        # only VM_STATE_FILE's literal contents into PREV_VM_NAME, never a
        # `prlctl list` query, so nothing it stops/deletes can be
        # pattern-matched.
        printf '%s\n' "$VM_NAME" > "$VM_STATE_FILE"
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

# The first verified good commit captures CSS cornflower blue: (100, 149, 237).
# Later known-good commits render a dark desktop/compositor pattern instead of
# cornflower blue, so rendered non-red content is also scanout-good.
blue_good = r < 150 and 130 <= g <= 180 and b > 200
red_bad = solid_red or (r > 200 and g < 80 and b < 80) or redish >= 0.95
rendered_nonred_good = bool(data.get("passes_rendered_desktop_bar", False)) and redish < 0.10

if blue_good or rendered_nonred_good:
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
