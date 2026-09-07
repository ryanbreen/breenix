## Summary

`scripts/f24-render-verdict.sh` is the render oracle for the `./run.sh --parallels
--test N` gate. Its PASS/FAIL is a monotone function of **how many scanlines the
host-side capture happened to receive**, not of whether the guest rendered. On the
ten captures of the five-run 120 s sweep at `72ad554d2fc701156c71f05ea7dbe87f4608cb3e`
it splits captures that all show the same rendered Breenix desktop into 5 PASS and
5 FAIL, purely on captured area.

## Evidence

Every one of the ten captures was opened and looked at. All ten show rendered
guest content: the `breenix` menu bar with a live ET clock, the desktop wallpaper
grid, and the `Bounce` window's title bar (blue strip, `Bounce` label, grey
minimise button, red close button). Six also show bounce balls; three show the
whole 1280x960 desktop including the bottom taskbar. <!-- claim-lint:ok: 10/10 captures opened individually -->

The frames differ only in where the host capture stopped. Last non-black scanline
per capture (`PIL`, row is non-black if any pixel sampled every 8 px sums > 12),
against the verdict `scripts/f24-render-verdict.sh` returns for the same file: <!-- claim-lint:ok: 10/10 captures measured, one table row each -->

| capture | last non-black row (of 960) | VERDICT |
|---|---|---|
| run2 screenshot-2 | 959 | PASS |
| run5 screenshot-1 | 918 | PASS |
| run3 screenshot-1 | 361 | PASS |
| run1 screenshot-2 | 255 | PASS |
| run5 screenshot-2 | 238 | PASS |
| run4 screenshot-2 | 110 | FAIL |
| run2 screenshot-1 |  85 | FAIL |
| run3 screenshot-2 |  50 | FAIL |
| run1 screenshot-1 |  47 | FAIL |
| run4 screenshot-1 |  43 | FAIL |

The ordering is perfectly monotone: every capture with a transferred band of 238
rows or more passes, every capture with 110 rows or fewer fails. Nothing else
separates the two groups. <!-- claim-lint:ok: 10/10 captures re-scored individually -->

The mechanism inside the script, from its own printed diagnostics:

- The `coherent_region` that a PASS is built on is the **desktop wallpaper**
  bucket, not a window. Full frames score `bucket=(3, 3, 9) bbox=(0, 457, 1279, 863)
  bbox_frac=0.4424`; partial frames score the darker top strip
  `bucket=(1, 2, 5) bbox=(1, 6, 1279, 140) bbox_frac=0.1466`. So the script rewards
  visible *background area*.
- `run2 screenshot-1` and `run4 screenshot-2` both **have** a coherent region
  (`frac=0.0284` and `frac=0.0467`, over the `frac >= 0.02` floor) and still fail,
  on `dom_frac < 0.90` — `dom_frac` is 0.9500 and 0.9228, and the dominant colour
  is `(0, 0, 0)`, the *untransferred remainder of the frame*. The gate is therefore
  reading "fraction of the frame the capture missed" as "fraction of the screen the
  guest failed to draw".
- The three `coherent_region=None` captures (`distinct` 10, 19, 6) are the 43-50
  row bands: after the dominant black, the top-4 buckets the script inspects are
  too small to clear `frac >= 0.02`. <!-- claim-lint:ok: 3/10 captures, distinct counts from the script output -->

The same predicate would score a correctly rendering desktop FAIL merely for
having a small window on a mostly empty desktop, which is what the bounce
workload is.

## Why this matters

This oracle is what a `5x120s Parallels sweep passes` acceptance clause resolves
to. In the previous round it produced 2 PASS out of 7 attempts and the sweep was
read as not passing, on frames that showed a working desktop.

## Proposed fix (not applied)

1. **Score only the transferred band.** Compute `L`, the last non-black scanline,
   and evaluate `distinct`, `dom_frac`, `big_color_buckets` and `coherent_region`
   over `img.crop((0, 40, w, L + 1))` instead of the full 960 rows. Print `L` and
   the capture attempt count in the verdict output, so a partial transfer shows up
   as a number instead of silently becoming a render FAIL.

2. **Judge on the presence of any rendered window, not on background area.** Add a
   window-chrome predicate: at least one horizontal run of >= 200 px of the bwm
   active-title colour `(40, 100, 220)` (tolerance 25/35/40 per channel) anywhere in
   the frame. Measured over the eleven round-3 captures and six historical ones:

   | capture set | longest title-bar run |
   |---|---|
   | all 11 captures at `72ad554d` (5 runs x 2, plus the wait_stress run) | 404 px, row 38, every one | <!-- claim-lint:ok: 11/11 captures measured individually -->
   | `sweeps/network-483-465-2026-09-07/evidence/run{1,2}-test120/screenshot.png` (solid black) | 0 px |
   | `sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-1/screenshot.png` (solid black) | 0 px |
   | `sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-3/screenshot.png` (menu bar only, no window) | 0 px |
   | `sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-4/screenshot.png` (solid cornflower blue) | 0 px |
   | `sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-5/screenshot.png` (full desktop) | 404 px |

   The predicate accepts every frame with a drawn window and refuses every frame
   without one, including the `run3-test120-3` frame that has desktop chrome but no
   window — which a "any coherent region" rule alone would risk accepting. <!-- claim-lint:ok: 17/17 captures measured in the table above -->

3. **Make the capture deterministic instead of racing the transfer.**
   `scripts/parallels/capture-display.sh` already retries past an all-black warmup
   frame; extend the retry predicate to "the frame is complete" — retry, on the
   existing schedule, until the bottom band is non-black or two consecutive captures
   agree on `L`. Preserve every attempt so a genuine partial guest scanout stays
   visible rather than being retried away. <!-- claim-lint:ok: 0/4 proposed items applied -->

4. **Keep `CAPTURE_MISSING` (exit 2) exactly as it is.** A wholly black or solid
   single-colour frame must stay a refusal. The historical captures listed above are
   that shape and must not become passes. <!-- claim-lint:ok: 0/4 proposed items applied -->

Changing the *workload* does not fix this. Capturing while `bterm` is up would put
the terminal in the same top-left region, and a 43-row transfer would still miss
it; the measure, not the subject, is what is wrong.

## Not claimed

Not claimed: that the partial transfers are definitively a host-side capture
artefact rather than a partial guest present. The same VM produces a full 960-row
frame at other capture instants in the same sweep (959 and 918 rows), and the cut is
always a single hard horizontal boundary at a continuously varying row
(43, 47, 50, 85, 110, 238, 255, 361, 918, 959) rather than at damage-rect or window
boundaries, which points at a mid-transfer read. Recording `L` per item 1 is what
would make the question measurable instead of arguable. <!-- claim-lint:ok: 10/10 last-non-black rows measured; 0/10 attributed to a cause -->

Not claimed: any change to the thresholds in this issue has been applied or tested
in a boot. Nothing here was applied. <!-- claim-lint:ok: 0/4 proposed items applied; 0/0 boots run -->

Evidence for every number above: the ten `screenshot-{1,2}.png` files under the
round-3 sweep's `run{1..5}-test120/` directories and the six historical
`screenshot.png` files named in the tables. <!-- claim-lint:ok: 10/10 round-3 and 6/6 historical captures named in the tables -->
