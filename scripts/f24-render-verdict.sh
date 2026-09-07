#!/usr/bin/env bash
# Strict bwm render verdict for F24 Parallels captures.
#
# Usage:
#   scripts/f24-render-verdict.sh <png_path>
#
# Exit 0: rendered desktop content includes a spatially coherent UI region
#         (VERDICT=PASS).
# Exit 1: a real capture was scored but did not pass (VERDICT=FAIL).
# Exit 2: no real capture exists to score -- <png_path> is missing, it is
#         the known-degenerate solid-black frame that
#         scripts/parallels/capture-display.sh's own black-warmup retry is
#         meant to filter before handing a caller a PNG, or the file exists
#         but cannot be decoded as an image at all (a truncated/corrupt
#         capture) -- #917 fix-pass finding C-9: this third case used to
#         raise an uncaught PIL exception, exit 1, and print no VERDICT=
#         line at all, indistinguishable from an ordinary FAIL to a
#         $?-only caller (see the three CAPTURE_MISSING tests in
#         tests/parallels_capture_structure.rs, #917:
#         docs/planning/green-program/gui/PARALLELS-CAPTURE-2026-09-07.md).
#         A caller must not treat exit 2 the same as VERDICT=FAIL -- FAIL
#         means "captured the desktop and it looks wrong", CAPTURE_MISSING
#         means "did not capture the desktop".

set -euo pipefail

PNG="${1:?png path required}"

if [ ! -f "$PNG" ]; then
    echo "VERDICT=CAPTURE_MISSING reason=png-absent path=$PNG"
    exit 2
fi

python3 - "$PNG" <<'PY'
import sys
import warnings
from collections import Counter
from math import sqrt
from PIL import Image

path = sys.argv[1]
try:
    img = Image.open(path).convert("RGB")
except Exception as exc:
    print(f"VERDICT=CAPTURE_MISSING reason=unreadable-image error={exc} path={path}")
    sys.exit(2)
w, h = img.size

# A capture-layer failure -- capture-display.sh's own black-warmup retry
# (scripts/parallels/capture-display.sh) is meant to filter this shape out
# before it ever reaches a caller -- not a render-layer one: a solid single
# color across the whole frame that is at-or-near black. Checked by
# dominant-color count, not a byte-identical file hash (see
# f24_render_verdict_rejects_solid_black_capture in
# tests/parallels_capture_structure.rs, which synthesizes a fresh black PNG
# each run rather than depending on one fixed file), so it still catches
# the failure mode if the PNG encoder's byte output changes. Checked over the
# full image, unlike the render verdict below (which crops the Parallels
# toolbar first), because a rendered desktop's toolbar-region pixels differ
# from its background even when the content below it is flat.
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    full_pixels = list(img.getdata())
full_colors = Counter(full_pixels)
if len(full_colors) == 1:
    (r, g, b) = next(iter(full_colors))
    if r <= 8 and g <= 8 and b <= 8:
        print(f"VERDICT=CAPTURE_MISSING reason=solid-black-capture color=({r},{g},{b}) path={path}")
        sys.exit(2)

# Crop top 40px to remove the Parallels toolbar from the verdict.
content = img.crop((0, 40, w, h))
cw, ch = content.size
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    pixels = list(content.getdata())
total = len(pixels)

distinct = len(set(pixels))
colors = Counter(pixels)
dominant, dom_count = colors.most_common(1)[0]
dom_frac = dom_count / total

def dist(a, b):
    return sqrt(sum((a[i] - b[i]) ** 2 for i in range(3)))

def bucket(p):
    # Quantize to 32 levels per channel, same bucket width as F23.
    return (p[0] >> 3, p[1] >> 3, p[2] >> 3)

bucket_counts = Counter(bucket(p) for p in pixels)
big_buckets = sum(1 for v in bucket_counts.values() if v / total > 0.01)

blue_baseline = dist(dominant, (100, 149, 237)) < 15
red_baseline = dist(dominant, (255, 0, 0)) < 15

def coherent_region(candidate_bucket):
    xs = []
    ys = []
    count = 0
    for y in range(ch):
        row_start = y * cw
        for x in range(cw):
            if bucket(pixels[row_start + x]) == candidate_bucket:
                xs.append(x)
                ys.append(y)
                count += 1
    if count == 0:
        return None

    x0, x1 = min(xs), max(xs)
    y0, y1 = min(ys), max(ys)
    bbox_w = x1 - x0 + 1
    bbox_h = y1 - y0 + 1
    bbox_area = bbox_w * bbox_h
    frac = count / total
    bbox_frac = bbox_area / total
    fill_frac = count / bbox_area if bbox_area else 0
    coherent = frac >= 0.02 and bbox_frac < 0.80 and fill_frac >= 0.20
    return {
        "bucket": candidate_bucket,
        "count": count,
        "frac": frac,
        "bbox": (x0, y0, x1, y1),
        "bbox_frac": bbox_frac,
        "fill_frac": fill_frac,
        "coherent": coherent,
    }

regions = []
for candidate, _ in bucket_counts.most_common(4)[1:4]:
    region = coherent_region(candidate)
    if region:
        regions.append(region)

region = next((r for r in regions if r["coherent"]), None)

passes_base = (
    distinct > 20
    and dom_frac < 0.90
    and not blue_baseline
    and not red_baseline
    and big_buckets >= 3
)
passes = passes_base and region is not None

print(f"distinct={distinct} dominant={dominant} dom_frac={dom_frac:.4f}")
print(f"big_color_buckets={big_buckets} blue_baseline={blue_baseline} red_baseline={red_baseline}")
if region:
    print(
        "coherent_region="
        f"bucket={region['bucket']} frac={region['frac']:.4f} "
        f"bbox={region['bbox']} bbox_frac={region['bbox_frac']:.4f} "
        f"fill_frac={region['fill_frac']:.4f}"
    )
else:
    print("coherent_region=None")
print(f"VERDICT={'PASS' if passes else 'FAIL'}")
sys.exit(0 if passes else 1)
PY
