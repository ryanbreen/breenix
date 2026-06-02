#!/usr/bin/env bash
#
# launcher-smoke.sh — ONE full launcher->terminal smoke run on a fresh Parallels VM.
#
# Flow under test:
#   boot (run.sh --parallels) -> BWM ready -> double-tap SUPER opens the launcher
#   (/bin/blauncher, pre-selecting APPS[0] = "Terminal") -> Enter launches the
#   terminal (/bin/bterm). PASS requires REAL serial evidence that bterm started
#   (its own '[bterm] config:' line) AND became functional (spawned its child
#   shell, '[bterm] spawned child pid=') — never "launcher opened" alone.
#   NB: blauncher launches bterm via fork+execv, which does NOT emit the kernel's
#   "[spawn] path='...'" line — so we validate bterm's OWN startup logs, which are
#   stronger proof (the binary actually ran and initialized) than a spawn record.
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
#
# The launcher opens on a double-tap of the SUPER modifier. Breenix's USB-HID
# layer (kernel/src/drivers/usb/hid.rs) maps the Left-CTRL bit to SUPER, so
# injecting a plain Left-Ctrl tap registers as Super in the guest — this is
# literally why the operator calls it the "double control key", and it is the
# exact key Parallels delivers.
#
# We deliberately do NOT use the 0xE0 0x5B (left-GUI) extended scancode: Parallels
# Desktop 26.3.3 rejects a bare `--scancode 91` ("Invalid scan code sequence: 5B")
# and offers no way to send the extended pair as separate --scancode calls. Plain
# (non-extended) scancodes like Left-Ctrl (29) are accepted and map to SUPER.
#
# A "tap" = press/release of the code. A "double-tap" = two taps within 400 ms
# (INTER_TAP_MS gap + ~40 ms hold). To change the trigger, edit THESE values.
# =============================================================================
SUPER_PREFIX=          # none — Left-Ctrl is a basic, non-extended scancode
SUPER_CODE=29          # 0x1D Left-Ctrl; Breenix maps the Ctrl HID bit to SUPER
INTER_TAP_MS=150       # gap between the two taps (must be < 400 ms)
ENTER_CODE=28          # Enter / Return

# =============================================================================
# Other tunables
# =============================================================================
READY_MARKER='[bwm] hotkeys: using built-in defaults for early boot'
LAUNCHER_MARKER="[spawn] path='/bin/blauncher'"
BTERM_CONFIG_MARKER='[bterm] config:'            # bterm started + read its config
BTERM_SHELL_MARKER='[bterm] spawned child pid='  # bterm launched its child shell
WARMUP_SECS=60         # VirGL warmup after readiness marker
POST_SUPER_WAIT=1.5    # settle after double-Super before grepping for launcher
POST_ENTER_WAIT=3      # settle after Enter before grepping for bterm
FILTER_TEXT='term'     # typed when --type-filter is set (Terminal stays index 0)

# =============================================================================
# Argument parsing
# =============================================================================
NO_BUILD=0
KEEP_VM=0
OVERALL_TIMEOUT=1200
TYPE_FILTER=0
NO_BACKGROUND=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build)      NO_BUILD=1 ;;
        --keep-vm)       KEEP_VM=1 ;;
        --type-filter)   TYPE_FILTER=1 ;;
        --no-background) NO_BACKGROUND=1 ;;
        --timeout)       OVERALL_TIMEOUT="${2:?--timeout needs a value}"; shift ;;
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
VM_PROC_PID=""
# Inode of any pre-existing (stale, prior-run) serial log, captured before we
# launch run.sh. run.sh `rm -f`s the log and recreates it fresh on boot, which
# changes the inode; we refuse to trust any marker until the inode differs (or
# the file is gone), so a leftover prior-run marker can never be mis-read as
# readiness for THIS boot.
STALE_SERIAL_INODE=""

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

# CPU-relief strategy (the operator uses this Mac during runs): keep the VM at
# LOW priority (renice 20) through the long boot/warmup/idle phases so it yields
# CPU to the operator's foreground apps under contention — but RESTORE it to
# normal priority for the brief, timing-sensitive double-tap injection window.
#
# We use renice ONLY (no `taskpolicy -b`): banishing the VM to efficiency cores
# starved the guest so hard it could not consume the two taps inside bwm's 400ms
# double-tap window (observed 1876ms => launcher never opened). renice keeps the
# VM on the performance cores at low priority (polite under contention) and is
# cleanly reversible, so the injection window stays responsive. No sudo needed.
background_vm_proc() {
    [[ "$NO_BACKGROUND" -eq 1 ]] && return 0
    local pid
    pid="$(pgrep -f 'prl_vm_app.*--vm-name breenix-' 2>/dev/null | head -1 || true)"
    [[ -z "$pid" ]] && return 1
    VM_PROC_PID="$pid"
    renice 20 -p "$pid" >/dev/null 2>&1 || true
    log "lowered Breenix VM pid=$pid to nice 20 — yields CPU to your foreground apps under contention (stays on perf cores so injection stays responsive)"
    return 0
}

# Restore the VM to normal priority for the timing-sensitive injection window.
foreground_vm_proc() {
    [[ "$NO_BACKGROUND" -eq 1 ]] && return 0
    [[ -z "$VM_PROC_PID" ]] && return 0
    renice 0 -p "$VM_PROC_PID" >/dev/null 2>&1 || true
    log "restored Breenix VM pid=$VM_PROC_PID to nice 0 for the double-tap injection window"
}

ms_to_s() { awk "BEGIN{printf \"%.3f\", ${1}/1000}"; }

# Current inode of the serial log, or empty if it does not exist.
serial_inode() { [[ -e "$SERIAL_LOG" ]] && stat -f '%i' "$SERIAL_LOG" 2>/dev/null || true; }

# True only once the serial log is the FRESH one run.sh created for this boot:
# either the stale file is gone, or its inode changed since we captured it.
serial_is_fresh() {
    local cur
    cur="$(serial_inode)"
    [[ -z "$cur" ]] && return 1                 # not (re)created yet
    [[ -z "$STALE_SERIAL_INODE" ]] && return 0  # no stale file existed at all
    [[ "$cur" != "$STALE_SERIAL_INODE" ]]
}

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
    # Run the probe as an if-condition: it exits 1 when UNLOCKED (the normal,
    # required state), and a bare non-zero command would trip `set -e` before we
    # could read $?. As a condition, `set -e` is exempt and the else-branch sees
    # the real exit code. 0 = LOCKED, 1 = UNLOCKED, other = probe errored.
    if python3 -c "import Quartz,sys; d=Quartz.CGSessionCopyCurrentDictionary(); sys.exit(0 if (d and d.get('CGSSessionScreenIsLocked')) else 1)" >/dev/null 2>&1; then
        LOCK_CHECK_RC=0
    else
        LOCK_CHECK_RC=$?
    fi
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

# Serial-only guard: these runs MUST be serial. run.sh kills any existing breenix
# VM before creating its own, so an overlapping run would destroy an in-flight VM
# (and two VMs would fight the dispatcher). Refuse to start if one is already up.
EXISTING_VM="$(prlctl list 2>/dev/null | awk '/breenix-/{print $NF}' | head -1 || true)"
if [[ -n "$EXISTING_VM" ]]; then
    echo "RESULT: FAIL: a Breenix VM ($EXISTING_VM) is already running — launcher-smoke runs must be SERIAL (one VM at a time). Stop it (prlctl stop $EXISTING_VM --kill) and retry."
    exit 1
fi

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
# Snapshot the inode of any leftover serial log from a previous run BEFORE we
# launch run.sh, so the readiness poll can tell "fresh log from this boot" apart
# from "stale log that already contains a prior run's readiness marker".
STALE_SERIAL_INODE="$(serial_inode)"
if [[ -n "$STALE_SERIAL_INODE" ]]; then
    log "stale serial log present (inode $STALE_SERIAL_INODE); will wait for run.sh to recreate it"
fi

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
BG_DONE=0
while :; do
    if [[ "$(remaining_budget)" -le "$WARMUP_SECS" ]]; then
        log "timed out waiting for readiness marker"
        break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
        finish_fail "run.sh exited before readiness (see $RUN_LOG)"
    fi
    # As soon as the VM process exists, drop it to background priority so it does
    # not fight the operator's foreground apps for CPU (injection stays foreground).
    if [[ "$BG_DONE" -eq 0 ]] && background_vm_proc; then BG_DONE=1; fi
    # Only trust the marker once the serial log is the fresh one run.sh created
    # for THIS boot — never a leftover prior-run log that may already contain it.
    if serial_is_fresh && grep -qF -- "$READY_MARKER" "$SERIAL_LOG"; then
        READY=1
        break
    fi
    sleep 3
done
[[ "$READY" -eq 1 ]] || finish_fail "readiness marker not seen within timeout ($READY_MARKER)"
log "readiness marker seen"

# =============================================================================
# (c) Resolve the VM name (breenix-<epoch>) created by THIS run.sh.
#
# Authoritative source: run.sh prints `VM:     breenix-<epoch>` to its stdout
# (captured in RUN_LOG) AFTER it has created and started that exact VM. Reading
# it from RUN_LOG is immune to leftover/stuck breenix-* VMs that run.sh failed
# to delete. Fall back to the prlctl-list heuristic only if RUN_LOG has no such
# line (e.g. run.sh output format changed).
# =============================================================================
VM_NAME="$(grep -oE 'breenix-[0-9]+' "$RUN_LOG" 2>/dev/null | tail -1 || true)"
if [[ -n "$VM_NAME" ]]; then
    log "resolved VM from run.sh output: $VM_NAME"
else
    VM_NAME="$(prlctl list -a 2>/dev/null | grep -o 'breenix-[0-9]\+' | tail -1 || true)"
    [[ -n "$VM_NAME" ]] || finish_fail "could not resolve a breenix-* VM (no name in $RUN_LOG, none via prlctl list -a)"
    log "resolved VM via prlctl fallback: $VM_NAME"
fi
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

# Restore full VM priority for the timing-sensitive injection + launch window
# (it ran low-priority through the long boot/warmup for CPU relief).
foreground_vm_proc
BASE_LINE="$(serial_lines)"
log "serial line baseline: $BASE_LINE"

log "injecting double-Super (prefix=$SUPER_PREFIX code=$SUPER_CODE gap=${INTER_TAP_MS}ms)"
INJ_T0="$(python3 -c 'import time;print(int(time.time()*1000))' 2>/dev/null || echo 0)"
"$INJECT" doubletap "$SUPER_CODE" "$INTER_TAP_MS" "$SUPER_PREFIX" \
    || finish_fail "inject doubletap failed (key injection error — see 'Host prerequisites & known limitations' in README)"
INJ_T1="$(python3 -c 'import time;print(int(time.time()*1000))' 2>/dev/null || echo 0)"
INJ_MS=$(( INJ_T1 - INJ_T0 ))
# The double-tap is sent as a SINGLE `prlctl send-key-event -j` batch, so the
# inter-tap spacing (INTER_TAP_MS) is applied by the dispatcher precisely and is
# INDEPENDENT of this wall-time. INJ_MS is just prlctl's one-call overhead — it
# can be large under host load WITHOUT affecting whether the taps land in bwm's
# 400ms window. (Pre-batching, 4 separate prlctl spawns made INJ_MS == the tap
# spacing and blew the window on a loaded host; batching fixed that.)
log "double-tap injected as one -j batch; prlctl wall-time ${INJ_MS}ms (inter-tap spacing dispatcher-controlled at ${INTER_TAP_MS}ms, load-independent)"

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
# (g)/(h) Honest oracle: PASS requires BOTH bterm's own startup config line AND
#         its child-shell spawn line — i.e. the terminal launched AND loaded a
#         working shell. Launcher-only, or a half-initialized bterm, is a FAIL.
# =============================================================================
SAW_BTERM_CONFIG=0
SAW_BTERM_SHELL=0
tail_since | grep -qF -- "$BTERM_CONFIG_MARKER" && SAW_BTERM_CONFIG=1
tail_since | grep -qF -- "$BTERM_SHELL_MARKER"  && SAW_BTERM_SHELL=1

if [[ "$SAW_BTERM_CONFIG" -eq 1 && "$SAW_BTERM_SHELL" -eq 1 ]]; then
    log "terminal launched + loaded: saw '$BTERM_CONFIG_MARKER' AND '$BTERM_SHELL_MARKER'"
    finish_pass
fi

if [[ "$SAW_BTERM_CONFIG" -eq 1 ]]; then
    finish_fail "bterm started ('$BTERM_CONFIG_MARKER') but did not spawn its shell ('$BTERM_SHELL_MARKER') — terminal did not finish loading"
elif [[ "$SAW_BTERM_SHELL" -eq 1 ]]; then
    finish_fail "saw '$BTERM_SHELL_MARKER' but no '$BTERM_CONFIG_MARKER' (inconsistent evidence)"
else
    finish_fail "launcher opened but terminal did not launch (no '$BTERM_CONFIG_MARKER' after Enter)"
fi
