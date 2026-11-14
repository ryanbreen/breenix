# Intent-border recovery audit for #2952

At Breenix `b8fe3d40`, the retained comparison marks 85 intent rows as
unrecoverable. Code and retained-pixel inspection found no placement defect to
repair in this unit. These rows remain flagged for the operator in Vigil.
This audit adds a host guard and records causes; it changes no guest code,
reference, approval, recovery predicate, cap or case expectation.

The source evidence is the GPU capture `20261114T074015Z-metal` and BreenGL
capture `20261114T100121Z-metal`, paired with the approved references in
`docs/suites/frames/screens.json` at `b8fe3d40`. This is a screen investigation,
not a claim that the GPU capture satisfies final-head hardware acceptance.
Coordinator ledger line 165 governs that separate acceptance work.

The retained comparison report SHA-256 is
`f3cc3d315d62495debb0aa6f6ce7901e3c4548c4d3b976806938fa00bf1e9d05`.
The private diagnostic report SHA-256 is
`d9e9c492adbd2cff82e699ba517e7bd8a7d93a274e38e813fb25ed8018de0d24`.
It records each row's two image identities, frame dimensions, white-pixel
bounds, ring completeness, checker surround, and interior uniformity. Decoded
BGRA hashes matched the recorded identities for the 135 distinct images used
by these 85 pairs. No picture was approved by visual judgment.

## Placement and recovery contracts

- `libs/libbreenix/src/suite_intent.rs:32-34`: when source and mode are equal,
  the source occupies the whole frame at scale 1 and has no border. Adding a
  border inside it would cover judged pixels; shrinking it would resample them.
- `suite_intent.rs:36-40`: smaller pictures use the largest integer scale with
  a 64-pixel size allowance, or scale 1 when they fit with a complete 2-pixel
  ring (the size allowance is then only 4 pixels).
- `suite_intent.rs:42-73`: oversized pictures fold or page without dropping
  source pixels; the available page size is mode minus 4, not mode minus 64.
  `suite_intent.rs:51` retains the full height of wide strips.
- `suite_intent.rs:13-20` centers the picture and puts its 2-pixel ring outside
  it. `suite_intent.rs:131-140` draws that ring and its interior; these are
  placement operations, not recovery metadata.
- Vigil `f632660`, `Sources/Vigil/Breenix/BreenixScreenIntent.swift:19-37`
  requires a centered complete white ring, uniform or frame-aligned checker
  surround, and an interior no larger than mode minus 64. Lines 42-62 recover
  an integer source grid only from observed color transitions and the scale
  formula. A uniform field supplies no grid evidence. Authored dimensions
  must not be substituted to turn an unknown scale into an approval.

The Breenix layout and stream drawing above are unchanged from `origin/main`
`a85904b6`; the branch's `suite_intent.rs` edits concern display custody in
`Frame::present` and its constructor, not placement. The new modes expose
existing borderless and folded cases. Vigil's conservative rule does not
invert the full-mode, near-edge and folded producer layouts, and refusing such a frame is appropriate
for this mechanical-approval path: it must not invent a ring or source grid.
No guard was loosened to admit one.

## Row groups

| New / approved-reference recovery | Rows | Cause |
| --- | ---: | --- |
| Uniform / uniform | 42 | Complete ring and checker surround on both sides, but neither interior has a color transition. The source scale is unknown under Vigil lines 53-62. |
| Borderless / borderless | 39 | Full-mode sources take the explicit borderless branch, on both old and new modes. White text or cursor pixels are not a centered complete ring. |
| Borderless / recovered | 1 | `gpu/present/flip` now renders a native-size source into the native mode; its old smaller source had a ring. |
| Near edge / near edge | 1 | `gpu/profile/int-float-exact` folds the full strip into a page wider than Vigil's mode-minus-64 bound. Both captures have complete rings. |
| Recovered / near edge | 1 | `breengel/bmp/padding` fits the old mode with the 4-pixel allowance, but exceeds its mode-minus-64 bound; the new frame recovers. |
| Recovered / borderless | 1 | `breengel/panel/kernel-screens` has a recoverable native console picture in the new scale-2 frame, but its old console reference fills its mode. |

The uniform group contains 20 GPU and 22 BreenGL rows. Direct interior pixel
inspection shows a single RGB value on each side of each pair, with the same
value across the pair. That establishes color equality but leaves the original source
dimensions unknown, so the existing flags stay.

The borderless/borderless group contains two GPU fault cases and 37 BreenGL
rows. GPU source allocation follows the display mode in
`userspace/programs/src/suite_gpu/present.rs:447-456,490-499`.
BreenGL gets the active scale-2 extent from
`userspace/programs/src/suite_breengel/window_cases/composition.rs:118-125`;
full-screen handoffs include `composition.rs:643-647,1172-1179` and
`panel.rs:22-24`. The old 2560x1600 source filled 2560x1600, and the new
2560x2048 source fills 2560x2048. The GPU fault pictures similarly fill the
old 2560x1600 and new 1280x1024 modes. Placement is intentional, not clipping.

The remaining four pairs have these concrete geometries (rectangles are
`x,y,width,height`, excluding the ring):

| Row | New | Approved reference | Producing code |
| --- | --- | --- | --- |
| `gpu/present/flip` | Source/frame 1280x1024; no ring | Source 1280x800 in frame 2560x1600 at `(640,400,1280,800)` | `present.rs:166-180` derives the native source; `suite_intent.rs:32` fills an equal mode. |
| `gpu/profile/int-float-exact` | Source 16384x2 folded into `(2,499,1276,26)` in 1280x1024 | Same source folded into `(2,793,2556,14)` in 2560x1600 | `profile.rs:259-271` produces the strip; `suite_intent.rs:42,51,65-73` packs it. 1276 exceeds 1216; 2556 exceeds 2496. |
| `breengel/bmp/padding` | Source 1446x1553 at `(557,247,1446,1553)` in 2560x2048; scale 1 recovered | Same source at `(557,23,1446,1553)` in 2560x1600; 1553 exceeds 1536 | `scripts/metal/intent-pictures.tsv:185` declares the fixture panel; `suite_intent.rs:37` admits the complete ring at scale 1. |
| `breengel/panel/kernel-screens` | Source 1280x1024 at `(640,512,1280,1024)` in 2560x2048; scale 1 recovered | Source/frame 2560x1600; no ring | `composition.rs:134-139` reads the restored console mode; `panel.rs:168-175` hands off that console extent. |

Folding does not carry a source-cell map in the border. The strip therefore
also needs its authored tiling to reconstruct the original 16384x2 grid; its
near-edge ring is not evidence for the ordinary enlargement inverse.
The old padding and console pictures are immutable references. A new boot
cannot supply them with new borders.

## Named rows

The 85 rows below are `intent` checkpoints and remain flagged. The group
labels refer to the code and geometry above; new comes before reference.

| Suite | Case | Group |
| --- | --- | --- |
| gpu | `device/no-fallback` | uniform/uniform |
| gpu | `submit/validation-atomic` | uniform/uniform |
| gpu | `submit/stream-overflow` | uniform/uniform |
| gpu | `submit/fence-order` | uniform/uniform |
| gpu | `submit/device-error-eio` | uniform/uniform |
| gpu | `submit/fence-echo-checked` | uniform/uniform |
| gpu | `submit/watchdog-quarantine` | uniform/uniform |
| gpu | `readback/full-target` | uniform/uniform |
| gpu | `readback/error-invalidates-targets` | uniform/uniform |
| gpu | `readback/efault` | uniform/uniform |
| gpu | `textures/upload-visibility` | uniform/uniform |
| gpu | `profile/clear-exact` | uniform/uniform |
| gpu | `profile/int-float-exact` | near-edge/near-edge |
| gpu | `profile/scissor-exact` | uniform/uniform |
| gpu | `profile/alpha-write` | uniform/uniform |
| gpu | `profile/no-dither` | uniform/uniform |
| gpu | `pipelines/rounded-rect` | uniform/uniform |
| gpu | `isolation/quota-memory` | uniform/uniform |
| gpu | `present/flip` | borderless/recovered |
| gpu | `present/present-completion` | uniform/uniform |
| gpu | `present/present-owner-only` | uniform/uniform |
| gpu | `present/fault-skip-flush` | borderless/borderless |
| gpu | `present/fault-wrong-scanout` | borderless/borderless |
| gpu | `performance/wait-no-spin` | uniform/uniform |
| breengel | `raster/fill-rect` | uniform/uniform |
| breengel | `raster/fill-rect-offscreen-left` | uniform/uniform |
| breengel | `raster/fill-rect-offscreen-top` | uniform/uniform |
| breengel | `raster/fill-rect-empty` | uniform/uniform |
| breengel | `raster/circle-fill` | uniform/uniform |
| breengel | `raster/circle-fill-offscreen-left` | uniform/uniform |
| breengel | `raster/extreme-work-bound` | uniform/uniform |
| breengel | `raster/blit-clip` | uniform/uniform |
| breengel | `raster/blit-scale` | uniform/uniform |
| breengel | `raster/clip-empty` | uniform/uniform |
| breengel | `text/ttf-clip-left` | uniform/uniform |
| breengel | `text/ttf-clip-top` | uniform/uniform |
| breengel | `bmp/padding` | recovered/near-edge |
| breengel | `windows/create` | uniform/uniform |
| breengel | `windows/create-zeroed` | uniform/uniform |
| breengel | `windows/destroy` | uniform/uniform |
| breengel | `windows/fork-no-mapping` | uniform/uniform |
| breengel | `windows/commit-immutable` | uniform/uniform |
| breengel | `windows/resize` | uniform/uniform |
| breengel | `windows/commit-waits-render` | uniform/uniform |
| breengel | `composition/compose-position` | borderless/borderless |
| breengel | `composition/compose-overlap` | borderless/borderless |
| breengel | `composition/compose-offscreen` | borderless/borderless |
| breengel | `composition/compose-fullscreen` | borderless/borderless |
| breengel | `composition/compose-many` | borderless/borderless |
| breengel | `composition/no-compositor` | borderless/borderless |
| breengel | `composition/backend-error` | borderless/borderless |
| breengel | `composition/cursor-hidden` | borderless/borderless |
| breengel | `composition/cursor-no-trail` | borderless/borderless |
| breengel | `composition/cursor-shape` | borderless/borderless |
| breengel | `composition/cursor-stationary-repaint` | borderless/borderless |
| breengel | `composition/takeover-giveback` | borderless/borderless |
| breengel | `composition/ownership-stack` | borderless/borderless |
| breengel | `composition/gpu-composite` | borderless/borderless |
| breengel | `composition/scale-2` | borderless/borderless |
| breengel | `events/resize-request` | uniform/uniform |
| breengel | `events/processed-ack` | uniform/uniform |
| breengel | `scanout/changing-scene` | borderless/borderless |
| breengel | `scanout/full-destination` | borderless/borderless |
| breengel | `scanout/binding` | borderless/borderless |
| breengel | `scanout/atomic-record` | borderless/borderless |
| breengel | `scanout/format-canonical` | borderless/borderless |
| breengel | `scanout/fault-skipped-transfer` | borderless/borderless |
| breengel | `scanout/fault-wrong-resource` | borderless/borderless |
| breengel | `scanout/fault-stale-fence` | borderless/borderless |
| breengel | `scanout/fault-outside-damage` | borderless/borderless |
| breengel | `scanout/present-flip` | borderless/borderless |
| breengel | `scanout/hidpi-mode` | borderless/borderless |
| breengel | `panel/stage-layout` | uniform/uniform |
| breengel | `panel/tile-stamps` | borderless/borderless |
| breengel | `panel/tile-thumbnail` | borderless/borderless |
| breengel | `panel/spotlight-picture` | borderless/borderless |
| breengel | `panel/spotlight-timing` | borderless/borderless |
| breengel | `panel/milestone-ribbon` | borderless/borderless |
| breengel | `panel/finale` | borderless/borderless |
| breengel | `panel/stage-hidpi` | borderless/borderless |
| breengel | `panel/handoff` | borderless/borderless |
| breengel | `panel/between-cases` | borderless/borderless |
| breengel | `panel/cursor-restore` | borderless/borderless |
| breengel | `panel/cursor-trail-detected` | borderless/borderless |
| breengel | `panel/kernel-screens` | recovered/borderless |

## Guard and disposition

`tests/suite_intent_test.rs::native_intent_layout_keeps_full_and_folded_pictures_lossless`
exercises production `picture_pages` and `samples` at the relevant old and new
extents. It checks full-mode placement, ring geometry, exact scale-1 mapping,
no destination overlap, and a source hit count of one across each judged picture.
It passed on the unchanged `b8fe3d40` implementation, so it is a U29 regression
guard, not evidence of a red/green repair. Check-only commit: `1d32adff`.

The scoped host command was:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test \
  --manifest-path libs/libbreenix/suite/Cargo.toml --lib \
  native_intent_layout_keeps_full_and_folded_pictures_lossless -- \
  --exact suite::intent_tests::native_intent_layout_keeps_full_and_folded_pictures_lossless
```

Result: exit 0, one passed, no failures. The host compile reported two existing
`unexpected cfg` warnings at `suite_memory.rs:123` and `suite.rs:215`; both cfg
sites and the suite package's missing check-cfg metadata are present on
`origin/main`. The new test introduces no warning. No guest source changed,
so this unit runs neither architecture userspace builds nor boots. The brief's
full architecture-build requirement applies to a placement repair; this audit
leaves the placement intact. No retained capture was rescored or screen decision changed.

A new boot at the same guest code would not change any of these causes. The
acceptance unit still follows coordinator line 165 for the final-head console
and GPU boots, and handles the resulting candidate identities separately.
The 85 flags remain included in the existing 253 operator flags; there are no
new approvals in this unit.
