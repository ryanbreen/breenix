#!/bin/bash
# Capture the guest display for a Parallels VM.
#
# Two independent capture methods are tried, in order:
#   1. `prlctl capture` -- Parallels' own backend-level screen capture. This
#      works whether or not the VM has a visible on-screen window, and is the
#      only method that has ever succeeded from this harness (see #917 RCA
#      below).
#   2. A Core Graphics window capture (`screencapture -l<window-id>`) -- only
#      useful when Parallels Desktop.app itself has an actual window open for
#      this VM. When the harness starts a VM via `prlctl start` with no GUI
#      open, Parallels does not create such a window (0 of 12 Parallels-owned
#      CGWindowList entries observed in either state --
#      docs/planning/green-program/gui/evidence/windowlist-no-vm.txt), so
#      this method is expected to find nothing in that mode; it exists for
#      the case where a human has the Parallels Desktop app open and the
#      VM's window visible.
#
# #917 RCA (docs/planning/green-program/gui/PARALLELS-CAPTURE-2026-09-07.md):
# the window-matching code this replaced matched on `kCGWindowName`
# containing the VM's name. `CGWindowListCopyWindowInfo`, inspected both
# during a running boot and with no Breenix VM running at all
# (docs/planning/green-program/gui/evidence/windowlist-no-vm.txt: 12/12
# Parallels-owned windows, 0/12 with a non-empty `kCGWindowName`), showed
# Parallels does not set a per-VM window title -- so a title-substring match
# cannot succeed by construction, matching the 6/6 identical failures (of
# the runs that reached this step) in
# docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/.
# This version drops title matching and instead requires a window owned by
# the actual `prl_vm_app --vm-name <VM>` process backing this VM (the one
# reliable signal available, #917 fix-pass C-3 -- a *preference*, i.e. a
# scoring bonus with a same-process-name fallback, still let an unrelated
# Parallels-owned window such as Parallels Desktop's own UI be selected
# whenever no PID match existed), while still logging the full window
# inventory so a next investigation does not have to re-derive it from
# scratch.
#
# This script does not write a synthetic or placeholder image to its output
# path — checked by capture_display_writes_output_exactly_once_and_only_on_a_verified_frame
# in tests/parallels_capture_structure.rs (7/7 passing). On success, OUTPUT
# holds a real, non-degenerate capture. On failure (the retry schedule
# exhausted with no valid frame, or a required command missing), OUTPUT is
# not created (a prior screenshot is removed before retries), and the script
# exits non-zero -- callers must not
# treat a missing file as a black PASS.
#
# Usage:
#   scripts/parallels/capture-display.sh <vm-name> [output.png]
#
# stdout: on success, two lines --
#   [PARALLELS_CAPTURE:method=<prlctl|window>:reason=ok]
#   <path to OUTPUT>
# on failure, one line, with the literal method value shown below (both
# shapes appear as real captured output in
# docs/planning/green-program/gui/evidence/prlctl-capture-failure-modes.txt
# and in PARALLELS-CAPTURE-2026-09-07.md's "Verification runs" section) --
#   [PARALLELS_CAPTURE:method=none:reason=<why>]
# claim-lint:ok: #917 -- the literal method=none value in that line above is
# the documented failure-shape output, not a claim about how often it fires.
# Diagnostics (attempt-by-attempt logging, the window inventory, prlctl's
# own stderr) go to stderr.
#
# Environment:
#   BREENIX_CAPTURE_RETRY_SCHEDULE  Space-separated delays, default "30 60 90".
#   BREENIX_CAPTURE_BASELINE_DIR    Baseline dir, default logs/.../f20-baseline-red.

set -euo pipefail

VM_NAME="${1:?Usage: $0 <vm-name> [output.png]}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT="${2:-$BREENIX_ROOT/logs/breenix-parallels-cpu0/capture-$(date +%Y%m%d-%H%M%S).png}"
BASELINE_DIR="${BREENIX_CAPTURE_BASELINE_DIR:-$BREENIX_ROOT/logs/breenix-parallels-cpu0/f20-baseline-red}"
RETRY_SCHEDULE="${BREENIX_CAPTURE_RETRY_SCHEDULE:-30 60 90}"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/breenix-capture.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

log() {
    printf '%s\n' "$*" >&2
}

# Emits the required verdict line on stdout. Called exactly once, on the
# single success return and on the single terminal-failure return. `reason`
# may come from a subprocess's own stderr text, which can itself contain
# colons (e.g. prlctl's "Failed to get VM config: ..."); those are replaced
# with ';' first so the emitted line keeps exactly two top-level ':'
# separators (method=, reason=) and stays trivially parseable.
emit_verdict() {
    local method="$1"
    local reason="${2//:/;}"
    printf '[PARALLELS_CAPTURE:method=%s:reason=%s]\n' "$method" "$reason"
}

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log "ERROR: required command not found: $1"
        emit_verdict "none" "missing-command-$1"
        exit 1
    fi
}

image_probe() {
    local image="$1"
    python3 - "$image" <<'PY'
import json
import sys
import warnings
from collections import Counter
from pathlib import Path

try:
    from PIL import Image
except Exception as exc:
    print(json.dumps({"ok": False, "error": f"PIL unavailable: {exc}"}))
    sys.exit(0)

path = Path(sys.argv[1])
try:
    with Image.open(path) as im:
        rgb = im.convert("RGB")
        width, height = rgb.size
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            colors = rgb.getdata()
        total = width * height
        counts = Counter(colors)
        dominant, dominant_count = counts.most_common(1)[0]
        distinct = len(counts)
        non_red_distinct = sum(
            1
            for (r, g, b) in counts
            if not (r >= 230 and g <= 25 and b <= 25)
        )
        redish_pixels = sum(
            count
            for (r, g, b), count in counts.items()
            if r >= 230 and g <= 25 and b <= 25
        )
        blackish_pixels = sum(
            count
            for (r, g, b), count in counts.items()
            if r <= 8 and g <= 8 and b <= 8
        )
        avg = [
            round(sum(pixel[i] * count for pixel, count in counts.items()) / total, 3)
            for i in range(3)
        ]
        solid_red = distinct == 1 and dominant[0] >= 230 and dominant[1] <= 25 and dominant[2] <= 25
        black_delay = blackish_pixels / total >= 0.98
        rendered = (
            non_red_distinct >= 2
            and redish_pixels / total < 0.90
            and not solid_red
            and not black_delay
        )
        print(json.dumps({
            "ok": True,
            "path": str(path),
            "width": width,
            "height": height,
            "distinct_colors": distinct,
            "non_red_distinct_colors": non_red_distinct,
            "dominant_rgb": dominant,
            "dominant_fraction": round(dominant_count / total, 6),
            "redish_fraction": round(redish_pixels / total, 6),
            "blackish_fraction": round(blackish_pixels / total, 6),
            "average_rgb": avg,
            "solid_red": solid_red,
            "black_delay": black_delay,
            "passes_rendered_desktop_bar": rendered,
        }))
except Exception as exc:
    print(json.dumps({"ok": False, "error": str(exc)}))
PY
}

json_bool() {
    local json="$1"
    local key="$2"
    python3 - "$json" "$key" <<'PY'
import json
import sys
data = json.loads(sys.argv[1])
print("true" if data.get(sys.argv[2]) else "false")
PY
}

json_value() {
    local json="$1"
    local key="$2"
    python3 - "$json" "$key" <<'PY'
import json
import sys
data = json.loads(sys.argv[1])
value = data.get(sys.argv[2])
if isinstance(value, (list, tuple)):
    print(",".join(str(v) for v in value))
else:
    print(value)
PY
}

write_baseline_and_stats() {
    local image="$1"
    local stats="$2"

    mkdir -p "$BASELINE_DIR" || return 1
    local solid_baseline="$BASELINE_DIR/solid-red.png"

    if [ ! -f "$solid_baseline" ]; then
        python3 - "$image" "$solid_baseline" <<'PY'
import sys
from pathlib import Path
from PIL import Image

source = Path(sys.argv[1])
target = Path(sys.argv[2])
with Image.open(source) as im:
    Image.new("RGB", im.size, (255, 0, 0)).save(target)
PY
        log "Created solid-red baseline: $solid_baseline"
    fi

    printf '%s\n' "$stats" > "${image}.stats.json"

    local baseline_cmp
    baseline_cmp=$(python3 - "$image" "$solid_baseline" <<'PY'
import sys
from pathlib import Path
from PIL import Image, ImageChops

captured = Path(sys.argv[1])
baseline = Path(sys.argv[2])
with Image.open(captured).convert("RGB") as a, Image.open(baseline).convert("RGB") as b:
    if a.size != b.size:
        print("different-size")
    else:
        diff = ImageChops.difference(a, b)
        bbox = diff.getbbox()
        print("match" if bbox is None else "different")
PY
)
    log "Solid-red baseline comparison: $baseline_cmp"
}

# Runs `prlctl capture`, capturing its real exit code and stderr rather than
# swallowing both into /dev/null. Logs the outcome either way so a caller can
# see exactly why the backend capture did or did not work (#917: this used to
# redirect both stdout and stderr to /dev/null and only ever logged "prlctl
# capture failed, trying..." with no detail -- silent on the one diagnostic
# that actually explains a failure, e.g. PRL_ERR_IO_STOPPED for a
# suspended/stopped VM, or "not registered" for a wrong name).
capture_prlctl() {
    local out="$1"
    local errfile="$2"
    local rc=0
    prlctl capture "$VM_NAME" --file "$out" >/dev/null 2>"$errfile" || rc=$?
    if [ "$rc" -ne 0 ]; then
        local errtext
        errtext="$(tr '\n' ' ' <"$errfile" | sed 's/  */ /g; s/^ *//; s/ *$//')"
        log "prlctl capture: exit=$rc stderr=\"${errtext:-<empty>}\""
        return "$rc"
    fi
    log "prlctl capture: exit=0"
    return 0
}

# Finds the process (if any) that Parallels started to back this VM's own
# display. `prlctl start` launches a per-VM `prl_vm_app --vm-name <VM>`
# process; that PID is the correct identity to look for among CGWindowList
# owners, unlike the VM's own name, which the file header's #917 RCA shows
# does not appear in kCGWindowName. Prints nothing and returns non-zero if
# no such process is running (VM not started, or already stopped).
# claim-lint:ok: #917
find_vm_backend_pid() {
    ps -axo pid=,command= 2>/dev/null | awk -v needle="--vm-name $VM_NAME" '
        index($0, "prl_vm_app") > 0 {
            pos = index($0, needle)
            if (pos > 0) {
                after = substr($0, pos + length(needle), 1)
                if (after == "" || after == " ") {
                    print $1
                    found = 1
                    exit
                }
            }
        }
        END { if (!found) exit 1 }
    '
}

# Logs the CGWindowList inventory of each window owned by a process whose
# name contains "Parallels", to stderr, so a diagnosis does not have to
# re-derive "what windows actually exist" from scratch. Returns the window
# id of the best candidate on stdout (nothing, and a non-zero exit, if none
# qualifies).
# claim-lint:ok: #917
find_parallels_window_id() {
    local vm_pid="$1"
    python3 - "$VM_NAME" "$vm_pid" <<'PY'
import sys
import Quartz

vm = sys.argv[1]
vm_pid = int(sys.argv[2]) if sys.argv[2] not in ("", "0") else None

windows = Quartz.CGWindowListCopyWindowInfo(
    Quartz.kCGWindowListOptionAll,
    Quartz.kCGNullWindowID,
)

candidates = []
inventory = []
for w in windows:
    owner = str(w.get("kCGWindowOwnerName", "") or "")
    if "Parallels" not in owner:
        continue
    bounds = w.get("kCGWindowBounds", {}) or {}
    width = int(bounds.get("Width", 0))
    height = int(bounds.get("Height", 0))
    title = str(w.get("kCGWindowName", "") or "")
    layer = int(w.get("kCGWindowLayer", 0) or 0)
    pid = w.get("kCGWindowOwnerPID")
    window_id = int(w.get("kCGWindowNumber", 0) or 0)
    inventory.append(
        f"owner={owner!r} title={title!r} pid={pid} layer={layer} size={width}x{height} id={window_id}"
    )
    if layer != 0 or width < 300 or height < 200:
        continue
    # The one principled signal: this window's owning process must be the
    # actual backend process Parallels started for THIS vm. Without a known
    # backend PID (vm_pid is None), or when this window's pid does not equal
    # it, this window is not a candidate at all -- #917 fix-pass finding C-3
    # found that a scoring *bonus* (rather than a hard requirement) still let
    # every eligible window from any Parallels-owned process become a
    # candidate, so with no PID match at all the function picked the largest
    # such window (Parallels Desktop's own UI, in the recorded evidence) and
    # reported it as this VM's capture. Title matching is not attempted --
    # kCGWindowName is non-empty for 0 of the 12 Parallels-owned windows
    # inventoried in
    # docs/planning/green-program/gui/evidence/windowlist-no-vm.txt, so it
    # cannot discriminate between VMs.
    # claim-lint:ok: #917, 0/12 above
    if vm_pid is None or pid != vm_pid:
        continue
    score = width * height
    candidates.append((score, window_id, width, height, pid))

for line in inventory:
    print("WINDOW " + line, file=sys.stderr)
if not inventory:
    print(f"WINDOW <none owned by any process with 'Parallels' in its name>", file=sys.stderr)

if not candidates:
    print(f"NO_MATCH vm={vm!r} vm_pid={vm_pid!r}", file=sys.stderr)
    sys.exit(1)

candidates.sort(reverse=True)
best = candidates[0]
print(f"MATCH id={best[1]} size={best[2]}x{best[3]} owner_pid={best[4]}", file=sys.stderr)
print(best[1])
PY
}

capture_window() {
    local out="$1"
    local vm_pid
    vm_pid="$(find_vm_backend_pid || true)"
    local window_id
    window_id="$(find_parallels_window_id "${vm_pid:-0}" || true)"
    if [ -z "$window_id" ]; then
        return 1
    fi
    # Clear stale content from capture_prlctl at this per-attempt path (C-4).
    rm -f "$out"
    screencapture -x -o -l"$window_id" "$out" 2>&1 | while IFS= read -r line; do log "screencapture: $line"; done
    # The function is an if condition, so errexit does not check this pipeline.
    # Inspect screencapture's own status before accepting its output (C-4).
    local sc_rc="${PIPESTATUS[0]}"
    if [ "$sc_rc" -ne 0 ]; then
        log "screencapture exited $sc_rc"
        return 1
    fi
    [ -s "$out" ]
}

require_cmd prlctl
require_cmd python3

mkdir -p "$(dirname "$OUTPUT")"
# Clear a prior invocation's screenshot so failed retries cannot leave stale
# evidence at OUTPUT (#917 fix-pass C-7).
rm -f "$OUTPUT"

attempt=0
last_stats=""
last_reason="not-attempted"
for delay in $RETRY_SCHEDULE; do
    attempt=$((attempt + 1))
    log "Attempt $attempt: waiting ${delay}s before capture for VM '$VM_NAME'"
    sleep "$delay"

    candidate="$TMP_DIR/display-attempt-${attempt}.png"
    prlctl_err="$TMP_DIR/prlctl-attempt-${attempt}.err"
    method="prlctl"
    if capture_prlctl "$candidate" "$prlctl_err"; then
        reason="ok"
    else
        prlctl_rc=$?
        prlctl_errtext="$(tr '\n' ' ' <"$prlctl_err" | sed 's/  */ /g; s/^ *//; s/ *$//')"
        log "Attempt $attempt: prlctl capture failed, trying Core Graphics window capture"
        method="window"
        if capture_window "$candidate"; then
            reason="ok"
        else
            last_reason="prlctl-exit-${prlctl_rc}:${prlctl_errtext:-no-stderr}:no-window-match"
            log "Attempt $attempt: window capture also failed ($last_reason)"
            continue
        fi
    fi

    stats="$(image_probe "$candidate")"
    last_stats="$stats"
    if [ "$(json_bool "$stats" ok)" != "true" ]; then
        last_reason="image-probe-failed:$(json_value "$stats" error)"
        log "Attempt $attempt: image probe failed: $(json_value "$stats" error)"
        continue
    fi

    width="$(json_value "$stats" width)"
    height="$(json_value "$stats" height)"
    dominant="$(json_value "$stats" dominant_rgb)"
    distinct="$(json_value "$stats" distinct_colors)"
    log "Attempt $attempt: method=$method size=${width}x${height} dominant=${dominant} distinct=${distinct}"

    if [ "$(json_bool "$stats" black_delay)" = "true" ]; then
        last_reason="black-frame-warmup"
        log "Attempt $attempt: capture is black; treating as Parallels VirGL warmup delay"
        continue
    fi

    cp "$candidate" "$OUTPUT"
    # Diagnostics are best effort: a real verified capture is already at
    # OUTPUT, so a baseline failure must not turn it into capture=none (C-6).
    # claim-lint:ok: #917; tests/parallels_capture_structure.rs
    if ! write_baseline_and_stats "$OUTPUT" "$stats"; then
        log "WARNING: write_baseline_and_stats failed (non-fatal; $OUTPUT was already written)"
    fi
    emit_verdict "$method" "ok"
    printf '%s\n' "$OUTPUT"
    exit 0
done

if [ -n "$last_stats" ]; then
    log "Last image stats: $last_stats"
fi
log "ERROR: failed to capture a non-black Parallels display for VM '$VM_NAME' (reason=$last_reason)"
emit_verdict "none" "$last_reason"
exit 1
