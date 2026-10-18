#!/usr/bin/env python3
"""Scratch test equipment (not in the repo): read the driver's screendumps.

For each frame: how many places show the kernel's cursor sprite exactly (one
means no trail), and where the runner panel's background lies relative to the
boot screen's stage rows (bottom 528 at 1280x800) and status line (y 728).
"""
import glob
import json
import os
import sys

import numpy as np
from PIL import Image

BITMAP = [
    "200000000000", "220000000000", "212000000000", "211200000000", "211120000000",
    "211112000000", "211111200000", "211111120000", "211111112000", "211111111200",
    "211111111120", "211111222220", "211112000000", "211211200000", "212021120000",
    "220021120000", "200002120000", "000002200000",
]
COLORS = {"1": (255, 255, 255), "2": (0, 0, 0)}
PANEL_BG = (26, 26, 46)
STAGES_BOTTOM, STATUS_Y = 528, 728

out_dir = sys.argv[1]
report = []
for path in sorted(glob.glob(os.path.join(out_dir, "*.ppm"))):
    img = np.asarray(Image.open(path).convert("RGB")).astype(np.int16)
    h, w, _ = img.shape
    ch, cw = len(BITMAP), len(BITMAP[0])
    match = np.ones((h - ch + 1, w - cw + 1), dtype=bool)
    for dy, row in enumerate(BITMAP):
        for dx, v in enumerate(row):
            if v == "0":
                continue
            target = np.array(COLORS[v], dtype=np.int16)
            window = img[dy:dy + h - ch + 1, dx:dx + w - cw + 1]
            match &= (window == target).all(axis=-1)
    ys, xs = np.nonzero(match)
    cursors = [(int(x), int(y)) for y, x in zip(ys, xs)]
    panel = (img == np.array(PANEL_BG, dtype=np.int16)).all(axis=-1)
    py, px = np.nonzero(panel)
    bbox = None if len(py) == 0 else [int(px.min()), int(py.min()), int(px.max()), int(py.max())]
    inside = bbox is not None and bbox[1] > STAGES_BOTTOM and bbox[3] < STATUS_Y
    png = path[:-4] + ".png"
    Image.open(path).save(png)
    entry = {"frame": os.path.basename(path), "size": [w, h], "cursor_sprites": cursors,
             "panel_bbox": bbox, "panel_between_stages_and_status": inside}
    report.append(entry)
    print(json.dumps(entry))
with open(os.path.join(out_dir, "analysis.json"), "w") as f:
    json.dump(report, f, indent=1)
