#!/bin/bash
# Capture the guest display for a Parallels VM without requiring a visible
# desktop window. Prints the final PNG path on stdout; diagnostics go to stderr.
#
# Usage:
#   scripts/parallels/capture-display.sh <vm-name> [output.png]
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

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log "ERROR: required command not found: $1"
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

    mkdir -p "$BASELINE_DIR"
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

capture_prlctl() {
    local out="$1"
    prlctl capture "$VM_NAME" --file "$out" >/dev/null 2>&1
}

find_parallels_window_id() {
    python3 - "$VM_NAME" <<'PY'
import Quartz
import re
import sys

vm = sys.argv[1].lower()
breenix_hint = "breenix" in vm
windows = Quartz.CGWindowListCopyWindowInfo(
    Quartz.kCGWindowListOptionAll,
    Quartz.kCGNullWindowID,
)

candidates = []
for w in windows:
    owner = str(w.get("kCGWindowOwnerName", ""))
    if "Parallels" not in owner:
        continue
    bounds = w.get("kCGWindowBounds", {}) or {}
    width = int(bounds.get("Width", 0))
    height = int(bounds.get("Height", 0))
    if width < 300 or height < 200:
        continue
    title = str(w.get("kCGWindowName", "") or "")
    layer = int(w.get("kCGWindowLayer", 0))
    if layer != 0:
        continue
    score = width * height
    lower_title = title.lower()
    if vm and vm in lower_title:
        score += 10_000_000
    elif breenix_hint and "breenix" in lower_title:
        score += 5_000_000
    candidates.append((score, int(w.get("kCGWindowNumber", 0)), width, height, title))

if not candidates:
    sys.exit(1)

candidates.sort(reverse=True)
print(candidates[0][1])
PY
}

capture_window() {
    local out="$1"
    local window_id
    window_id="$(find_parallels_window_id || true)"
    if [ -z "$window_id" ]; then
        return 1
    fi
    screencapture -x -o -l"$window_id" "$out" >/dev/null 2>&1
}

require_cmd prlctl
require_cmd python3

mkdir -p "$(dirname "$OUTPUT")"

attempt=0
last_stats=""
for delay in $RETRY_SCHEDULE; do
    attempt=$((attempt + 1))
    log "Attempt $attempt: waiting ${delay}s before capture for VM '$VM_NAME'"
    sleep "$delay"

    candidate="$TMP_DIR/display-attempt-${attempt}.png"
    method="prlctl"
    if ! capture_prlctl "$candidate"; then
        log "Attempt $attempt: prlctl capture failed, trying Core Graphics window capture"
        method="screencapture"
        if ! capture_window "$candidate"; then
            log "Attempt $attempt: screencapture fallback failed"
            continue
        fi
    fi

    stats="$(image_probe "$candidate")"
    last_stats="$stats"
    if [ "$(json_bool "$stats" ok)" != "true" ]; then
        log "Attempt $attempt: image probe failed: $(json_value "$stats" error)"
        continue
    fi

    width="$(json_value "$stats" width)"
    height="$(json_value "$stats" height)"
    dominant="$(json_value "$stats" dominant_rgb)"
    distinct="$(json_value "$stats" distinct_colors)"
    log "Attempt $attempt: method=$method size=${width}x${height} dominant=${dominant} distinct=${distinct}"

    if [ "$(json_bool "$stats" black_delay)" = "true" ]; then
        log "Attempt $attempt: capture is black; treating as Parallels VirGL warmup delay"
        continue
    fi

    cp "$candidate" "$OUTPUT"
    write_baseline_and_stats "$OUTPUT" "$stats"
    printf '%s\n' "$OUTPUT"
    exit 0
done

if [ -n "$last_stats" ]; then
    log "Last image stats: $last_stats"
fi
log "ERROR: failed to capture a non-black Parallels display for VM '$VM_NAME'"
exit 1
