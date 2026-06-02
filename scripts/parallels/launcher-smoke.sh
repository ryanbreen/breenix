#!/usr/bin/env bash
#
# launcher-smoke.sh — ONE full launcher->terminal smoke run on a fresh Parallels VM.
#
# Flow under test:
#   boot (run.sh --parallels) -> BWM ready -> double-tap SUPER opens the launcher
#   (/bin/blauncher, pre-selecting APPS[0] = "Terminal") -> Enter launches the
#   terminal (/bin/bterm). PASS requires REAL serial evidence that bterm spawned
#   AND emitted its config line — never "launcher opened" alone.
#
# Usage:
#   scripts/parallels/launcher-smoke.sh [--no-build] [--keep-vm]
#                                       [--timeout SECS] [--type-filter]
#
# Final stdout line is EXACTLY one of:
#   RESULT: PASS                      (exit 0)
#   RESULT: FAIL: <reason>            (exit 1)
#
# Callers must run this un-sandboxed (a wrapper passes dangerouslyDisableSandbox);
# this script contains no sandbox logic.
#
set -euo pipefail

# =============================================================================
# INJECTION METHOD CONFIG — tune the trigger in ONE place.
# Super = PS/2 set-1 extended scancode 0xE0 0x5B => prefix 224 (0xE0), code 91 (0x5B).
# A "tap" = press/release of the code (wrapped by the extended prefix).
# A "double-tap" = two taps within 400 ms; we use INTER_TAP_MS gap + ~40 ms hold.
# If the proven trigger ever changes (different key, non-extended, etc.), edit
# THESE values (and ENTER_CODE) — nothing else in this script needs to change.
# =============================================================================
SUPER_PREFIX=224       # 0xE0 extended prefix
SUPER_CODE=91          # 0x5B left-GUI / Super
INTER_TAP_MS=150       # gap between the two Super taps (must be < 400 ms)
ENTER_CODE=28          # Enter / Return

# =============================================================================
# Other tunables
# =============================================================================
READY_MARKER='[bwm] hotkeys: using built-in defaults for early boot'
LAUNCHER_MARKER="[spawn] path='/bin/blauncher'"
BTERM_SPAWN_MARKER="[spawn] path='/bin/bterm'"
BTERM_CONFIG_MARKER='[bterm] config:'
WARMUP_SECS=60         # VirGL warmup after readiness marker
POST_SUPER_WAIT=1.5    # settle after double-Super before grepping for launcher
POST_ENTER_WAIT=2      # settle after Enter before grepping for bterm
FILTER_TEXT='term'     # typed when --type-filter is set (Terminal stays index 0)

# =============================================================================
# Argument parsing
# =============================================================================
NO_BUILD=0
KEEP_VM=0
OVERALL_TIMEOUT=900
TYPE_FILTER=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build)    NO_BUILD=1 ;;
        --keep-vm)     KEEP_VM=1 ;;
        --type-filter) TYPE_FILTER=1 ;;
        --timeout)     OVERALL_TIMEOUT="${2:?--timeout needs a value}"; shift ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "launcher-smoke.sh: unknown flag: $1" >&2; exit 2 ;;
    esac
    shift
done

# =============================================================================
# Paths
# =============================================================================
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
INJECT="$SCRIPT_DIR/inject.sh"
CAPTURE="$SCRIPT_DIR/capture-display.sh"
RUN_SH="$BREENIX_ROOT/run.sh"

RUN_TS="$(date +%Y%m%d-%H%M%S)"
EVIDENCE_DIR="$BREENIX_ROOT/logs/parallels-launcher-test/run-$RUN_TS"
mkdir -p "$EVIDENCE_DIR"
RESULT_FILE="$EVIDENCE_DIR/result.txt"
SERIAL_EXCERPT="$EVIDENCE_DIR/serial-excerpt.txt"
RUN_LOG="$EVIDENCE_DIR/run-sh.log"

START_EPOCH="$(date +%s)"

# State carried into cleanup / final report.
RUN_PID=""
VM_NAME=""
FINAL_REASON=""
CAFFEINATE_PID=""

log() { printf '[smoke %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

# =============================================================================
# Cleanup trap — always kill the backgrounded run.sh; stop the VM unless --keep-vm.
# =============================================================================
cleanup() {
    local rc=$?
    if [[ -n "$RUN_PID" ]] && kill -0 "$RUN_PID" 2>/dev/null; then
        log "cleanup: killing run.sh pid $RUN_PID"
        kill "$RUN_PID" 2>/dev/null || true
        # run.sh spawns children (tail -f); reap the process group best-effort.
        pkill -P "$RUN_PID" 2>/dev/null || true
    fi
    if [[ -n "$CAFFEINATE_PID" ]] && kill -0 "$CAFFEINATE_PID" 2>/dev/null; then
        log "cleanup: killing caffeinate pid $CAFFEINATE_PID"
        kill "$CAFFEINATE_PID" 2>/dev/null || true
    fi
    if [[ "$KEEP_VM" -eq 0 && -n "$VM_NAME" ]]; then
        log "cleanup: stopping VM $VM_NAME"
        prlctl stop "$VM_NAME" --kill >/dev/null 2>&1 || true
    elif [[ -n "$VM_NAME" ]]; then
        log "cleanup: --keep-vm set, leaving $VM_NAME running"
    fi
    return "$rc"
}
trap cleanup EXIT

# Emit the single canonical RESULT line and exit. Also persists result.txt.
finish_pass() {
    {
        echo "RESULT: PASS"
        echo "vm=$VM_NAME"
        echo "type_filter=$TYPE_FILTER"
        echo "evidence_dir=$EVIDENCE_DIR"
        echo "elapsed_s=$(( $(date +%s) - START_EPOCH ))"
    } > "$RESULT_FILE"
    echo "RESULT: PASS"
    exit 0
}
finish_fail() {
    FINAL_REASON="$1"
    {
        echo "RESULT: FAIL: $FINAL_REASON"
        echo "vm=$VM_NAME"
        echo "type_filter=$TYPE_FILTER"
        echo "evidence_dir=$EVIDENCE_DIR"
        echo "elapsed_s=$(( $(date +%s) - START_EPOCH ))"
    } > "$RESULT_FILE"
    echo "RESULT: FAIL: $FINAL_REASON"
    exit 1
}

remaining_budget() {
    local now elapsed
    now="$(date +%s)"
    elapsed=$(( now - START_EPOCH ))
    echo $(( OVERALL_TIMEOUT - elapsed ))
}

# Capture a screenshot into the evidence dir (best-effort; never fatal).
capture_evidence() {
    local label="$1"
    if [[ -x "$CAPTURE" && -n "$VM_NAME" ]]; then
        log "capturing display ($label)"
        BREENIX_CAPTURE_RETRY_SCHEDULE="5 15 30" \
            "$CAPTURE" "$VM_NAME" "$EVIDENCE_DIR/display-$label.png" \
            >/dev/null 2>>"$EVIDENCE_DIR/capture.log" || \
            log "capture ($label) failed (non-fatal); see capture.log"
    fi
}

ms_to_s() { awk "BEGIN{printf \"%.3f\", ${1}/1000}"; }

# =============================================================================
# Preflight
# =============================================================================
[[ -x "$INJECT" ]]  || finish_fail "missing/non-executable inject helper at $INJECT"
[[ -x "$RUN_SH" ]]  || finish_fail "missing/non-executable run.sh at $RUN_SH"
command -v prlctl >/dev/null 2>&1 || finish_fail "prlctl not found on PATH"

# =============================================================================
# Locked-screen preflight + caffeinate keep-alive.
#
# Hard requirement: macOS must NOT be locked. When the console is locked,
# Parallels detaches the VM window and silently drops every injected
# keystroke (send-key-event returns rc=0 but the key never reaches the guest).
# This is NOT a TCC/permissions issue — injection goes through the virtual
# xHCI HID via prl_disp_service, not macOS CGEvent — so there is no
# non-interactive bypass. We therefore refuse to run on a locked Mac.
#
# The lock check must never crash the run on its own (missing python/Quartz,
# headless CI, etc.): if the check itself errors, we warn and proceed.
# =============================================================================
LOCK_CHECK_RC=2
if command -v python3 >/dev/null 2>&1; then
    python3 -c "import Quartz,sys; d=Quartz.CGSessionCopyCurrentDictionary(); sys.exit(0 if (d and d.get('CGSSessionScreenIsLocked')) else 1)" \
        >/dev/null 2>&1
    LOCK_CHECK_RC=$?
else
    log "WARNING: python3 not found; skipping macOS lock check (proceeding)"
fi

case "$LOCK_CHECK_RC" in
    0)
        echo "RESULT: FAIL: macOS screen is locked — Parallels drops injected keyboard input with no presented console. Unlock the Mac at the console, run 'caffeinate -d &', then retry."
        exit 1
        ;;
    1)
        log "lock check: macOS screen is unlocked"
        ;;
    *)
        log "WARNING: lock check failed to run (no Quartz / errored); proceeding without it"
        ;;
esac

# Keep the display awake for the duration of the (long) run so the screen
# never auto-locks/sleeps mid-injection. Best-effort: a missing caffeinate
# must not abort the run. Killed in cleanup.
if command -v caffeinate >/dev/null 2>&1; then
    caffeinate -d &
    CAFFEINATE_PID=$!
    log "started caffeinate -d (pid $CAFFEINATE_PID) to keep the display awake"
else
    log "WARNING: caffeinate not found; display may sleep/lock during a long run"
fi

# =============================================================================
# (a) Launch run.sh --parallels in the BACKGROUND. run.sh tails serial forever,
#     so it must be backgrounded; we kill it in cleanup.
# =============================================================================
RUN_ARGS=(--parallels)
[[ "$NO_BUILD" -eq 1 ]] && RUN_ARGS+=(--no-build)
log "launching: $RUN_SH ${RUN_ARGS[*]} (background)"
nohup "$RUN_SH" "${RUN_ARGS[@]}" >"$RUN_LOG" 2>&1 &
RUN_PID=$!
log "run.sh pid=$RUN_PID, log=$RUN_LOG"

# =============================================================================
# (b) Poll the serial log for the readiness marker, bounded by the overall timeout.
#     run.sh removes the serial log fresh on boot, so any match is from THIS boot.
# =============================================================================
log "waiting for readiness marker: $READY_MARKER"
READY=0
while :; do
    if [[ "$(remaining_budget)" -le "$WARMUP_SECS" ]]; then
        log "timed out waiting for readiness marker"
        break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
        finish_fail "run.sh exited before readiness (see $RUN_LOG)"
    fi
    if [[ -f "$SERIAL_LOG" ]] && grep -qF -- "$READY_MARKER" "$SERIAL_LOG"; then
        READY=1
        break
    fi
    sleep 3
done
[[ "$READY" -eq 1 ]] || finish_fail "readiness marker not seen within timeout ($READY_MARKER)"
log "readiness marker seen"

# =============================================================================
# (c) Resolve the running VM name (breenix-<epoch>) created by this run.sh.
# =============================================================================
VM_NAME="$(prlctl list -a 2>/dev/null | grep -o 'breenix-[0-9]\+' | tail -1 || true)"
[[ -n "$VM_NAME" ]] || finish_fail "could not resolve a running breenix-* VM via prlctl list -a"
log "resolved VM: $VM_NAME"
export VM="$VM_NAME"

# =============================================================================
# (d) VirGL warmup.
# =============================================================================
log "VirGL warmup: sleeping ${WARMUP_SECS}s"
sleep "$WARMUP_SECS"
capture_evidence "pre-trigger"

# =============================================================================
# (e) Record the serial line count, inject double-Super, then look for the
#     launcher marker in the tail since that line.
# =============================================================================
serial_lines() { [[ -f "$SERIAL_LOG" ]] && wc -l <"$SERIAL_LOG" | tr -d ' ' || echo 0; }

BASE_LINE="$(serial_lines)"
log "serial line baseline: $BASE_LINE"

log "injecting double-Super (prefix=$SUPER_PREFIX code=$SUPER_CODE gap=${INTER_TAP_MS}ms)"
"$INJECT" doubletap "$SUPER_CODE" "$INTER_TAP_MS" "$SUPER_PREFIX" \
    || finish_fail "inject doubletap failed (key injection error — see 'Host prerequisites & known limitations' in README)"

sleep "$(ms_to_s "$(awk "BEGIN{printf \"%d\", $POST_SUPER_WAIT*1000}")")"

# Grep only the lines appended since BASE_LINE.
tail_since() { [[ -f "$SERIAL_LOG" ]] && tail -n +"$(( BASE_LINE + 1 ))" "$SERIAL_LOG" || true; }

if tail_since | grep -qF -- "$LAUNCHER_MARKER"; then
    log "launcher opened (saw $LAUNCHER_MARKER)"
else
    capture_evidence "no-launcher"
    tail_since > "$SERIAL_EXCERPT" || true
    finish_fail "launcher did not open after double-Super (no '$LAUNCHER_MARKER')"
fi

# =============================================================================
# (f) Optionally type the filter, then Enter; look for the bterm oracles.
#     Terminal is APPS[0] so it stays selected whether or not we filter.
# =============================================================================
if [[ "$TYPE_FILTER" -eq 1 ]]; then
    log "typing filter text '$FILTER_TEXT'"
    "$INJECT" type "$FILTER_TEXT" \
        || finish_fail "inject type '$FILTER_TEXT' failed (key injection error)"
    sleep 0.5
fi

log "pressing Enter (code=$ENTER_CODE)"
"$INJECT" key "$ENTER_CODE" \
    || finish_fail "inject Enter failed (key injection error)"

sleep "$POST_ENTER_WAIT"
capture_evidence "post-enter"

# Save the full tail-since excerpt as evidence regardless of outcome.
tail_since > "$SERIAL_EXCERPT" || true

# =============================================================================
# (g)/(h) Honest oracle: PASS requires BOTH the bterm spawn line AND the bterm
#         config line. Launcher-only is an explicit FAIL.
# =============================================================================
SAW_BTERM_SPAWN=0
SAW_BTERM_CONFIG=0
tail_since | grep -qF -- "$BTERM_SPAWN_MARKER" && SAW_BTERM_SPAWN=1
tail_since | grep -qF -- "$BTERM_CONFIG_MARKER" && SAW_BTERM_CONFIG=1

if [[ "$SAW_BTERM_SPAWN" -eq 1 && "$SAW_BTERM_CONFIG" -eq 1 ]]; then
    log "terminal launched: saw '$BTERM_SPAWN_MARKER' AND '$BTERM_CONFIG_MARKER'"
    finish_pass
fi

if [[ "$SAW_BTERM_SPAWN" -eq 1 ]]; then
    finish_fail "bterm spawned but no '$BTERM_CONFIG_MARKER' (terminal did not initialize)"
elif [[ "$SAW_BTERM_CONFIG" -eq 1 ]]; then
    finish_fail "saw '$BTERM_CONFIG_MARKER' but no '$BTERM_SPAWN_MARKER' (inconsistent evidence)"
else
    finish_fail "launcher opened but terminal did not launch (no '$BTERM_SPAWN_MARKER' after Enter)"
fi
