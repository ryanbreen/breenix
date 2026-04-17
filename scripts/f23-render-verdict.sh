#!/usr/bin/env bash
# Strict bwm render verdict for Parallels captures.
#
# Usage:
#   scripts/f23-render-verdict.sh <png_path>
#
# Returns 0 if rendered content is present, 1 otherwise.

set -euo pipefail

PNG="${1:?png path required}"

python3 - "$PNG" <<'PY'
import sys
import warnings
from PIL import Image
from collections import Counter
from math import sqrt

img = Image.open(sys.argv[1]).convert('RGB')
w, h = img.size

# Crop top 40px to remove the Parallels toolbar from the verdict.
content = img.crop((0, 40, w, h))
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    pixels = list(content.getdata())
total = len(pixels)

distinct = len(set(pixels))
c = Counter(pixels)
dominant, dom_count = c.most_common(1)[0]
dom_frac = dom_count / total

def dist(a, b):
    return sqrt(sum((a[i] - b[i]) ** 2 for i in range(3)))

def bucket(p):
    return (p[0] >> 3, p[1] >> 3, p[2] >> 3)

bc = Counter(bucket(p) for p in pixels)
big_buckets = sum(1 for v in bc.values() if v / total > 0.01)

blue_baseline = dist(dominant, (100, 149, 237)) < 15
red_baseline = dist(dominant, (255, 0, 0)) < 15

passes = (
    distinct > 20
    and dom_frac < 0.90
    and not blue_baseline
    and not red_baseline
    and big_buckets >= 3
)

print(f"distinct={distinct} dominant={dominant} dom_frac={dom_frac:.4f}")
print(f"big_color_buckets={big_buckets} blue_baseline={blue_baseline} red_baseline={red_baseline}")
print(f"VERDICT={'PASS' if passes else 'FAIL'}")
sys.exit(0 if passes else 1)
PY
