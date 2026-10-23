# BreenGL: the written contract

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
This is the contract the `breengel` suite (`docs/suites/breengel.json`, `/sbin/suite-breengel`) cites, as
the POSIX suites cite the standard. It fixes BreenGL's 2D library, its text, font, image and widget layers,
and the window protocol, userspace GPU compositor and device observation path, to the precision the cases need:
the API each case calls, the formula or literal table its expected pixels come from, the parser and decoder
bounds, the window interface's semantics and the error each operation returns. The plan of record is
`docs/gui/plan.md`; section numbers written "plan §N" refer to it. Where the plan already fixes a rule, this
document transcribes it and gives it a section number; where it leaves a detail to this contract, the detail
is fixed here. The GPU rebuild follows `gpu.md` section 2 and rulings B1–B8; its
[case reconciliation](breengel-gpu-cases.md) assigns all 399 cases and the five retired identities;
ruling B41 adds `events/resize-request`, the 400th.

Each case's failure message names the section it checks. A case whose layer is not present fails with a
message naming the missing piece and its section, never skips, and never passes because a layer is missing.

## 1. Conventions

- **Integers.** Public geometry is `i32`. Every coordinate computation (an end `x + w`, a product
  `stride × y`) is done in `i64` or checked arithmetic, so no input wraps.
- **Rectangles** are `(x, y, w, h)` and half-open: they cover the pixels `x ≤ px < x + w`, `y ≤ py < y + h`.
  A rectangle with `w ≤ 0` or `h ≤ 0` is empty. `A ∩ B` is the usual intersection; an empty intersection is
  empty whatever its corner.
- **Colours.** `Color { r, g, b }` is an opaque straight colour. A premultiplied colour is `(b, g, r, a)`
  with each colour channel ≤ `a`.
- **Rounding.** `div255(x) = (x + 128 + ((x + 128) >> 8)) >> 8`, the exact round-to-nearest of `x / 255` for
  `0 ≤ x ≤ 65025`. `round_half_away(n / d)` rounds a rational to the nearest integer, halves away from zero,
  computed exactly in integers.
- **Errors** in the kernel interface are negative errno values on the raw return (section 25). Library
  errors are the enums named in each section.
- **Expectations** come from this document's literal tables and formulas, from independent fixtures whose
  provenance is recorded here, or from a lower layer whose own cases certify it first (plan §2.5). The code
  under test never produces its own expectation.

## 2. Pixels and alpha

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
- **Formats.** A 4-byte pixel is stored `B, G, R, X` when the surface is BGR (the canonical format,
  BGRX8888) and `R, G, B, X` when it is not. A 3-byte pixel is `B, G, R` or `R, G, B` the same way. The
  BGR flag swaps the order of the three colour bytes and changes nothing else. Render targets use the
  GPU profile's B8G8R8A8 or R8G8B8A8 formats; three-byte formats exist only for upload and
  conversion of completed readback into capture/encoding output, never as render targets.
- **X byte.** Every primitive writes the X byte of a 4-byte opaque pixel as `0xFF`. Every capture and
  every comparison normalizes X to `0xFF` first (section 20.1), so a case that compares colours compares the
  three colour bytes. `raster/x-byte` checks X itself.
- **Premultiplied surfaces** are BGRA8888; a valid pixel has each colour channel ≤ alpha, and fully
  transparent is all zero.
- **Premultiplying** a straight colour `c` with alpha `a` gives `div255(c × a)`. It is applied exactly once,
  at the API boundary that accepts straight colour (`blit_straight`, section 5.8).
- **Source-over** on premultiplied values: `out_c = s_c + div255(d_c × (255 − s_a))` and
  `out_a = s_a + div255(d_a × (255 − s_a))`. For valid inputs the result never exceeds 255; no saturation
  is applied. Literal results (`raster/alpha-over-examples`):

  | source `(c, a)` | destination `(c, a)` | `out_c` | `out_a` |
  |---|---|---|---|
  | (128, 128) | (0, 255) | 128 | 255 |
  | (128, 128) | (255, 255) | 255 | 255 |
  | (64, 128) | (200, 255) | 164 | 255 |
  | (64, 128) | (100, 100) | 114 | 178 |
  | (0, 0) | (77, 77) | 77 | 77 |
  | (200, 255) | (13, 50) | 200 | 255 |

- **Alpha extremes.** A source with alpha 255 replaces the destination; a source with alpha 0 (and so all
  zero) leaves it unchanged.
- **Coverage blend** of an opaque foreground `f` over a background `b` with 8-bit coverage `a` (anti-aliased
  text): `out = div255(f × a + b × (255 − a))` per channel. `a = 255` gives `f`; `a = 0` leaves `b`. LCD
  text applies the same blend per channel with that channel's coverage.
- **16-bit to 8-bit** (PNG): `(v × 255 + 32767) / 65535`, integer division.
- **API** (`libgfx::alpha`): `div255(x: u32) -> u8`, `premultiply(c: u8, a: u8) -> u8`,
  `over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4]` (BGRA premultiplied), `blend(f: Color, b: Color, a: u8) ->
  Color` (the coverage blend).

## 3. Surfaces

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
Surfaces own GPU targets, created through BGL on syscall 416, not CPU pixel mappings.
`RESOURCE_CREATE` with kind target creates distinct, zeroed resource handles; the device maximum
reported by `CAPS` (capped at 16,384) bounds each dimension. Empty dimensions, dimensions past
that maximum and unsupported render formats are EINVAL (the library reports Empty, TooLarge
and Format respectively). Null and Misaligned are no longer surface errors: no pixel pointer
is accepted. A handle is never an address. Guest backing is charged to `RLIMIT_AS` and the
process GPU quota; `COUNTERS.resource_bytes` and resource count return exactly to baseline
when destruction and the final device unref complete, within the one-second release window.
A resource can retain its backing after handle destruction while a holder exists (§17.3).

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
`UPLOAD` accepts padded rows, including three-byte RGB/BGR input; `READBACK` returns canonical
four-byte rows, which may be converted to three-byte capture/encoding output. Conversion
layouts retain StrideTooShort for stride below width × bpp and SizeOverflow for
stride × height above 2³¹ − 1 or overflowing; these are checked before memory access. Neither transfer
reads or writes the caller's row padding. User buffers are guarded on both sides; extreme
rectangles cannot escape those buffers. Device draws preserve a separate canary target and the
caller clip. Clipped row copies upload whole pixels and use a clipped GPU copy; a rectangle
starting at the row end writes nothing and never wraps into another row (§5.9).

## 4. Clipping

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
The clip stack emits GPU scissors; every pixel-set rule below is unchanged.

- Every primitive draws only inside the surface and inside the current clip.
- `push_clip(rect)` sets the clip to the intersection of `rect` and the current clip; `pop_clip()` restores
  the clip before the matching push. With no clip pushed the clip is the whole surface. An empty clip draws
  nothing and marks no damage (section 6).
- Coordinates anywhere in `i32` are valid input: a primitive clips against the surface and the clip in
  `i64`, and never wraps an end coordinate into the surface (`raster/fill-rect-overflow`).

## 5. Primitives

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
All coordinates and expected pixel sets below stand. Rectangles use `solid`; lines, filled
and outlined circles, rounded rectangles and gradients use their corresponding profile
predicate pipelines. Blits use `copy`, `copy_scaled`, `over` or `over_straight`; single pixels
are one solid instance and clear is a GPU clear. No primitive produces CPU pixels. The closed
forms and their proofs are `gpu.md` §1.5.1; they implement the tables below without walking an
unclipped path. The midpoint-circle predicate uses the exact integer closed form specified
there and its eightfold symmetry, so the literal octant table remains its independent oracle.

All primitives write opaque pixels (section 2) unless they say otherwise, and change exactly the pixels given
here, intersected with the surface and the clip.

### 5.1 `fill_rect(fb, x, y, w, h, color)`

The pixels of the half-open rectangle `(x, y, w, h)`. Nothing when `w ≤ 0` or `h ≤ 0`.

### 5.2 `draw_rect(fb, x, y, w, h, color)`

The one-pixel perimeter: the pixels of `(x, y, w, h)` that are not in `(x + 1, y + 1, w − 2, h − 2)`.
Nothing when `w ≤ 0` or `h ≤ 0`.

### 5.3 `draw_line(fb, x0, y0, x1, y1, color)`

The pixels walked by this integer algorithm, both endpoints included:

```text
dx = |x1 − x0|, dy = −|y1 − y0|, sx = (x0 < x1 ? 1 : −1), sy = (y0 < y1 ? 1 : −1), err = dx + dy
loop: plot (x, y); if (x, y) = (x1, y1) stop
      e2 = 2·err; if e2 ≥ dy { err += dy; x += sx }; if e2 ≤ dx { err += dx; y += sy }
```

A line crossing the edges writes exactly the in-surface, in-clip pixels of its unclipped path
(`raster/line-clip`), with work bounded by section 7, so a line between far-away points is clipped
before it is walked. Literal tables (`raster/line-bresenham`), in walk order:

| Line | Pixels |
|---|---|
| `0,0 → 7,3` | (0,0) (1,0) (2,1) (3,1) (4,2) (5,2) (6,3) (7,3) |
| `7,3 → 0,0` | (7,3) (6,3) (5,2) (4,2) (3,1) (2,1) (1,0) (0,0) |
| `0,0 → 3,7` | (0,0) (0,1) (1,2) (1,3) (2,4) (2,5) (3,6) (3,7) |
| `3,7 → 0,0` | (3,7) (3,6) (2,5) (2,4) (1,3) (1,2) (0,1) (0,0) |
| `0,0 → 5,5` | (0,0) (1,1) (2,2) (3,3) (4,4) (5,5) |
| `0,5 → 5,0` | (0,5) (1,4) (2,3) (3,2) (4,1) (5,0) |
| `0,0 → 9,2` | (0,0) (1,0) (2,0) (3,1) (4,1) (5,1) (6,1) (7,2) (8,2) (9,2) |
| `2,9 → 0,0` | (2,9) (2,8) (2,7) (1,6) (1,5) (1,4) (1,3) (0,2) (0,1) (0,0) |

A horizontal or vertical line covers exactly its span, endpoints included (`raster/line-axes`).

### 5.4 `fill_circle(fb, cx, cy, r, color)`

The pixels `(x, y)` with `(x − cx)² + (y − cy)² ≤ r²`, computed in `i64`. `r = 0` is the centre pixel;
`r < 0` draws nothing. A circle partly or wholly off any edge writes nothing outside the surface
(`raster/circle-fill-offscreen-left`).

### 5.5 `draw_circle(fb, cx, cy, r, color)`

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
The GPU predicate reflects a candidate into `X ≥ Y ≥ 0`. For r ≥ 1 it selects exactly
`X² − X + Y² ≤ r² < (X+1)² − (X+1) + Y²`, using two-word integers after bounding both
absolute offsets by r. At Y=0 the largest satisfying X is r. For X≥1 the expression is
strictly increasing, so that largest X is unique. Advancing Y changes the expression by
`2Y+1`; decrementing X changes it by `−2X+2`. These are precisely the midpoint walk's
error increments, retaining equality on the lower side. Induction therefore yields the
same X for each Y until X<Y. Reflection yields its eight symmetric pixels. Radius zero
selects the centre and negative radii select nothing. This is the clean-room derivation
in `gpu.md` §1.5.1; the literal table below remains the independent check.

The midpoint circle. For `r ≥ 1` the first octant is walked from `(r, 0)`:

```text
x = r, y = 0, d = 1 − r
while x ≥ y: record (x, y); y += 1; if d ≤ 0 { d += 2y + 1 } else { x −= 1; d += 2(y − x) + 1 }
```

and the circle is the eight-fold symmetric set of every recorded `(x, y)`: `(cx ± x, cy ± y)` and
`(cx ± y, cy ± x)`. `r = 0` is the centre pixel; `r < 0` draws nothing. The recorded `x` for `y = 0, 1, …`,
per radius (`raster/circle-outline`):

| r | first-octant x for y = 0, 1, … |
|---|---|
| 0 | 0 |
| 1 | 1 1 |
| 2 | 2 2 |
| 3 | 3 3 2 |
| 4 | 4 4 4 3 |
| 5 | 5 5 5 4 |
| 6 | 6 6 6 5 5 |
| 7 | 7 7 7 6 6 5 |
| 8 | 8 8 8 7 7 6 |
| 9 | 9 9 9 9 8 8 7 |
| 10 | 10 10 10 10 9 9 8 7 |
| 11 | 11 11 11 11 10 10 9 9 8 |
| 12 | 12 12 12 12 11 11 10 10 9 |
| 13 | 13 13 13 13 12 12 12 11 10 9 |
| 14 | 14 14 14 14 13 13 13 12 12 11 10 |
| 15 | 15 15 15 15 14 14 14 13 13 12 11 |
| 16 | 16 16 16 16 16 15 15 14 14 13 13 12 |
| 17 | 17 17 17 17 17 16 16 16 15 14 14 13 12 |
| 18 | 18 18 18 18 18 17 17 17 16 16 15 14 13 |
| 19 | 19 19 19 19 19 18 18 18 17 17 16 16 15 14 |
| 20 | 20 20 20 20 20 19 19 19 18 18 17 17 16 15 14 |
| 21 | 21 21 21 21 21 20 20 20 19 19 18 18 17 17 16 15 |
| 22 | 22 22 22 22 22 21 21 21 21 20 20 19 18 18 17 16 |
| 23 | 23 23 23 23 23 22 22 22 22 21 21 20 20 19 18 17 17 |
| 24 | 24 24 24 24 24 23 23 23 23 22 22 21 21 20 20 19 18 17 |
| 25 | 25 25 25 25 25 25 24 24 24 23 23 22 22 21 21 20 19 18 |
| 26 | 26 26 26 26 26 26 25 25 25 24 24 24 23 23 22 21 21 20 19 |
| 27 | 27 27 27 27 27 27 26 26 26 25 25 25 24 24 23 22 22 21 20 19 |
| 28 | 28 28 28 28 28 28 27 27 27 27 26 26 25 25 24 24 23 22 21 21 20 |
| 29 | 29 29 29 29 29 29 28 28 28 28 27 27 26 26 25 25 24 24 23 22 21 |
| 30 | 30 30 30 30 30 30 29 29 29 29 28 28 28 27 27 26 25 25 24 23 22 21 |
| 31 | 31 31 31 31 31 31 30 30 30 30 29 29 29 28 28 27 27 26 25 25 24 23 22 |
| 32 | 32 32 32 32 32 32 31 31 31 31 30 30 30 29 29 28 28 27 26 26 25 24 23 |
| 33 | 33 33 33 33 33 33 32 32 32 32 31 31 31 30 30 29 29 28 28 27 26 25 25 24 |
| 34 | 34 34 34 34 34 34 33 33 33 33 33 32 32 31 31 31 30 29 29 28 28 27 26 25 24 |
| 35 | 35 35 35 35 35 35 34 34 34 34 34 33 33 33 32 32 31 31 30 29 29 28 27 26 25 25 |
| 36 | 36 36 36 36 36 36 36 35 35 35 35 34 34 34 33 33 32 32 31 31 30 29 29 28 27 26 |
| 37 | 37 37 37 37 37 37 37 36 36 36 36 35 35 35 34 34 33 33 32 32 31 30 30 29 28 27 26 |
| 38 | 38 38 38 38 38 38 38 37 37 37 37 36 36 36 35 35 34 34 33 33 32 32 31 30 29 29 28 27 |
| 39 | 39 39 39 39 39 39 39 38 38 38 38 37 37 37 36 36 36 35 35 34 33 33 32 32 31 30 29 28 |
| 40 | 40 40 40 40 40 40 40 39 39 39 39 38 38 38 37 37 37 36 36 35 35 34 33 33 32 31 30 30 29 |

### 5.6 `rounded_rect(fb, x, y, w, h, radius, color)`

The pixels of `(x, y, w, h)`, except in the four `r × r` corner squares, where `r = min(radius, w / 2, h / 2)`.
In the top-left square (`x ≤ px < x + r`, `y ≤ py < y + r`) a pixel is drawn when
`(2(x + r − px) − 1)² + (2(y + r − py) − 1)² ≤ (2r)²`, which tests the pixel's centre against the quarter
circle of radius `r` centred on the square's inner corner; the other three corners mirror it. `r ≤ 0` is a
plain `fill_rect`.

### 5.7 `gradient_v(fb, x, y, w, h, top, bottom)`

Row `i` of `(x, y, w, h)` (`0 ≤ i < h`) is one colour; each channel is
`(top × (h − 1 − i) + bottom × i + (h − 1) / 2) / (h − 1)` in integer division, and `top` when `h = 1`.

### 5.8 Blits

- `blit(dst, src, x, y)`: destination pixel `(x + i, y + j)` becomes source pixel `(i, j)`, converted between
  formats by channel, for every source pixel whose destination is inside the surface and clip.
- `blit_over(dst, src, x, y)`: `src` is premultiplied BGRA; each destination pixel becomes `over(src, dst)`.
- `blit_straight(dst, src, x, y)`: `src` is straight-alpha RGBA; each source pixel is premultiplied once
  (section 2) and then blended with `over`.
- `blit_scaled(dst, src, x, y, n)`, `n ≥ 1`: source pixel `(i, j)` fills the `n × n` square at
  `(x + i·n, y + j·n)`.

### 5.9 Uploaded rows and single pixels

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
`copy_row_from` and byte-granular partial pixels leave the drawing API. A whole-pixel upload
followed by a GPU region copy clips to the destination row and clip. Uploading eight pixels
and copying them at (5,1) into an 8×4 destination changes exactly pixels (5..8,1); the same
copy at x=8 changes nothing. No row wrap or padding access is permitted.
`put_pixel` emits a single solid instance; pixels can be read only from completed device
readback. `clear` fills the current clip on the GPU.

## 6. Damage

Damage means *written*, not *changed*: a primitive marks the rectangle of the pixels it may write after
clipping, even when it writes the colour already there. A draw clipped to nothing marks nothing. A surface
accumulates one bounding rectangle, the union of every mark since it was last taken;
`take_dirty() -> Option<DirtyRect>` returns it and resets it to nothing.

| Primitive | Rectangle marked (then intersected with the surface and clip) |
|---|---|
| `put_pixel(x, y)` | `(x, y, 1, 1)` |
| `fill_rect`, `draw_rect`, `rounded_rect`, `gradient_v` `(x, y, w, h)` | `(x, y, w, h)` |
| `draw_line(x0, y0, x1, y1)` | `(min(x0, x1), min(y0, y1), |x1 − x0| + 1, |y1 − y0| + 1)` |
| `fill_circle`, `draw_circle` `(cx, cy, r ≥ 0)` | `(cx − r, cy − r, 2r + 1, 2r + 1)` |
| blits at `(x, y)` of a `w × h` source, scale `n` | `(x, y, w·n, h·n)` |
| uploaded row copy at `(x, y)` of `k` whole pixels | `(x, y, k, 1)` |
| `clear` | the clip |
| text | the union of every drawn glyph's bitmap rectangle |

`raster/damage-sound` checks soundness: over a seeded random scene, every pixel that changed lies inside
the accumulated rectangle.

## 7. Work counters and raster bounds

`libgfx::work`, `libfont::work` and `libimg::work` each keep an always-on step counter: `reset()` and
`steps() -> u64`. A step is one iteration of any loop, including iterations that reject or skip, and one call
of any recursive function (plan §12.3). Allocations are counted by the suite's own global allocator
wrapper, which counts calls and bytes per thread.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
| Work | Bound |
|---|---|
| a raster primitive at any coordinates | CPU geometry setup at most 64 steps per primitive (a text run is one primitive per glyph); emitted fragment area at most the clipped bounding rectangle; work estimate at most that area times the pipeline weight plus its required blend-copy area; a wholly clipped primitive emits no draw or work | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| font load | at most `16 × file bytes + 65,536` steps and `2 × file bytes + 64 KiB` allocated |
| one glyph's outline | compound depth ≤ 8, ≤ 256 components, ≤ 65,535 points and no more than maxp declares (`maxPoints` for a simple glyph, `maxCompositePoints` for a compound after expansion), ≤ `16 × glyph bytes + 8 × P + 4,096` steps for its `P` points after compound expansion, ≤ `64 × points + 64 KiB` allocated |
| one glyph's flattening | quadratic subdivision depth ≤ 9 (≤ 512 segments per curve), ≤ 65,536 segments per glyph (else `GlyphTooComplex`), ≤ `1,028 × P` steps for `P` points after compound expansion |
| one glyph's rasterization at `s` px | target ≤ `4s × 4s`; CPU crossings and sort ≤ `E + H × (8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64` steps; GPU fragment work is the `coverage_spans` formula in `gpu-abi.md`, including its blend copy. The existing allocation bound `H × C + 4C + 48E + 4,096` and admission test against the original total bound 2²⁸ remain; GlyphTooExpensive precedes all crossing, raster and device work | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| inflate | ≤ `4 × output + 16 × input + 1,024 × blocks` steps; 32 KiB window plus ≤ 64 KiB of tables besides the caller's output |
| image dimensions (BMP, PNG decode and encode) | width and height ≤ 16,384 and `width × height` ≤ 16,777,216, checked before any allocation |
| PNG decompressed size | rejected at the first byte beyond `rows × (1 + row bytes)` for the image's own dimensions, per pass for Adam7 |

**The independent square.** Four independently specified edges surround a 1,022 px square
at (1,1) within a 1,024×1,024 target. The CPU crossing bound is **214,084**, with at least
8,192 sub-scanline visits. There are 16 crossings per interior row, on 1,022 rows. The exact
GPU estimate is `8192 × (1024² + 1024 × 16 × 1022) + 1024²` = **145,761,501,184**, including
the blend copy. It is summed over submissions individually below the unchanged 2³¹ cap.
Neither omitting CPU crossing/sort work nor omitting GPU span fill can satisfy both checks.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B9 and #2177 specify the required Fixture identity check. -->
**GPU-only enforcement.** `COUNTERS` records upload bytes by Image/Fixture and instances,
fragment area and work estimate. Primitive output has zero Image uploads; only
declared static source tables may upload as Fixture, whole and byte-identical by content
comparison, with the content hash recorded. Any other CPU pixel upload, under any purpose,
fails `raster/cpu-pixels-zero`. Clipped or culled runs and once-per-context table uploads
need no per-run byte equality or per-case byte allowance. A draw must
actually reach the GPU and stay within its constant instance budget. The same accounting
guard refuses planted renamed CPU-fill uploads (`cpu-image`), per-pixel solid draws
(`cpu-instances`), and a passing case's uploaded expected image (`passing-oracle`). A host
structure check in gpu-raster also refuses the raw mutable-pixel library fixture and forbidden
`libgfx-ref` consumers, as `gpu.md` §2.4 specifies. Expected/diff image uploads are allowed
only as labelled failure artifacts, never as the passing actual image.

## 8. Text

### 8.1 The 5×7 bitmap font (`libgfx::font`)

These tables and checks stand, drawn by GPU `bitmap_text`.

`draw_char(fb, byte, x, y, color, scale)`, `draw_text(fb, bytes, x, y, color, scale)`,
`text_width(bytes, scale)`. A glyph is seven row bytes; pixel `(col, row)`, `0 ≤ col < 5`, is set when bit
`0x10 >> col` of row byte `row` is set, and is drawn as the `scale × scale` square at
`(x + col·scale, y + row·scale)`. Glyphs advance `6 × scale`. `text_width` of `n ≥ 1` bytes is
`(n − 1) × 6 × scale + 5 × scale`, and 0 for none. A byte without a glyph draws the **replacement box**: all
seven rows `0x1F`. Literal glyphs the cases use:

| Byte | Rows |
|---|---|
| `A` | 0E 11 11 1F 11 11 11 |
| `0` | 0E 11 13 15 19 11 0E |
| `1` | 04 0C 04 04 04 04 0E |
| `/` | 01 02 02 04 08 08 10 |
| replacement | 1F 1F 1F 1F 1F 1F 1F |

### 8.2 The 16 px anti-aliased bitmap font (`libgfx::bitmap_font`)

These cells and checks stand, drawn by GPU `bitmap_text`.

`draw_char(fb, ch, x, y, fg)` draws the `noto-sans-mono-bitmap` 0.3 raster (`Regular`, `Size16`) of `ch`
with its top-left at `(x, y)`, each pixel by the coverage blend (section 2) of `fg` over the pixel already
there. The crate's raster is the independent reference cell (`text/noto-ascii`): the suite links the same
crate version libgfx draws from and reads the raster directly. A character the crate lacks draws the raster of `?`.

### 8.3 TrueType text (`libgfx::ttf_font`, `libfont`)

- **Sizes** are pixels per em. A size below 1 or above 1,024 is rejected: `rasterize_glyph` returns
  `FontError::BadSize` and draws nothing. `size26 = round_half_away(size × 64)`.
- **Advances and kerning** are 26.6 fixed point: `advance26(g) = round_half_away(advance_units(g) × size26 /
  unitsPerEm)` and `kern26(l, r) = round_half_away(kern_units(l, r) × size26 / unitsPerEm)`, using the legacy
  `kern` table's format 0 pairs. `kern_units` sums the pair's values over every horizontal, non-minimum,
  non-cross-stream format 0 subtable in table order; a subtable with the override bit (coverage bit 3)
  replaces the sum so far. Layout is one codepoint at a time: the pen starts at 0, glyph `i` is drawn
  with its origin at `x + floor((pen_i + 32) / 64)`, and `pen_{i+1} = pen_i + advance26(g_i) + kern26(g_i,
  g_{i+1})`. Advances are accumulated in 26.6 and rounded once per glyph position, never truncated per glyph.
- **Width.** `draw_text` returns, and `text_width` equals, `floor((pen_end + 32) / 64)`: the distance from
  the pen start to the end of the last advance, rounded once (`text/ttf-advance`, `ttf-width`,
  `ttf-kerning`).
- **Translation.** A glyph's pixels depend on its pen position only by translation: drawing at
  `(x + dx, y + dy)` moves every pixel by `(dx, dy)`, and text starting off the left or top edge draws exactly
  its visible part (`text/ttf-clip-left`, `ttf-clip-top`). A glyph whose bitmap rectangle, from its record
  header, lies wholly outside the surface or clip is laid out (it still advances the pen) but is not
  rasterized, so text wholly outside rasterizes no glyph.
- **Missing glyphs.** A codepoint the font does not map draws glyph 0 (`.notdef`), never a substitute
  character.
- **UTF-8.** `draw_text_bytes(fb, font, bytes, x, y, size, fg)` decodes UTF-8 and draws one U+FFFD for each
  maximal invalid subpart (Unicode §3.9, "U+FFFD substitution of maximal subparts"), and never panics.
  `draw_text` takes `&str`.
- **Line metrics.** `Font::metrics(size)` returns the hhea ascender, descender and line gap scaled by
  `size / unitsPerEm`; each value rounded to 26.6 (`round_half_away(value × 64)`) equals
  `round_half_away(hhea_value × size26 / unitsPerEm)`.
- **Glyph metrics.** `Font::glyph_metrics(glyph) -> GlyphMetrics { advance, lsb, x_min, y_min, x_max, y_max }`
  in font units, from hmtx and the glyph's bounding box.
- **LCD subpixel** text (`draw_text_subpixel`) samples three columns per pixel and blends per channel.
- **Calls with many arguments.** Text entry points that take ten or more arguments, including trailing `f32`
  or 26.6 values, give the same pixels as the same text drawn through a short call (`text/many-args-abi`;
  the aarch64 code generation defect behind it is recorded in the project's memory as the f32 ABI bug).
- **Non-goals:** shaping (GSUB, GPOS), hinting, bidirectional text, combining-mark positioning, CFF outlines,
  colour and bitmap glyphs.

**Normative binary32 coverage order.** CPU crossing preparation visits rows, then sub-scans
0..7 at `row + (sub + 0.5) × (1/8)`, then segments in outline order. It rejects an edge
whose endpoints are both below that y or both at/above it, skips zero dy, and computes
`dy = y1-y0`, `t = (y-y0)/dy`, `x = x0 + t×(x1-x0)` in that order, in binary32. Direction
is +1 for increasing y, otherwise −1. Stable bottom-up merge sort orders equal-x crossings
without changing their source order. No reassociation or fused multiply-add is allowed.
The sorted crossings and directions are uploaded as geometry, not coverage pixels.

GPU `coverage_spans` starts each pixel's accumulator at binary32 zero, visits sub-scans in
that same order and walks their sorted crossings in order with winding initially zero and
fill_start zero. A zero-to-nonzero transition saves x; a nonzero-to-zero transition adds
the span from max(fill_start,0) to min(x,width). A nonzero final winding adds through width.
For each span, right≤left adds nothing; a one-pixel span adds right−left; otherwise the first
pixel adds `(start+1)−left`, interior pixels add 1, and the last adds `right−(end−1)`, with
start=floor(left) and end=min(ceil(right),width). Additions occur in sub-scan and span order.
Conversion is `(coverage × (1/8)) × 255`, binary32 at each operation, clamped at 0/255 and
otherwise truncated to u8. LCD scales segment x by 3 first and applies the identical order.
The frozen CPU reference and four reviewed references remain unchanged; different device
pixels are defects. At display scale 2, text rasterizes at twice its logical size, equals
that size's certified reference and differs from 2× replication. `text/hidpi-sizes`
exercises the production UI text entry point with logical position (16,8) and the theme's
logical 16 px size, and compares with the certified 32 px H at physical (32,16).
The fixture never multiplies the size or position passed to the text stack.

### 8.4 The glyph cache

<!-- claim-lint:ok: Normative atlas accounting fixed by coordinator ruling B15, issue #2205. -->
`CachedFont::new(font, capacity)` caches GPU-atlas glyphs keyed by `(glyph, size26)` and evicts the least
recently used entry when full. `CachedFont::with_default_cache(font)` uses the contract's capacity: at most
256 entries and at most 4 MiB of atlas storage, evicting until both hold. `cache_stats() -> CacheStats { entries,
bytes, hits, misses }` reports the cache. Under ruling B15 (#2205), the atlas uses
B8G8R8A8 render targets: GPU draws write coverage into one channel, and no CPU-rasterized
coverage is uploaded. R8 remains sample-only. `bytes` counts actual atlas storage,
four bytes per atlas texel, with rows padded to 256 bytes and the allocation
rounded up to 4 KiB pages (B23). Thus `text/cache-hit` expects the kernel's real charge, and `text/cache-bound` requires actual storage above 3 MiB and at most
4 MiB. Both the entry and actual-storage budgets hold after every insertion; unused
atlas space is not hidden from accounting. A glyph served from the cache, or evicted
and rasterized again, is identical to a fresh rasterization.

### 8.5 Reference fonts and their independent tables

The suite reads the fonts installed at `/usr/share/fonts`. Their SHA-256 pins (`text/fonts-pinned`):

| File | SHA-256 | unitsPerEm | hhea ascender, descender, lineGap | numberOfHMetrics | numGlyphs |
|---|---|---|---|---|---|
| `DejaVuSansMono.ttf` | `b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582` | 2048 | 1901, −483, 0 | 4 | 3377 |
| `DejaVuSans.ttf` | `7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954` | 2048 | 1901, −483, 0 | 6238 | 6253 |

The tables below were extracted at authoring time with fontTools 4.62.1, a parser independent of libfont
(provenance: 2026-10-15, from the pinned files above).

- **Metrics** (`text/metrics-independent`): DejaVu Sans Mono, codepoints U+0021 to U+0052. Every glyph has
  advance 1233; glyph ids are `codepoint − 0x1D`. Columns: codepoint, lsb, xMin, yMin, xMax, yMax.

  | cp | lsb | bbox | cp | lsb | bbox | cp | lsb | bbox |
  |---|---|---|---|---|---|---|---|---|
  | 21 | 516 | 516 0 719 1493 | 22 | 338 | 338 938 895 1493 | 23 | 2 | 2 0 1229 1470 |
  | 24 | 190 | 190 −301 1114 1556 | 25 | 33 | 33 0 1200 1432 | 26 | 57 | 57 −29 1221 1520 |
  | 27 | 528 | 528 938 702 1493 | 28 | 426 | 426 −270 885 1554 | 29 | 348 | 348 −270 807 1554 |
  | 2A | 166 | 166 586 1067 1520 | 2B | 88 | 88 113 1145 1171 | 2C | 403 | 403 −287 754 303 |
  | 2D | 356 | 356 479 877 643 | 2E | 489 | 489 0 741 305 | 2F | 102 | 102 −190 1079 1493 |
  | 30 | 133 | 133 −29 1100 1520 | 31 | 246 | 246 0 1094 1493 | 32 | 152 | 152 0 1059 1520 |
  | 33 | 137 | 137 −29 1079 1520 | 34 | 102 | 102 0 1135 1493 | 35 | 143 | 143 −29 1069 1493 |
  | 36 | 133 | 133 −29 1100 1520 | 37 | 139 | 139 0 1079 1493 | 38 | 131 | 131 −29 1102 1520 |
  | 39 | 127 | 127 −29 1094 1520 | 3A | 489 | 489 0 741 1063 | 3B | 403 | 403 −287 754 1063 |
  | 3C | 88 | 88 141 1145 1143 | 3D | 88 | 88 352 1145 930 | 3E | 88 | 88 141 1145 1143 |
  | 3F | 244 | 244 0 1040 1520 | 40 | 27 | 27 −319 1178 1395 | 41 | 37 | 37 0 1196 1493 |
  | 42 | 166 | 166 0 1137 1493 | 43 | 139 | 139 −29 1073 1520 | 44 | 137 | 137 0 1106 1493 |
  | 45 | 197 | 197 0 1102 1493 | 46 | 233 | 233 0 1112 1493 | 47 | 102 | 102 −29 1104 1520 |
  | 48 | 137 | 137 0 1096 1493 | 49 | 201 | 201 0 1030 1493 | 4A | 109 | 109 −29 956 1493 |
  | 4B | 137 | 137 0 1225 1493 | 4C | 215 | 215 0 1139 1493 | 4D | 86 | 86 0 1145 1493 |
  | 4E | 139 | 139 0 1094 1493 | 4F | 117 | 117 −29 1116 1520 | 50 | 197 | 197 0 1141 1493 |
  | 51 | 117 | 117 −270 1116 1520 | 52 | 143 | 143 0 1233 1493 | | | |

- **cmap** (`text/cmap-independent`): DejaVu Sans Mono, 200 codepoints. U+0020 to U+007E map to glyphs 3
  to 97 (`codepoint − 0x1D`); U+00A0 to U+00FF to glyphs 98 to 193 (`codepoint − 0x42`); U+0100 → 194,
  U+2013 → 1822, U+2014 → 1823, U+2018 → 1827, U+2019 → 1828, U+201C → 1831, U+201D → 1832; U+E000 and
  U+10FFFF are unmapped (0).
- **hmtx** (`font-parser/hmtx-compressed`): DejaVu Sans Mono has 4 long metrics for 3,377 glyphs, so every
  glyph from 4 on takes the last long advance (1233) and its own left side bearing from the trailing array.
- **Kerning** (`text/ttf-kerning`): DejaVu Sans maps `A` to glyph 36 and `V` to glyph 57, each advancing
  1401; its `kern` table's pair (36, 57) is −131. At 32 px `size26 = 2048`, so `advance26` = 1401 and
  `kern26` = −131: "AV" is 42 px wide and "A" and "V" are 22 px each.
- **Advances** (`text/ttf-advance`): DejaVu Sans Mono at 16 px has `advance26` = 617 (1233 × 1024 / 2048 =
  616.5, rounded away), so a 200-character line ends at pen 123,400 = 1,928 px. The font has no `kern`
  table.
- **Compound glyphs** (`text/compound-glyph`): `é` (U+00E9) is glyph 171 in both fonts, built from glyph 72
  (`e`) at (0, 0) and glyph 118 (`acute`) at (14, 0) in DejaVu Sans Mono and at (139, 0) in DejaVu Sans.
- **Line metrics** (`text/line-metrics`), 26.6, both fonts: ascent, descent, line gap at 8 px = 475, −121, 0;
  12 px = 713, −181, 0; 16 px = 951, −242, 0; 24 px = 1426, −362, 0; 48 px = 2852, −725, 0.

### 8.6 Reviewed references

<!-- claim-lint:ok: Normative reviewed-reference rule in section 8.6 and plan section 7.3; the placement follows the users shared test layout. -->
Cases whose expectation is a whole rendered scene with no practical formula (`text/raster-identical`,
`ttf-glyph`, `ttf-sizes`, `ttf-subpixel`, `compound-transform`, `icons/assets`) compare against a reviewed PNG
under `docs/suites/frames/breengel/`, rendered by `tools/gfx-refgen` from the same `no_std` code at
authoring time and recorded in `docs/suites/frames/PROVENANCE.json` with the generator commit, reviewer,
date and what was checked (plan §7.3). The guest reads them from `/usr/libexec/breenix/share/breengel/frames/`.
Frames saved from runs never become references.

## 9. The UI clock and the font watcher

### 9.1 `breengel::clock`

`breengel::clock::now_ns()` reads the clock page named by `BREENIX_UI_CLOCK` (a file path, mapped shared)
and falls back to `CLOCK_MONOTONIC` without the variable. The page's first 64 bytes:

| Offset | Field |
|---|---|
| 0 | magic `b"BGCLK001"` |
| 8 | `epoch: u64` |
| 16 | `now_ns: u64` |
| 24 | `wall_base_ns: u64` (2026-01-01 00:00:00 UTC) |
| 32 | `futex: u32` |
| 36 | `participants: u32` |
| 40 | reserved, zero |

The participant table that follows is the `ui-testing` effort's (plan §6.3). The provider counts every real
clock read it makes while a page is set and reports the count as `real_clock_reads` in commit metadata
(`Meta`, section 17.1).

### 9.2 Font configuration

`fonts.conf` is `$BREENIX_CONFIG_ROOT/etc/fonts.conf`, or `/etc/fonts.conf` without the variable. It holds
lines `key=value`; blank lines and lines starting with `#` are ignored. A valid file has each of
`mono.font`, `mono.size`, `display.font` and `display.size` exactly once, sizes from 6 to 72, and ends with a
newline. Anything else is malformed, including a file cut short.

### 9.3 `breengel::font::FontWatcher`

- `FontWatcher::new()` loads the configuration and both fonts. `poll() -> Result<Option<FontChange>,
  FontWatchError>` checks, on the UI clock, at most once per 250 ms: `fonts.conf` and each configured font
  file are compared by inode number, size and modification time, so a replacement by rename (a new inode) is
  always seen.
- A changed, valid configuration or a changed font file is loaded whole; later text uses newly rasterized
  glyphs of the new font or size (the cache is cleared for that font). Mono and display fonts change
  independently.
- A malformed configuration, or a configured font file that is missing or does not parse, keeps every
  previous font in use and returns `FontWatchError::Config` or `FontWatchError::Font(path)`.
- A change made at UI time *t* is in use in a window's first commit for an epoch at or after the first check
  after *t*, whether or not the window receives events (`text/fontwatch-idle`). Breengel delivers
  `Event::FontChanged` to the window when a change is applied.

## 10. Font parsing

`libfont::Font::parse(bytes) -> Result<Font, FontError>` accepts TrueType outlines (`glyf`) with `head`,
`hhea`, `maxp`, `cmap`, `hmtx`, `loca` and an optional legacy `kern`. A rejection never panics, never reads
outside the input and stays within section 7's bounds. A defect confined to one glyph rejects that glyph
only: `rasterize_glyph` returns the error for it, and every other glyph still works. Rules, with the error
each one gives (`font-parser/*`):

| Input | Scope | Error |
|---|---|---|
| any read past the end of the file or of a table | font | `Truncated` |
| a table record whose offset or length runs past the file | font | `BadTable(tag)` |
| a missing required table | font | `MissingTable(tag)` |
| `unitsPerEm` of 0 or above 16,384 | font | `BadUnitsPerEm` |
| a `loca` entry beyond the `glyf` table | glyph | `GlyphOutOfRange` |
| `loca` entries that decrease | font | `BadLoca` |
| a `loca` length other than `(numGlyphs + 1)` entries | font | `GlyphCountMismatch` |
| cmap format 4: odd `segCountX2`, end codes not increasing, or a last end code other than 0xFFFF | font | `BadCmap` |
| cmap format 4: an `idRangeOffset` that points outside the subtable | font | `BadCmap` |
| cmap format 12: groups unsorted, overlapping, or with start above end | font | `BadCmap` |
| contour end points that do not increase | glyph | `BadGlyph` |
| more points than `maxp.maxPoints` in a simple glyph or `maxp.maxCompositePoints` in a compound after expansion, or above 65,535 | glyph | `TooManyPoints` |
| a flag repeat or coordinate run past the glyph's data or point count, or a compound's instructions (`WE_HAVE_INSTRUCTIONS`) running past its record | glyph | `BadGlyph` |
| compound nesting deeper than 8, or more than 256 components | glyph | `CompoundTooDeep` |
| a compound that refers to itself, directly or through another | glyph | `CompoundCycle` |
| a component index at or above `numGlyphs` | glyph | `BadComponent` |
| `hmtx` shorter than `4 × numberOfHMetrics + 2 × (numGlyphs − numberOfHMetrics)` bytes, or `numberOfHMetrics` of 0 or above `numGlyphs` | font | `BadHmtx` |
| a glyph past the flattening or rasterization bounds of section 7 | glyph | `GlyphTooComplex`, `GlyphTooExpensive` |

A component's offset (`ARGS_ARE_XY_VALUES`) is in the compound's units; with `SCALED_COMPONENT_OFFSET` set and
`UNSCALED_COMPONENT_OFFSET` clear, it is in the component's units and the component's scale or 2×2 transform
applies to it as well (OpenType `glyf`). The cmap subtables read are the first Unicode subtable of each of
formats 4 and 12; a kern subtable's pairs lie within its length (a format 0 subtable of more than 10,920 pairs
cannot state its size in 16 bits, so a length equal to its size modulo 65,536 stands for that size), and a
read past it is `Truncated`.

A valid format 4 cmap with any number of segments, odd counts included (the OpenType specification's
example has 39), maps exactly as the specification describes. A compressed `hmtx` gives glyphs past the
last long metric the last advance and their own left side bearings. The individual `font-parser` cases
build their fonts in the suite from literal tables and require rejection (any `FontError`) of each defect
and correct parsing of each valid variant; `corpus` requires each of its 64 files to give its documented
error. The corpus lives at `/usr/libexec/breenix/share/breengel/fonts-corpus/`, with `index.txt` naming each
file's error.

## 11. Inflate and zlib (`libimg::inflate`)

- `inflate(input, limit) -> Result<Vec<u8>, InflateError>` decodes a raw DEFLATE stream (RFC 1951);
  `zlib_decompress(input, limit)` a zlib stream (RFC 1950). `Inflater::new(limit)` with
  `feed(&[u8]) -> Result<(), InflateError>` and `finish() -> Result<Vec<u8>, InflateError>` decodes the same
  stream fed in any pieces, with identical results (`inflate/byte-split`).
- **Blocks**: stored (LEN must equal the one's complement of NLEN, else `BadStoredLength`), fixed Huffman,
  dynamic Huffman; type 3 is `BadBlockType`. Decoding ends after the block with BFINAL set.
- **Codes**: an over-subscribed code-length set is `CodeOversubscribed`. An incomplete code is
  `CodeIncomplete`, except a literal/length or distance code of exactly one one-bit code, which is
  accepted (a block holding only end-of-block decodes to nothing). A block whose distance code lengths are
  all zero (RFC 1951 §3.2.7) decodes its literals, and a length symbol in it is `BadSymbol`. A repeat code
  with no previous length is `RepeatWithoutPrevious`. Literal/length symbols 286 and 287 and distance symbols
  30 and 31 are `BadSymbol`.
- **Matches**: a distance before the start of the output is `DistanceTooFar`; distances up to 32,768 and
  lengths up to 258 decode.
- Allocation failure is `OutOfMemory`; output growth is fallible and does not reserve past `limit`.
- **Ends**: a stream cut off at any byte is `Truncated`. Output past `limit` is `OutputLimit`, and nothing is
  written past the limit.
- **zlib**: a bad header check value, a preset dictionary, a method other than 8 and a window above 32 KiB
  (CINFO > 7) are `BadZlibHeader`, `PresetDictionary`, `BadMethod` and `WindowTooLarge`; a wrong Adler-32 is
  `Adler32Mismatch`.
- **Fixtures** (`inflate/independent-fixtures`): twelve streams made at authoring time by an independent
  encoder, with the bytes it recorded, under `/usr/libexec/breenix/share/breengel/inflate/`.

## 12. BMP (`libimg::bmp`)

- `encode_bmp_24(width, height, rgb) -> Vec<u8>` writes a 14-byte `BITMAPFILEHEADER` (`BM`, file size, 0,
  data offset 54), a 40-byte `BITMAPINFOHEADER` (size 40, width, height positive (bottom-up), planes 1,
  24 bits, compression 0, image size, then 16 zero bytes), then the rows bottom-up as `B, G, R`, each padded
  with zero bytes to a multiple of 4. For a 2 × 2 image `[(1,2,3), (4,5,6); (7,8,9), (10,11,12)]` (RGB, top row
  first) the 70 bytes are:

  ```text
  42 4D 46 00 00 00 00 00 00 00 36 00 00 00 28 00 00 00 02 00 00 00 02 00 00 00 01 00 18 00
  00 00 00 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
  09 08 07 0C 0B 0A 00 00 03 02 01 06 05 04 00 00
  ```

- `decode_bmp_24(bytes) -> Option<(width, height, rgb)>` decodes 24-bit uncompressed files, bottom-up
  (positive height) and top-down (negative height) alike, into top-down RGB rows. `decode_bmp(bytes) ->
  Result<Bmp, BmpError>` is the same with the reason for a rejection: `Truncated` (a file shorter than its
  headers or pixel data claim), `BadSignature`, `Unsupported { bpp, compression }`, `TooLarge` (section 7's
  dimension limits) and `SizeOverflow` (a row or image size that overflows). `TooLarge` and `SizeOverflow`
  are found before any allocation; a rejection never panics.
  `BadHeader` denotes zero dimensions, a DIB size below the supported header sizes, or a pixel
  offset inside the headers. The 12-byte `BITMAPCOREHEADER` is supported alongside
  `BITMAPINFOHEADER` and its extensions; its dimensions are unsigned 16-bit and its rows bottom-up.

## 13. PNG (`libimg::png`)

- `decode(bytes) -> Result<Image, PngError>`; `Image { width, height, pixels }` holds premultiplied BGRA
  (opaque images have alpha 255). Grayscale `g` becomes `(g, g, g)`; sub-8-bit samples scale by
  `v × 255 / (2^depth − 1)`; 16-bit samples convert by section 2's formula; a tRNS key colour (grayscale or
  RGB, compared at the image's own depth) becomes transparent; palette tRNS entries apply per index, and
  entries past the tRNS length are opaque. Every legal colour type and depth decodes: grayscale 1, 2, 4, 8,
  16; RGB 8, 16; palette 1, 2, 4, 8; grayscale-alpha 8, 16; RGBA 8, 16. Every other pair is
  `BadColourType`.
- **Filters** 0 to 4 at every bytes-per-pixel width; above 4 is `BadFilter`. **Adam7** decodes to the same
  pixels as the non-interlaced image, including sizes where passes are empty.
- **Chunks**: IHDR first; PLTE before IDAT and required for palette images (`MissingPlte`); PLTE length a
  multiple of 3 and at most 2^depth entries (`BadPlte`); IDAT may be split across chunks, empty ones
  included; IEND required. Out of order is `ChunkOrder`; a bad CRC is `BadCrc`; a chunk length above
  2³¹ − 1 is `BadChunkLength`; an unknown critical chunk is `UnknownCritical`; gAMA, iCCP, sRGB, tEXt and
  other ancillary chunks are ignored (no colour management). Valid tRNS is processed as above;
  invalid tRNS lengths and tRNS on colour types 4/6 are ignored. A palette tRNS before PLTE,
  a repeated tRNS, or tRNS after IDAT is `ChunkOrder`. A palette index past the palette is
  `BadPaletteIndex`. Zero width or height and unknown compression, filter or interlace methods are
  `BadHeader`; a bad signature is `BadSignature`; a cut-off stream is `Truncated`, including
  EOF exactly at a chunk boundary before IEND and a truncated zlib stream in a complete IDAT.
  IEND terminates the PNG datastream; bytes following it are ignored.
- **Limits**: section 7's dimension limits (`TooLarge`, before inflating) and decompressed size (`Bomb`).
  Allocation failure is `OutOfMemory`; decoder allocations are fallible.
- `encode(image) -> Result<Vec<u8>, PngError>` writes RGB (opaque images) or RGBA at depth 8, filter 0 on every row, one
  IDAT of stored or fixed-Huffman deflate blocks in a zlib stream with a correct Adler-32, and correct
  CRCs. Encoding the same image twice gives identical bytes. Images past the dimension limits are refused
  with `TooLarge`.
- **Fixtures**: the public PngSuite images, with the pixels an independent decoder recorded at authoring
  time, under `/usr/libexec/breenix/share/breengel/png/`; `png-decode/from-disk` reads `pinned.png` there.

The image error clarifications follow the operator's 2026-10-15 review-fix brief:
`Vec<u8>` cannot express the required `TooLarge`, hence the `Result` encoding signature;
PNG §11.3.1.1 specifies sample-depth tRNS masking, §§11.2.4 and 13.1 distinguish the
end of the datastream and unexpected EOF, and §13.1 permits ignoring invalid ancillary
payloads. These clarify error reporting and preserve the pixel formulas and dimension limits.
BMP's published BITMAPCOREHEADER layout specifies the 12-byte header's 16-bit fields.

## 14. Widgets (`libbui`)

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
Widgets are driven in-process with explicit `InputState` values (pointer position, button level and its press
and release edges) and drawn into GPU surfaces. R3 uses pinned DejaVu Sans at the theme's 16 px logical size
with a 16 px line box and §8.3 advances; R4 multiplies geometry and font size by display
scale. Expected text uses that certified lower layer; widget rendering never supplies its
own expectation. Editing and event rules in §15 remain unchanged, and cursor scrolling
uses those TrueType advances. Bitmap-font APIs and their independent text cases remain.

### 14.1 The theme

`Theme::dark()`, the theme every widget case uses:

| Field | Value | Field | Value |
|---|---|---|---|
| `widget_bg` | (60, 60, 60) | `text_primary` | (255, 255, 255) |
| `widget_bg_hover` | (80, 80, 80) | `text_secondary` | (180, 180, 180) |
| `widget_bg_active` | (100, 140, 220) | `border` | (120, 120, 120) |
| `accent` | (70, 130, 220) | `panel_bg` | (45, 45, 45) |
| `padding`, `spacing`, `border_width` | 4, 4, 1 | `focus_ring` | (255, 200, 60) |

### 14.2 Events and redraw

`update(&InputState)` returns the widget's event (`WidgetEvent::Clicked`, `Toggled(bool)`,
`ValueChanged(f32)`, or `None`). Each update also reports whether the widget's drawing changed:
`last_update_redraw() -> bool` is false after an input that changed no widget state (`widgets/no-change-ack`),
the library half of section 19.4's processed watermark. Keyboard input reaches a focused widget through
`key(&KeyInput)`, where `KeyInput { key: Key, shift, ctrl, alt, text }` and `Key` names Left, Right, Up, Down,
Home, End, Enter, Escape, Tab, Space, Backspace, Delete and Char. A widget built with `set_enabled(false)` draws
its label in `text_secondary`, its background unchanged, and reports no events.

### 14.3 Widget geometry and behaviour

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
- **Button** `(x, y, w, h)`: background `fill_rect` of the rectangle in `widget_bg` (idle), `widget_bg_hover`
  (pointer inside) or `widget_bg_active` (pressed); a one-pixel `draw_rect` border in `border`; the label in
  `text_primary` at `(x + (w − tw) / 2, y + (h − 16) / 2)`, `tw` its `text_width`, in Rust integer division.
  A press inside followed by a release inside reports exactly one `Clicked`, on the release; a press inside
  released outside reports none.
- **Checkbox** `(x, y, w, h)`: a 14 × 14 box at `(x, y + (h − 14) / 2)`. A press inside the rectangle toggles
  it and reports `Toggled(new state)`. Checked, the box is filled with `accent` and the mark is the two lines
  `(bx + 3, by + 7) → (bx + 5, by + 10)` and `(bx + 5, by + 10) → (bx + 11, by + 3)` in `text_primary`
  (section 5.3); then the box border in `border`; the label follows at `bx + 14 + spacing`. Space toggles a
  focused checkbox.
- **Slider** `(x, y, w, h)`, range `[min, max]`: the track starts at `tx = x + 6` and is `tw = w − 12` wide.
  A press inside starts a drag; while dragging, `value = clamp((pointer_x − tx) / tw, 0, 1)` in `f32` and
  `ValueChanged(min + value × (max − min))` is reported whenever `value` changes. Left and Right step a
  focused slider by its step size, clamped.
- **Label**: text wider than its bounds is drawn cut at the last whole glyph that fits with `...` (three <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  dots, width from §8.3 advances), and nothing outside the bounds.
- **Tab bar** `(x, y, w, h)` of `n` tabs: tabs are at least `text_width(label) + 16` wide; when they fit,
  tab `i` is `(x + i·⌊w / n⌋, y, ⌊w / n⌋, h)`. A release over tab `j` other than the selected one selects it
  and reports `ValueChanged(j)`. When they do not fit, the bar scrolls (`scroll_by(tabs)`) so that every tab
  can be brought into view and selected.
- **Scroll bar** `(x, y, w, h)` over content `C` with view `V`: `max_scroll = max(C − V, 0)`; the thumb is
  `max(⌊V / C × h⌋, 20)` pixels long, capped at `h`, at `y + ⌊offset / max_scroll × (h − thumb)⌋`. It is
  drawn as the track `(x, y, w, h)` in (40, 42, 48) and the thumb `(x + 2, thumb_y + 1, w − 4, thumb − 2)` in
  (80, 85, 100), or (140, 150, 170) while dragged. `scroll(d)` moves the offset by `−40 × d`, clamped to
  `[0, max_scroll]`. Dragging the thumb by `dy` pixels sets `offset = ⌊(thumb_top) / (h − thumb) × max_scroll⌋`
  for the thumb's new top, clamped. Offsets are `i32`, ratios `f32`.
- **Menu bar** at `(x, y, w, 22)`: menu `i`'s title is `(x_i, y, text_width(label) + 20, 22)`, titles laid left <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  to right from `x`. A press on a title opens its menu: the dropdown is `(x_i, y + 22, W, H)`, filled with
  `widget_bg` and bordered in `border`, `W = max(140, widest row)`, `H` the sum of its rows (22 per item, 9
  per separator). An item's row is `18 + 24 + text_width(label)` wide, plus `24 + text_width(shortcut) + 8`
  when it has a shortcut and `24` when it has none; a shortcut is written `Ctrl+`, `Shift+`, `Alt+` (those it
  has, in that order) and its key in upper case. Item `k`'s label is drawn at `(x_i + 18, row_y + (22 − 16) / 2)` in `text_primary`, or `text_secondary`
  when disabled. A release over an enabled item activates it and closes the menu; a disabled item cannot be
  chosen; a press outside the bar and dropdown closes the menu without choosing.
  `match_shortcut(ascii, ctrl, shift, alt)` returns the enabled item bound to that shortcut, accepting the
  control byte (Ctrl+S is 0x13) and either letter case. With a menu open, Up and Down move the highlight,
  Enter chooses and Escape closes.
- **Layout**: `h_stack(container, item_w, n, spacing)` places item `i` at `(cx + i·(item_w + spacing), cy,
  item_w, ch)`; `v_stack` does the same vertically.
- **Focus ring**: a focused button, checkbox, slider, tab bar or text field draws a one-pixel `focus_ring`
  rectangle around its bounds, `(x − 2, y − 2, w + 4, h + 4)`, and none when not focused.

## 15. Text entry, focus and the file picker

- **TextField** (`libbui::widget::text_field`): `TextField::new(rect, max_chars)`; `key(&KeyInput)` edits;
  `text()`, `cursor()` (a character index), `selection() -> Option<(start, end)>`, `selected_text()`,
  `insert_str(&str)` (paste). Typed text is inserted at the cursor; Backspace and Delete remove the character
  before and after it; Left, Right, Home and End move it by one character or to either end; Shift with an
  arrow extends a selection, and typing or pasting replaces it and leaves the cursor after the inserted
  text. Characters are Unicode scalar values: a multi-byte character inserts and deletes as one. Input past
  `max_chars` is refused whole. Text wider than the field scrolls so the cursor stays visible.
- **Focus traversal** (`libbui::focus::FocusChain`): Tab moves focus to the next focusable widget in
  insertion order, Shift+Tab to the previous, both wrapping and skipping disabled widgets; a press on a
  focusable widget focuses it.
- **File picker** (`libbui::widget::file_picker`): entries are listed directories first, then files, each
  group in byte order of the name. Up and Down move the selection and Enter opens it (a directory navigates,
  a file is `Selected`). The picker's path field navigates to a typed path. A press selects a row; a second
  press on the same row within 400 ms and 4 pixels on each axis, by the `InputState` timestamps
  (`time_ms`), opens it; a later second press only selects. Lists longer than the view scroll by Up, Down,
  Page Up and Page Down and by the wheel (3 rows per step). A directory that cannot be read shows its error
  text in the list area instead of entries.

## 16. Icons (`libicon`)

- **Time.** Icons take explicit time: `Icon::update(dt_us, mouse)`. Animation state integrates in fixed
  quanta of 1 ms up to the time given, carrying the remainder, so the state after `update(1_000_000)` equals
  the state after a thousand `update(1_000)` and after any split of the same total (`icons/step-invariance`).
- **Springs.** `Spring { value, velocity, target, stiffness, damping }` advances one 1 ms quantum by
  semi-implicit Euler in `f32`: `velocity += (−stiffness × (value − target) − damping × velocity) × 0.001`,
  then `value += velocity × 0.001`; `update(0.001)` is one quantum. A spring is settled when
  `|value − target| < 0.1` and `|velocity| < 0.1`. The reference spring (stiffness 300, damping 20, from 0
  toward 1), within 1e-4 (`icons/spring-steps`):

  | ms | value | velocity |
  |---|---|---|
  | 1 | 0.000300 | 0.300000 |
  | 10 | 0.015507 | 2.730217 |
  | 20 | 0.055123 | 4.886276 |
  | 50 | 0.266207 | 8.397326 |
  | 100 | 0.691898 | 7.693301 |
  | 200 | 1.098417 | 0.854046 |
  | 500 | 0.992322 | 0.098756 |
  | 1000 | 0.999973 | 0.000876 |

- **States** (`IconBase`, `icons/states`, `interrupt`): Idle → HoverIn when the pointer enters; HoverIn →
  Hovering after 250 ms, → Pressed on a press, → HoverOut when the pointer leaves; Hovering → Pressed on a
  press, → HoverOut when it leaves; HoverOut → Idle after 200 ms, → HoverIn when it enters again; Pressed →
  Clicked on the release, → HoverOut when the pointer is outside with no button held; Clicked → Hovering
  (pointer inside) or Idle after 500 ms. Each transition resets the time in state.
- **Continuity** (`icons/reversal`): `Icon::hover_amount() -> f32` in `[0, 1]` is continuous in time: leaving
  mid-hover starts the return from the current amount, never from 0 or 1.
- **Reset**: after `reset()` an icon draws exactly as a newly built icon of the same kind at the same size and
  position.
- **Bounds**: an icon drawn centred at `(cx, cy)` with size `s` writes only inside the square of side
  `s + 2 × bounds_overflow()` centred there, in every state, particles included (`icons/bounded`, `settles`,
  `particles-bounded`). `Icon::settled() -> bool` is true once every animation has settled, which happens
  within 2,000 ms of UI time after the last input. A `ParticlePool` never holds more live particles than its
  capacity, and keeps them inside the box given to `ParticlePool::with_bounds`.
- **Easing**: `linear(t) = t`; `ease_out_cubic(t) = 1 − (1 − t)³`; `ease_in_cubic(t) = t³`;
  `ease_in_out_cubic(t) = 4t³` for `t < 0.5`, else `1 − (−2t + 2)³ / 2`; `ease_out_elastic(t) = 0` at 0, 1 at
  1, else `2^(−10t) × sin((10t − 0.75) × 2π / 3) + 1`; `ease_out_bounce` with `n₁ = 7.5625`, `d₁ = 2.75` (the
  usual four parabolas); `ease_out_back(t) = 1 + c₃(t − 1)³ + c₁(t − 1)²`, `c₁ = 1.70158`, `c₃ = c₁ + 1`. The
  table at `t = 0, 0.1, …, 1` (`icons/easing`); the library's values must be within 1e-5, except
  `ease_out_elastic`, which uses `sin_approx` and must be within 0.002:

  | function | 0 | 0.1 | 0.2 | 0.3 | 0.4 | 0.5 | 0.6 | 0.7 | 0.8 | 0.9 | 1 |
  |---|---|---|---|---|---|---|---|---|---|---|---|
  | `linear` | 0 | 0.1 | 0.2 | 0.3 | 0.4 | 0.5 | 0.6 | 0.7 | 0.8 | 0.9 | 1 |
  | `ease_out_cubic` | 0 | 0.271 | 0.488 | 0.657 | 0.784 | 0.875 | 0.936 | 0.973 | 0.992 | 0.999 | 1 |
  | `ease_in_cubic` | 0 | 0.001 | 0.008 | 0.027 | 0.064 | 0.125 | 0.216 | 0.343 | 0.512 | 0.729 | 1 |
  | `ease_in_out_cubic` | 0 | 0.004 | 0.032 | 0.108 | 0.256 | 0.5 | 0.744 | 0.892 | 0.968 | 0.996 | 1 |
  | `ease_out_elastic` | 0 | 1.25 | 1.125 | 0.875 | 1.03125 | 1.015625 | 0.984375 | 1.003906 | 1.001953 | 0.998047 | 1 |
  | `ease_out_bounce` | 0 | 0.075625 | 0.3025 | 0.680625 | 0.91 | 0.765625 | 0.7725 | 0.930625 | 0.94 | 0.988125 | 1 |
  | `ease_out_back` | 0 | 0.408828 | 0.705802 | 0.907132 | 1.029028 | 1.087697 | 1.099352 | 1.0802 | 1.046451 | 1.014314 | 1 |

- **`sin_approx`** stays within 0.0012 of sin at 4,096 evenly spaced points of [−π, π] (`icons/sin-approx`).
  The reference is computed in the suite by a Taylor series in `f64`, independent of the library and of the
  C library's `sin`.

### 16.1 Reviewed icon catalogue

`icons/assets` compares a 640 × 512 scene, background (12, 19, 31), against
`frames/icons/assets.png`. Rows are Home, Search, Save and Arrow Forward; columns are Idle,
HoverIn, Hovering, Pressed and Clicked. Each icon is size 48, centred at
`(64 + 128 × column, 64 + 128 × row)`. Each cell starts with a fresh icon. Hover input has
`hovering = true`, `rel_x = rel_y = 0`; all other fields are initially false.

- Idle: no update.
- HoverIn: 101,000 µs of hover.
- Hovering: 251,000 µs of hover.
- Pressed: 251,000 µs of hover, a 1,000 µs press edge with the button held, then 50,000 µs held.
- Clicked: the Pressed schedule, a 1,000 µs release edge, then 120,000 µs of hover.

`tools/gfx-refgen` renders this literal scene for review; the guest drives its own copy of these input
schedules and compares every pixel of its own rendering against the reviewed file. Icons are procedural drawings; no external
icon asset is used. The reference's redistribution licence and review are recorded in the frame provenance.

## 17. The window interface, version 2

### 17.1 The system call

`window(op: u32, arg: *mut u8, len: usize) -> isize` is system call **414** on both architectures. `arg`
points to the operation's argument structure and `len` must equal its size, else EINVAL. Structures are
`repr(C)`, little-endian, at the literal offsets below; fields marked *out* are written by the kernel, and
every reserved field must be zero on input (EINVAL otherwise). The return is a non-negative value or a
negative errno (section 25). Op 0 (`VERSION`, `arg` null, `len` 0) returns **2**. An unknown op is EINVAL
(`isolation/unknown-op`). The interface is identical on ARM64 and x86-64; nothing in it is compiled for one
architecture only.

`Rect` is `{ x: i32 @0, y: i32 @4, w: i32 @8, h: i32 @12 }` (16 bytes).

| Op | Name | Argument (size) | Returns | Errors besides EFAULT and the authority table |
|---|---|---|---|---|
| 0 | `VERSION` | none (0) | 2 | — |
| 1 | `CREATE` | `Create` (64) | 0 | EINVAL size or buffer count; EAGAIN past the window limit; ENOMEM against `RLIMIT_AS` |
| 2 | `DESTROY` | `WinId` (8) | 0 | ENOENT stale id |
| 3 | `REGISTER` | `Title` (24) | 0 | EINVAL title; ENOENT |
| 4 | `SET_TITLE` | `Title` (24) | 0 | EINVAL title; ENOENT |
| 5 | `LIST` | `List` (16) | registered entries visible to the caller under section 17.4 | — |
| 6 | `ACQUIRE` | `Acquire` (296) | 0 | EINVAL, ETIMEDOUT, EINTR, ENOENT, ENOMEM replay resources, ENODEV unavailable core, EIO replay/render/reset or one-shot late failure |
| 7 | `COMMIT` | `Commit` (304) | 0 | EBUSY double commit; EINVAL buffer or damage; ENOENT; EIO already-failed render; EINTR publication-order wait |
| 8 | `COMMIT_NO_CHANGE` | `NoChange` (32) | 0 | ENOENT; EINTR pending-publication wait |
| 9 | `RESIZE` | `Resize` (48) | 0 | EINVAL size; ENOMEM; ENOENT |
| 10 | `READ_EVENTS` | `ReadEvents` (40) | events copied | ETIMEDOUT, EINTR, ENOENT |
| 11 | `WRITE_EVENTS` | `WriteEvents` (24) | events queued | EINVAL type or length; ENOENT |
| 12 | `EVENT_FD` | `WinId` (8) | a descriptor; window 0, for the compositor, the scene descriptor (section 19.2) | ENOENT, EMFILE |
| 13 | `SET_GEOMETRY` | `Geometry` (24) | 0 | ENOENT |
| 14 | retired `MAP_COMMITTED` | `MapBuffers` (40) | ENOSYS | after size validation, including a stale id |
| 15 | `CAPTURE` | `Capture` (56) | 0 | EINVAL short destination; ENOENT; EINTR pending-publication wait; EIO failed pending publication |
| 16 | `MAP_TREE` | `MapTree` (24) | 0 | ENOENT |
| 17 | `CLAIM_COMPOSITOR` | `Claim` (16) | 0 | EBUSY while a claim lives |
| 18 | `RELEASE_COMPOSITOR` | none (0) | 0 | — |
| 19 | `COMPOSITE` | `CompositeSubmit` (112) | 0 | EPERM, ENOENT, EINTR, EIO, EAGAIN, ENOMEM, EBUSY, ENODEV, EINVAL, EDQUOT; detailed effects in gpu-abi.md B13 |
| 20 | `COMPOSITE_RECORD` | `RecordQuery` (24) | 0 | ENOENT serial no longer kept; EINVAL short length |
| 21 | retired `MAP_COMPOSITOR_TEXTURE` | `MapTree` (24) | ENOSYS | after size validation |
| 22 | `SCANOUT_READBACK` | `Readback` (48) | 0 | EPERM, EINVAL, ENOMEM, ENODEV, EINTR, EAGAIN pending composition, EIO reason 1–12 |
| 23 | `SET_CURSOR` | `Cursor` (16) | 0 | EINVAL shape |
| 24 | `HOLD_EVIDENCE` | `WinId` (8) | 0 | EBUSY another holder; ENOENT |
| 25 | `READ_REMNANT` | `Capture` (56) | 0 | ENOENT |
| 26 | `RELEASE_REMNANT` | `WinId` (8) | 0 | ENOENT |
| 27 | `GIVE_DISPLAY` | `Give` (8) | 0 | EPERM, EINVAL, ESRCH, EBUSY, ENODEV, ENOMEM, EINTR, EIO; core custody rules in section 18 |
| 28 | `READ_META` | `MetaQuery` (24) | 0 | ENOENT |
| 29 | `COMPOSITE_SNAPSHOT` | `SceneSnapshot` (96) | 0 | EPERM, EINVAL, EBUSY, EAGAIN pending render, ENOMEM, ENODEV, EIO |

Argument structures (offsets in bytes):

- `WinId`: `id: u64 @0`.
- `Create`: `width: u32 @0`, `height: u32 @4`, `buffers: u32 @8` (2 or 3), `tree_kib: u32 @12` (0 means 64;
  at most 1,024; the kernel rounds it up to whole 4 KiB pages, so no two buffers share a tree page, and writes
  the size it used back), *out* `id: u64 @16`, *out* `stride: u32 @24`, reserved `@28`, *out*
  `buffer: [u64; 3] @32` (the owner's GPU target handle for each buffer, 0 past `buffers`), *out* `tree: u64 @56` (the
  first tree page; buffer `i`'s page is `tree + i × tree_kib × 1024`, with the `tree_kib` written back).
- `Title`: `id: u64 @0`, `ptr: u64 @8`, `len: u32 @16`, reserved `@20`.
- `List`: `ptr: u64 @0` (an array of `WinInfo`), `capacity: u32 @8`, reserved `@12`.
- `WinInfo` (128): `id: u64 @0`, `owner: u32 @8` (pid), `flags: u32 @12` (bit 0 registered, 1 visible,
  2 minimized, 3 focused), `width: u32 @16`, `height: u32 @20`, `x: i32 @24`, `y: i32 @28`, `z: u32 @32`,
  `title_len: u32 @36`, `title: [u8; 64] @40`, `commit_id: u64 @104`, `pixel_gen: u64 @112`,
  `backend: u32 @120`, reserved `@124`.
- `Acquire`: `id: u64 @0`, `timeout_ns: u64 @8` (`u64::MAX` waits forever), *out* `buffer: u32 @16`,
  *out* `replay_full: u32 @20`, *out* `base: u64 @24` (0 for none), *out* `replay_count: u32 @32`,
  reserved `@36`, *out* `replay: [Rect; 16] @40`.
- `Commit`: `id: u64 @0`, `buffer: u32 @8`, `damage_count: u32 @12` (at most 16), `damage: [Rect; 16] @16`,
  `meta: u64 @272` (pointer to `Meta`), `meta_len: u32 @280`, reserved `@284`, *out* `commit_id: u64 @288`,
  *out* `pixel_gen: u64 @296`.
- `NoChange`: `id: u64 @0`, `meta: u64 @8`, `meta_len: u32 @16`, reserved `@20`, *out* `commit_id: u64 @24`.
- `Meta` (128): `processed_serial: u64 @0`, `epoch: u64 @8`, `ui_time_ms: u64 @16`,
  `layout_version: u64 @24`, `real_clock_reads: u64 @32`, `tree_serializations: u64 @40`,
  `rejected_count: u32 @48`, reserved `@52`, `rejected: [u64; 8] @56`, `flags: u32 @120`, reserved `@124`.
- `Resize`: `id: u64 @0`, `width: u32 @8`, `height: u32 @12`, *out* `stride: u32 @16`, reserved `@20`,
  *out* `buffer: [u64; 3] @24`.
- `ReadEvents`: `id: u64 @0`, `ptr: u64 @8`, `capacity: u32 @16`, `flags: u32 @20` (bit 0 block),
  `timeout_ns: u64 @24`, *out* `lost: u64 @32`.
- `WriteEvents`: `id: u64 @0`, `ptr: u64 @8`, `count: u32 @16`, reserved `@20`.
- `Geometry`: `id: u64 @0`, `x: i32 @8`, `y: i32 @12`, `z: u32 @16`, `flags: u32 @20` (bit 0 visible,
  bit 1 minimized, bit 2 focused).
- `MapBuffers`: `id: u64 @0`, *out* `buffer: [u64; 3] @8`, *out* `stride: u32 @32`, reserved `@36`.
- `Capture`: `id: u64 @0`, `dst: u64 @8`, `dst_len: u64 @16`, `tree_dst: u64 @24` (0 for none),
  `tree_len: u64 @32`, *out* `commit_id: u64 @40`, *out* `width: u32 @48`, *out* `height: u32 @52`. Pixels are
  copied as BGRX rows of `width × 4` bytes.
- `MapTree`: `id: u64 @0`, *out* `addr: u64 @8`, *out* `len: u64 @16`.
- `Claim`: `flags: u32 @0`, reserved `@4`, *out* `epoch: u64 @8`.
- `SceneSnapshot` (96): in `context:u64 @0`, `windows:u64 @8`, `window_capacity:u32 @16`,
  out `window_count:u32 @20`; in `damage:u64 @24`, `damage_capacity:u32 @32`, out `damage_count:u32 @36`,
  `snapshot:u64 @40`, `width:u32 @48`, `height:u32 @52`, `scale:u32 @56`, `flags:u32 @60`,
  `Cursor @64`, `claim_epoch:u64 @80`, `owner_sequence:u64 @88`. Flags bit 0 means full initial/recovery scene.
- `SceneWindow` (80): `id:u64 @0`, `sample:u64 @8` (0 for never-published opaque black),
  `commit_id:u64 @16`, `pixel_gen:u64 @24`, `render_fence:u64 @32` (native, no WAIT authority),
  `target_generation:u64 @40`, `x:i32 @48`, `y:i32 @52`, `width:u32 @56`, `height:u32 @60`,
  `z:u32 @64`, `stride:u32 @68`, `format:u32 @72` (0 B8G8R8A8), reserved u32 @76.
- `CompositeSubmit` (112): in `snapshot:u64 @0`, `target:u64 @8`, `stream:u64 @16`, `length:u64 @24`, <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
  `flags:u32 @32` (0 submit, 1 cancel), reserved u32 @36; out `serial:u64 @40`, `pixels:u64 @48`,
  `render_fence:u64 @56`, `present_fence:u64 @64`, `estimate:u64 @72`, `error_index:u32 @80`,
  `error:u32 @84`, `reason:u32 @88`, `fence_status:u32 @92`, `answer:u32 @96`, `phase:u32 @100`
  (0 admission, 1 render, 2 present); in `scene_target:u64 @104`.
  Render/present are context fences accepted by WAIT; per-window native fences are different identities.
  All output/reserved fields start zero. The full argument/error/consumption rules are normative in
  [gpu-abi.md, Window front handoff](gpu-abi.md#window-front-handoff-b13-syscall-414-version-2).
  Window VERSION remains 2 for this coordinated unlanded-v2 change: old op-19 length 24 is EINVAL,
  never a compatibility request; ops above 29 are EINVAL. Syscall 416 stays VERSION 1, ops 0–13 unchanged.
- `RecordQuery`: `serial: u64 @0`, `ptr: u64 @8` (a `CompositeRecord`), `len: u32 @16`, reserved `@20`.
- `Readback`: `dst: u64 @0`, `dst_len: u64 @8`, `record: u64 @16` (an `ObservationRecord`),
  `record_len: u32 @24` (1,024), reserved `@28`, *out* `serial: u64 @32`, *out* `reason:u32 @40`, reserved u32 `@44`.
- `Cursor`: `shape: u32 @0` (0 arrow, 1 to 4 the resize shapes, 5 hidden), `visible: u32 @4`, `x: i32 @8`,
  `y: i32 @12`.
- `Give`: `pid: u32 @0`, reserved `@4`.
- `MetaQuery`: `id: u64 @0`, `meta: u64 @8` (pointer to a `Meta` the kernel fills), *out* `commit_id: u64 @16` (the
  newest commit, `COMMIT` or `COMMIT_NO_CHANGE`, whose metadata it is; 0 before the first).

### 17.2 Ids, limits and lifetime

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
- Window ids, commit ids, composite serials and input serials are 64-bit, start at 1 and are never reused
  within a boot. Every live window operation on a destroyed id is ENOENT, including GPU
  ACQUIRE without publishing a buffer. Retired ops 14/21 remain ENOSYS: they no longer resolve ids.
- A window is 1 to 4,096 pixels on each side (EINVAL outside). A title is valid UTF-8 of at most 64 bytes
  (EINVAL otherwise) and is stored and listed byte for byte. A process may own at most 16 windows; the
  seventeenth `CREATE` is EAGAIN and leaves the first sixteen unaffected.
- A new window's buffers are all zero. `LIST` reports registered windows only; `DESTROY` removes a window <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  from the listing, revokes the owner's GPU buffer handles and unmaps its tree pages. Target
  backing remains held until the last holder and device unref finish.
- Guest buffer backing counts against the owner's `RLIMIT_AS`, device resources against the <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  GPU quota; mapped tree pages remain separately charged. `COUNTERS` reports GPU resources
  and backing exactly. `CREATE`, `REGISTER` and `RESIZE` each have one
  commit point; a failure before it, at any allocation step, leaves the registry, every mapping and the
  accounting exactly as before. A failed `RESIZE` leaves the old buffers intact.
- GPU handles and contexts are not inherited across `fork`, nor are the tree mappings, so a
  child has neither GPU handle authority nor window authority. A process's windows are destroyed at `exec` and at exit.
- Breengel's `Window` destroys its window on `Drop`, and `Window::new` destroys the window it created when
  registration fails.

### 17.3 The commit protocol

Plan §3.1 is the contract, transcribed here.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
A window has two buffers, or three on request, each width × height × 4 BGRX bytes.
A buffer is free, acquired, pending (sealed candidate), published front (immutable), or retired
(replaced but still read by sampling/capture/replay). Published and accepted commits are distinct. Non-empty damage starts a strictly increasing
pixel_gen; damage is at most 16 rectangles covering every differing pixel. The validator
refuses later writes to pending/front/retired targets, and committed tree pages are read-only
(writes raise SIGSEGV).

Keep the existing 304-byte COMMIT layout. The client supplies a buffer index, damage and Meta, **not a fence value it could forge**. Under the same GPU admission lock that seals writes, capture that target's last **accepted** write and its internal outstanding-writer dependencies. This includes an admitted submission whose translation/send is not finished; assign its fence/identity at admission and retain it until published. Looking only at `Resource.last`, or at an empty completed-writer list, is incorrect.

The window layer needs a pending candidate alongside the last successfully published front:

1. Validate/acquire the commit gate, retain the candidate and previous front, seal the candidate against every writing entry point, protect its tree page, and snapshot damage/Meta and accepted render dependencies atomically with that seal. An already accepted writer may finish. No later writer may enter. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
2. If the render has already failed, return EIO without changing the published front, Meta, damage log or successful publication. The owner must fully redraw an invalid target before trying again. The core's ordinary failed-target recovery rule remains. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
3. If the render is pending, COMMIT returns acceptance with its reserved commit_id/pixel_gen. It does not wait for the compositor. The candidate is pending and immutable; the previous published front remains held. A presentation attempt including that window returns EAGAIN without changing the binding. This is required by `windows/commit-waits-render`: its depth-four fault cannot be released until after COMMIT has returned.
4. When all accepted writer dependencies are complete and the candidate is valid with its recorded final render successful, publish its front/tree/Meta/damage in one window transaction. Now retire the previous front. Different device contexts have no implied order; the kernel must establish readiness on the CPU before sending sampling commands. WAIT on just one foreign context's last fence is insufficient when other accepted writers remain. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
5. On a late render failure, reject the pending publication, retain the previous front/tree/Meta, queue EIO for the client's next ACQUIRE, and wake it. The client can also observe the genuine EIO through WAIT on its own render fence. There is no asynchronous success event invented in WindowEvent's 320-byte layout. ACQUIRE delivers the failure once without handing out a buffer; a retry can acquire a safely reusable buffer. A failed or hung candidate is not reusable until actual access ends.

Maintain a monotonic **accepted-commit** sequence: each accepted COMMIT/NO_CHANGE increments it by one; a late-rejected number is never reused. LIST/READ_META expose only successfully published metadata, so may skip a rejected reservation. CAPTURE identifies only a successfully published generation. A synchronous refusal consumes no accepted id. An earlier acceptance return is never retroactively changed to EIO. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Serialize publication order while one candidate is pending. Another COMMIT or NO_CHANGE can wait, killably, for that render decision; it does not overtake it or wait for display composition. Ordinary successful commits retain their exact +1 behavior. No compositor, including no display owner, is needed for publication or recycling. The no-compositor 250 ms case remains measured as written; rendering stalls are governed by the existing render watchdog, not used as an excuse to wait for composition.

NO_CHANGE retains its 32-byte layout, pixel generation and empty damage. It updates published metadata without new drawing or a sampling hold. A completed frame of the same pixel generation satisfies that later metadata commit's presentation requirement; observation records remain honest about the actual commit sampled, rather than rewriting history.

A CAPTURE begun while a candidate is pending waits killably for that render decision, then captures the newly published generation or returns EIO for its failed publication; a later CAPTURE after rejection sees the retained previous front. It never copies pending/partially rendered pixels. Existing CAPTURE/tree-output validation, authority, ENOENT-at-start and pinned whole-generation behavior remain in force. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

- **Bases and the damage log.** A buffer's base is the commit id whose pixels it holds: the id it was last
  committed under, the commit `ACQUIRE` last replayed it to, or none (new, or reallocated by a resize). The
  kernel keeps the damage of each window's last 8 commits.
- **`ACQUIRE` replays in the kernel.** Under the window's commit lock it reads the newest front *F* and takes <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  a replay pin on *F*'s buffer, and computes the replay set: the union of the damage of every commit after the
  acquired buffer's base up to *F* (its bounding rectangle when that needs more than 16 rectangles), or the
  whole window (`replay_full = 1`) when the base is none or a commit in that range has left the damage log.
  It drops the lock, issues GPU region copies of the replay set from *F* into the acquired
  target, waits for their fences, drops the pin, sets the buffer's
  base to *F*'s commit id and returns the buffer, its base and the set. The returned buffer equals exactly the
  committed generation its base names. Nothing is copied when the window has no front or the base is already
  *F*. With no free buffer `ACQUIRE` blocks until a release, the timeout (ETIMEDOUT), a signal (EINTR) or the
  window's destruction (ENOENT). Neither Breengel nor a raw client copies replay data in userspace.
- **Stale commits.** Committing a buffer whose base is older than the current front is accepted: the frame is
  that buffer's whole content, and the kernel widens its damage by the damage of every commit after its base
  up to the previous front, or to the whole window when that history is not available.
- A buffer can be committed once per acquire; a second commit is EBUSY. Two threads of the owner may commit
  concurrently; the kernel serializes them and their commit ids show the order.
- A commit never waits for composition: with no compositor it succeeds within 250 ms, and the frame is
  composed once a compositor claims.
- `COMMIT_NO_CHANGE(meta)` publishes metadata against the current front's pixels: a new `commit_id`, the same
  `pixel_gen`, empty damage, and no composite of its own.
- **Captures** (`CAPTURE`) device-read back the pinned published client target,
  retaining it through the transfer fence and the call's copies. A
  resize or destroy during a capture retires the buffer and frees it after the copy, so a capture returns a
  whole committed generation, or ENOENT when the window was destroyed before the copy began. With a tree
  destination, `CAPTURE` copies the same buffer's tree page under the same pin and returns the one commit id
  both belong to; until that copy ends, `ACQUIRE` does not hand out the buffer at that tree page's index, even
  when a resize has replaced the buffer the capture pinned. SCANOUT_READBACK instead
  pins the completed composed scanout binding (section 20).
- **No pins between system calls.** Snapshot metadata holds no pixel/tree backing and never prevents <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
  ACQUIRE. Sampling, replay and capture pins exist only inside their synchronous calls and the
  device work they submit. With two buffers ACQUIRE may wait for one admitted composite and capture;
  it never returns an in-use buffer, adds a third, or falls back to a CPU copy. Timeout zero with no
  free buffer is ETIMEDOUT. The complete ACQUIRE errors are EINVAL, EFAULT, EPERM, ENOENT,
  ETIMEDOUT, EINTR, ENOMEM (replay resources), ENODEV (core unavailable), EIO (failed replay,
  render/reset, or the one-shot late failure). No failure publishes an output buffer.
- **Remnants.** A root process registered with `HOLD_EVIDENCE` as a window's evidence holder keeps, when the
  owner exits or crashes, the window's last front and tree page as a read-only remnant: outside the dead
  owner's pool, counted against the holder's `RLIMIT_AS`, at most 16 per holder, read with `READ_REMNANT` and
  freed by `RELEASE_REMNANT` or the holder's exit. Nothing is retained while the owner lives.

### 17.4 Authority

Principals: the **owner** (by kernel identity, never by PID number, so PID reuse grants nothing), the
**compositor** (the holder of the current claim), **root** (uid 0) and an **unrelated** process.

| Operation | Owner | Compositor | Root, not compositor | Unrelated |
|---|---|---|---|---|
| `LIST` | windows whose owner has the caller's effective uid | registered registry | registered registry | windows whose owner has the caller's effective uid |
| `CREATE`, `REGISTER`, `SET_TITLE` | yes | — | — | — |
| `ACQUIRE`, `COMMIT`, `COMMIT_NO_CHANGE`, `DESTROY` | yes | EPERM | EPERM | EPERM |
| `RESIZE` | yes | EPERM (it asks with a `ResizeRequested` event, section 19.2) | EPERM | EPERM |
| `READ_EVENTS`, `EVENT_FD` | yes | `EVENT_FD` of window 0 only | — | EPERM |
| `WRITE_EVENTS`, `SET_GEOMETRY` | EPERM | yes | EPERM | EPERM |
| retired `MAP_COMMITTED`, `MAP_COMPOSITOR_TEXTURE` | ENOSYS | ENOSYS | ENOSYS | ENOSYS |
| `CAPTURE`, `MAP_TREE`, `READ_META` | yes | yes | yes | EPERM |
| `COMPOSITE_SNAPSHOT`, `COMPOSITE` | — | only while also current display owner | EPERM | EPERM |
| `COMPOSITE_RECORD`, `SET_CURSOR` | — | yes | EPERM | EPERM |
| `CLAIM_COMPOSITOR` | — | — | yes if no live claim, else EBUSY | EPERM |
| `SCANOUT_READBACK` | — | yes | yes | EPERM |
| `HOLD_EVIDENCE`, `READ_REMNANT`, `RELEASE_REMNANT` | EPERM | EPERM | yes (EBUSY if another holder) | EPERM |

`LIST` filters both its output and its returned count by the window owner's current effective uid.
The holder of the compositor claim and root see the registered registry. A different user sees neither
the window's entry nor its contribution to the count, including a zero-capacity sizing query (B46).
This grants no authority to write events, set geometry or send a resize request: those remain compositor
operations, and the owner answers a compositor's `ResizeRequested` event through `RESIZE` (B41).

### 17.5 Pointers, errors and quiet failure

- Every op copies through `copy_from_user` and `copy_to_user`. A range that overflows, is unmapped, crosses <!-- claim-lint:ok: Normative pointer-copy and COMPOSITE exception rules in gpu-abi.md Window front handoff. -->
  into an unmapped page or is read-only for output is EFAULT, and an op that fails with EFAULT has no side
  effect for preflight failures: copies in finish before state changes, output ranges are preflighted.
  For COMPOSITE, a concurrent unmap after sending GPU work is the explicit exception: return EFAULT
  without successful acknowledgement, retain DMA holds and damage, and force full next-frame repair.
  A pre-existing unmapped/read-only descriptor or output still sends no work and consumes no frame. A read of events
  into a bad buffer consumes no event. An output landing in a copy-on-write page after `fork` lands only in
  the caller's copy.
- No window op writes to the serial log or any kernel output on any path (section 22).
- Destroying a window while another thread commits it ends with exactly one of the two outcomes and no fault.
- A blocked `ACQUIRE` or blocking `READ_EVENTS` returns within 250 ms of a SIGKILL, a destroy or a compositor
  death. A blocking `READ_EVENTS` that began while a compositor held the claim, on any window, returns 0 events
  when that claim ends (its holder's death or release) with no event queued. A timed wait returns no earlier
  than its timeout and no later than the timeout plus 100 ms.

### 17.6 Retired operations

On ARM64, system calls 410 (`fb_info`), 411 (`fb_draw`, including FLUSH and
FLUSH_BATCH), 412 (`fb_mmap`) and the version-1 drawing/window operations are retired
under B42 and return ENOSYS. x86-64 keeps syscalls 410–412 and its framebuffer console
path until it has a GPU core. V2 MAP_COMMITTED/MAP_COMPOSITOR_TEXTURE remain retired
ENOSYS. The ARM64 wait-stress program and init gate retire with their v1 subject.
The compositor's keyboard state, once version 1's ops 26 (modifiers) and 31
(the latched Super presses), is the 8-byte form of `get_mouse_pos` (413): `[modifiers, super_taps]` as
`u32`s, where the 16-byte form is the pointer's `[x, y, buttons, scroll]`; any other length is EINVAL.

## 18. Compositor, composition and the display

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
- **Claims and epochs.** `CLAIM_COMPOSITOR` gives root the compositor role with an epoch that increases at
  every claim; a second claim while one lives is EBUSY; uid 1000 is EPERM. When the holder dies or releases,
  the role can be claimed again with a higher epoch. Windows outlive the compositor with their last geometry,
  stacking and visibility. Each claim delivers `CompositorChanged { epoch }` to every window (section 19).
- **Composition.** `COMPOSITE` composes every visible window at its geometry, in increasing `(z, window_id)` (later over <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  earlier), over black, by the current owner+claimant in userspace on the GPU core onto the persistent scene target, clipped at its edges, exactly: each output pixel
  is the topmost visible window's committed pixel there, or black. It processes exactly the pixels of the
  damage union since the previous composite, and the full screen the first time; the frame equals a full
  recomposition. At most one composite per window is in flight.
- **Composite records.** `COMPOSITE_RECORD(serial)` reads `CompositeRecord` (24-byte header `serial: u64 @0`, <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
  `pixels: u64 @8`, then `count: u32 @16`, `flags: u32 @20`, then `count` entries of 32 bytes: `id: u64`,
  `commit_id: u64`, `pixel_gen: u64`, `pixels: u64`). Every presentation-required window (registered,
  visible, not minimized) is listed with the generation the frame is correct for, a wholly occluded one with
  `pixels = 0`. A commit is presented when a composite with its readback completion lists its window at a
  `pixel_gen` at or above the commit's.
- **Failure.** A backend failure leaves the commit uncomposited, fails `COMPOSITE` with EIO and the record's
  reason, and the next composite retries it. The failed composite keeps its serial and has a record: `pixels`
  0, no entries, and `flags` bit 0 set with the backend's reason code in bits 8 to 15.
- **Cursor.** The compositor draws the cursor sprite above every window at its position. Moving it over a
  static scene restores every pixel it covered; hidden, no cursor pixel appears; content committed under a
  stationary cursor is shown with the cursor redrawn above it. Shapes 0 to 4 are the arrow and the four
  resize shapes.
- **Display ownership** is a stack: `take_over_display` (431) pushes the caller as owner, `give_back_display`
  (432) pops it, and `GIVE_DISPLAY(pid)` hands it to another process, which becomes owner above the giver. <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  When an owner dies the display returns to the process that gave it, never to the runner, and the previous
  owner's frame is restored exactly. Direct take-over requires uid 0; explicit GIVE_DISPLAY retains its
  owner authority. GPU targets and their backing are retained on the ownership stack; no
  CPU framebuffer snapshot or firmware copy replaces a saved target. The core's ownership
  depth and cycle refusals stand, and release restores the prior binding and exact pixels.
- **The display.** `MODES` reports 1280×800 at scale 1 and 2560×1600 at scale 2. This suite <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  runs at scale 2. Clients own GPU targets; bwm samples committed targets, draws the cursor
  as a top GPU layer and presents/flips a scanout target. No CPU compositor, display copy,
  fb_info, fb_mmap or FLUSH remains. Existing 1280×800 scene fixtures retain their exact
  pixel formulas at their stated coordinates; dedicated scale-2 fixtures double logical
  placement and compare every pixel of the 2560×1600 destination. `composition/scale-2`
  uses logical rectangles (40,60,128,96), (100,100,160,80), and (-20,780,100,40), in
  increasing z order, with source-window patterns 11, 12, and 13. Each logical-size
  source texture is a declared input fixture; COPY_SCALED with scale 2 draws it into
  the physical client target. No expected composite is uploaded. The fixture allocates
  targets at twice each width and height and sends
  twice each position through SET_GEOMETRY (a physical-pixel interface). Its oracle
  uses the unchanged scene formula at (physical_x/2, physical_y/2), including source
  pattern coordinates and opaque black outside windows.
  This fixture is distinct from the existing physical-pixel pattern scenes.
- **Lifetimes.** Handle destruction does not end holds by a queued submission, committed <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  front, remnant, compositor sample, scanout binding, capture/readback or replay copy. Each
  holds backing through actual completion or confirmed reset; a reported error alone never
  proves device access ended. The last hold and device unref return the charge exactly.
- **Numbers** (plan §12.2): one 256 × 256 window committing as fast as its buffers are released is composed at
  least 60 times a second (300 commits, the first 30 discarded); the longest IRQ-masked interval in the
  GPU path (validation, translation, submission, fence completion and readback copies) during full-screen damage is at most 500 µs. `composite_pixels` and record/per-window pixels count the union of actual clipped scene writes
  in persistent C once per destination pixel, excluding overdraw and the full C-to-A/B scanout
  transport copy. Per-window pixels intersect that union with the topmost visible region.
  Ordinary GPU COUNTERS draws, instances, fragment area and work estimate include the transport
  and its quota charges. The counters: trace counters `composites`,
  `composite_pixels`, `damage_rects`, `window_buffer_allocs`, and the gauge `gpu_irq_masked_max_us`, in
  `/proc/trace/counters`.

### 18.1 Snapshot authority and source aliases (B13)

There are three independent identities: window client, compositor claimant, and display owner. They are kernel process-incarnation identities, never a numeric PID comparison. A frame also names the claim epoch, display ownership sequence and device reset epoch. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Only a caller which is **both** current display owner **and** current compositor claimant may snapshot or submit foreign window fronts. Root alone cannot. A root claimant below the top of the display stack cannot. Merely receiving GIVE_DISPLAY cannot. The ordinary GPU validator continues to refuse explicit foreign resources with EINVAL, code 4.

A snapshot contains every presentation-required window: registered, visible, not minimized, including wholly occluded windows. Order is `(z, window_id)` ascending, matching the registry tie break. Each source alias names `(snapshot, window incarnation, buffer UID, committed pixel generation, resource epoch, write incarnation)`. The write incarnation advances whenever ACQUIRE permits modification, even before a render completes; reusing one of the client's two handles can never resurrect an alias. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Aliases use the non-address-shaped handle encoding, are private to the snapshot's process and named context, and are never inserted as ordinary writable resources in `Proc.resources`. Allocate them from the same non-reusing handle sequence, checking the context/resource/snapshot/alias tables for collisions; context/resource allocation must also check outstanding alias values. A cancelled value never later resolves to an unrelated resource. They can appear only in source positions in the op-19 stream: COPY source, DRAW_SAMPLED texture, or a supported parameterized pipeline's source texture. They cannot be BEGIN targets, COPY destinations, geometry buffers, UPLOAD destinations, PRESENT targets, READBACK handles, or handles re-exported to another process. Ordinary 416 SUBMIT does not resolve them. Its refusal is EINVAL/code 4, even for the owner. RESOURCE_DESTROY on an alias is EINVAL; cancellation uses op 19. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Device attachment is temporary and specific to the importing context. The translator resolves the authorized source alias to its internal resource ID, attaches it before use, and supplies a sample-only binding. Attachment itself is not authority: the validator must refuse every other route, including “the resource happens to be attached.” Never attach all clients' resources to all contexts of the owner. Clients retain backing ownership and quota charges. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

### 18.2 Admission, holds, scene damage and transport (B13)

The snapshot contains no strong pixel/tree reference. On op 19, use the window commit protocol to compare source UID, write incarnation, resource/reset epoch and published generation, require that it is still the window's current published front, then pin those exact buffers before dropping the lock. If an intervening pixel commit replaced a source, the snapshot is stale: EAGAIN, never a free buffer pinned while ACQUIRE can hand it out, or a new generation sampled under an old record. A metadata-only NO_CHANGE does not change the sampled buffer or invalidate its pixel authority; retain the snapshot's honest sampled commit metadata. Scene snapshots are retried; it does not block clients during userspace command construction. Cache/prepare command geometry before snapshot acquisition where possible. Achieving the existing 60 Hz case is an implementation gate, not an assumed consequence of retries or a reason to loosen that bound. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

For an admitted frame, transfer these call-local pins to submission-owned holds before any attach/draw can access them. Pending client rendering causes EAGAIN before device composition. A wholly occluded window remains in the manifest; a never-committed window contributes opaque black. Pins for sources not actually sampled can end after validation/admission; entries and generation provenance are immutable metadata. A source that is sampled stays held through actual sampling completion **and fence publication**. Detach safely after outstanding requests, and release the pool pin at the same completion boundary. Do not hold client fronts until scanout replacement: scanout holds the owner's composed target, not the clients. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

With two client buffers:

* F is front, B is free. ACQUIRE may hand out B after replaying from F with a replay pin/fence. F remains sealed; a compositor can concurrently read F.
* B commits. If rendering is pending, F remains the last published front and B is an immutable candidate. Once B publishes, F is retired if a sampling/capture/replay hold still reads it; otherwise F becomes free.
* With F still sampled and B front, there is no free buffer. ACQUIRE waits; it must not unseal F, return the front B, allocate a surprise third buffer or copy pixels into CPU memory. When F's final pool hold ends, ACQUIRE replays B into F on the GPU, waits for that copy, sets base to B's commit and only then unseals F and its tree page.

With three buffers the third can be acquired while an earlier front is retired, but the same rules apply when all three are front/pending/held/acquired. At most one composite is admitted at once, a stronger implementation of the existing per-window bound, and every wait is interruptible. With timeout zero and no free buffer, existing ACQUIRE returns ETIMEDOUT, not an invented EAGAIN. Its complete errors are the existing EINVAL/EFAULT/EPERM/ENOENT/ETIMEDOUT/EINTR plus ENOMEM for replay resources, ENODEV for unavailable GPU, and EIO for failed replay/render/reset or the late-failure delivery above. No output buffer is published on any failure. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Cancellation before send, signal before send and the pre-device `composite-stall` death case release pool pins immediately. After send, an EINTR, EIO, kill, dropped handle or synthesized watchdog status does not prove DMA ended. Keep backing and pool exclusion until device completion and fence publication or **confirmed successful reset**. A failed reset retains them. On owner loss wake blocked ACQUIREs within 250 ms; if safe recycling is still impossible because device work has not drained, return EIO instead of granting a writable target under DMA. The existing compositor-dies case stalls before sending, so it must still acquire successfully within its written 250 ms bound. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Replay and CAPTURE retain their independent pins, including tree-index exclusion during capture across resize. The damage log of eight commits, stale-base widening, 16-rectangle replay limit and full-replay fallback remain unchanged. Keep buffer UIDs independent of array indices; a resize can install a new set while old buffers remain in flight.

### Damage, flipping and accounting

Use a persistent GPU scene target C and two display-sized scanout targets A/B. Initialize all three with completed GPU clears before the measured composition interval, so the alternate scanout is a real, readable target for point 26 even on the first flip. Userspace updates only the normalized scene damage in C, restoring black/windows and the cursor there, then emits a GPU COPY of C into the unused A/B and presents it. The persistent scene avoids assuming that the other flip target already has the previous frame's unchanged pixels. It also avoids CPU snapshots or a full window recomposition on every flip. There are still exactly two **scanout** targets; C is an ordinary render target within the owner's quota. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Op 19 validates that alias-sampling scene draws write C, that the displayed destination is made from that completed scene by the submitted full GPU copy, and that other writes cannot race those resources. It reuses the BGL stream validator/translator; it does not draw either the scene or copy on behalf of userspace. The full scene-to-scanout COPY is presentation transport. `composite_pixels`/per-window pixels count the union of actual scene writes in C, once per destination pixel, not overdraw or the transport copy. All GPU work including the transport is counted in ordinary COUNTERS draws/instances/fragment area/work estimate and charged normally. State this distinction in §18's counter definition; do not conceal the copy's GPU cost. An implementation can later avoid the extra target/copy only if it preserves the same damage and flip properties. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

The first scene processes the full reported mode: 4,096,000 pixels at 2560×1600. Later record pixels derive from the validated stream's actual clipped scene write union, not the requested damage or a caller's claimed total. Per-window pixels are that union intersected with each window's topmost visible region; fully occluded windows have zero. The ignore-damage fault must cause actual excess scene work and an excess counter, not a falsified counter. It must not be “optimized away” by an admission check comparing work against the intended damage. Kernel validation is not a pixel oracle: suppress-draw/corrupt-pixel faults can complete with apparently valid provenance and must still be caught by device-pixel comparison. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Retain snapshot damage/publication watermarks until successful completion; acknowledge only the covered revisions, never clear damage committed during the in-flight frame. Cursor motion damages old and new footprints; stationary repaint redraws the cursor above new content. Failure retains damage and forces full scene repair. Direct writes outside this composition path invalidate the target's composition provenance. Scanout readback serializes against writers and pins a known binding throughout both poison transfers, so it cannot pair old metadata with newly overwritten pixels. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Client backing is charged once to the client (or its retained accounting identity after exit), not again for aliases or sampling. Compositor targets, commands, private submission copies and observation staging are charged to the compositor/observer under normal core rules. Charges return only after the final hold and completed device unref, or confirmed reset. B12's exact VmSize expectation and bounded wait for unrefs stand.

### 18.3 Teardown and core display custody (B13)

| Event | Required result |
|---|---|
| Client RESOURCE_DESTROY | Revoke its public handle. The window's retained target still supports committed-front sampling, replay and CAPTURE. A snapshot resolves the window's internal identity, not the dropped handle. |
| DESTROY | Remove registry visibility, invalidate unadmitted snapshots naming the window (op 19 ENOENT), revoke client handles and unmap tree pages. An already pinned frame/capture may complete its whole old generation. Damage the uncovered area for the next frame; free backing only after all holds/unref. |
| Client last-thread exit or exec | Run window cleanup and GPU handle cleanup exactly once per process incarnation. Same invalidation/draining rules as DESTROY; events/readers wake. On exit/crash only, an authorized evidence holder can retain the last successfully published front/tree as a remnant, up to the existing 16-holder bound. Pending failed data is never evidence. Exec creates no remnant. | <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
| Evidence holder exit/release | Drop remnant holds; charges follow existing holder RLIMIT_AS semantics. A remnant is not a sampleable live window or a route around compositor authority. |
| Compositor claim release/death | Invalidate its snapshots/aliases and wake window waiters. Windows retain geometry, visibility, stacking and last published fronts. Pre-send pins drain immediately; sent GPU work drains safely. A later root claim gets a strictly higher epoch and CompositorChanged to every window. | <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
| Display owner giveback/death/exec | Revoke that owner's foreign-sampling authority immediately, including its snapshots. Remove its stack entry, restore the previous live giver's mode and retained GPU target through a completed flush, or the kernel screen if none. A surviving claim does not bypass lost display ownership. Do not restore a CPU framebuffer copy. | <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
| Device reset | Invalidate snapshots by reset epoch; fail pending renders and submissions with the core's genuine reset status. Confirmed reset releases DMA access holds, loses old resource contents/bindings, and requires recreation/full redraw. Unconfirmed reset frees no DMA backing. Client operations against lost targets return EIO; clients destroy/recreate their windows/contexts, register the new monotonically allocated window IDs and fully redraw. The window-target context cache must discard reset/quarantined entries before creating new targets. No resize/replay can recover lost pixels. Owner recreates C/A/B/context and re-enumerates new windows. A lost retained screen cannot be restored “exactly”; report EIO/target-lost and redraw after recovery, never fabricate old pixels. | <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

Locks must not nest registry/core/process-manager/device waits. Use retained identity objects and a validate/reserve/recheck protocol; drop Pin objects outside the registry lock as the lock protocol requires. Display transition sequencing must prevent a delayed old-owner present from binding after a new owner takes custody. Once bind/flush is sent, it is serialized with takeover/pop; device-access holds survive changes of owner.

**GIVE_DISPLAY is 414/27**, existing 8-byte `pid:u32 @0, reserved:u32 @4`. Implement it in `gpu/display.rs` using the core ownership stack, not the removed display backend or v1 DISPLAY_OWNER_PID. Only the current owner may give; resolve the target PID once to its live incarnation, push it directly above the giver, retain giver mode/binding/backing. Root is required for direct 431 takeover, not for explicitly delegated custody. A delegated uid-1000 owner can draw/present its own resources and give back; it cannot claim the root-only compositor role or sample foreign fronts. No claim transfers implicitly with GIVE_DISPLAY.

GIVE errors: EPERM non-current-owner; EINVAL malformed size/reserved; EFAULT argument copy; ESRCH absent/exiting target; EBUSY self-give/cycle/already-in-stack target or depth five; ENODEV unavailable core; ENOMEM if fallible bookkeeping cannot be allocated; EINTR while waiting for a transition before push; EIO failed device transition. Validate before mutating custody. Four userspace owners maximum, excluding the kernel. Direct TAKE by the current owner remains idempotent; TAKE by a lower owner remains EBUSY. GIVE does not collapse the stack to the runner. Pop waits for a genuine restore flush; if restore fails keep binding unknown and follow the core's restore-retry/rollback rules. Repeated PID values do not revive expired authority.

The claim is singleton, not an ownership-stack entry. The stage normally needs no claim to render its own stage. It releases any claim before delegating a window test to kiosk; kiosk claims and takes delegated display ownership before announcing ready. A root stage that itself composes client windows must hold the claim while active and use the same APIs. Restoring display custody never silently restores a departed claim epoch. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->

## 19. Window events, version 2

### 19.1 Layout

`WindowEvent` is 320 bytes:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `serial: u64` | the input serial (section 19.3) |
| 8 | `time_ns: u64` | timestamp |
| 16 | `epoch: u64` | the new epoch, `CompositorChanged` only |
| 24 | `kind: u16` | the type below |
| 26 | `flags: u16` | bit 0: a repeat |
| 28 | `modifiers: u32` | bit 0 Shift, 1 Ctrl, 2 Alt, 3 Super |
| 32 | `keycode: u32` | Linux key code |
| 36 | `button: u32` | Linux button code: 0x110 left, 0x111 right, 0x112 middle |
| 40 | `x: i32` | window-local pointer x |
| 44 | `y: i32` | window-local pointer y |
| 48 | `scroll_x: i32` | signed horizontal steps |
| 52 | `scroll_y: i32` | signed vertical steps, positive up |
| 56 | `width: u32` | new width, `Resized` and `ResizeFailed`; the width asked for, `ResizeRequested` |
| 60 | `height: u32` | new height, as `width` |
| 64 | `pressed: u32` | 1 press, 0 release, `PointerButton` only |
| 68 | `text_len: u32` | bytes of `text`, at most 240 |
| 72 | `lost: u32` | events lost before this one, `Overflow` only |
| 76 | reserved | zero |
| 80 | `text: [u8; 240]` | UTF-8, `KeyPress` (the text the compositor attached) and `Text` |

<!-- claim-lint:ok: Normative event kinds specified by coordinator B41 and #2312. -->
Kinds: 1 `KeyPress`, 2 `KeyRelease`, 3 `Text`, 4 `PointerMove`, 5 `PointerButton`, 6 `Scroll`,
7 `FocusGained`, 8 `FocusLost`, 9 `CloseRequested`, 10 `Resized`, 11 `ResizeFailed`, 12 `FontChanged`
(made by Breengel, section 9.3), 13 `CompositorChanged`, 14 `Overflow`, 15 `ResizeRequested`. No field carries another field's
meaning. Breengel maps an unknown kind to `Event::Unknown { kind }`, never to a key press.

### 19.2 Delivery

<!-- claim-lint:ok: Normative request validation specified by coordinator B41 and #2312. -->
- `WRITE_EVENTS` (compositor only) queues events on one window's ring of 64. A kind outside 1 to 15, the kind
  Breengel makes (`FontChanged`), a kind the kernel makes (`Resized`, `ResizeFailed`, `CompositorChanged`), a `ResizeRequested` whose width or height is outside 1 to 4,096 (section 17.2) or a
  `text_len` above 240 is EINVAL, and nothing in that call is delivered.
  Writing to a destroyed window is ENOENT.
<!-- claim-lint:ok: Normative input admission and overflow contract adopted by B48 for #2339. -->
- Events arrive in the order written. `WRITE_EVENTS` returns the queued prefix's count. A full ring returns
  zero without changing its contents or counting the refused events as lost. The compositor retains the
  unqueued remainder, including keys, pastes and Close requests, in a bounded per-client backlog and retries
  as the client drains. Only exceeding that bound drops the oldest events; the compositor then writes
  `Overflow { lost: N }` to tell the client how many it dropped. A compositor-written Overflow is writable
  and reaches the client like other events.
- An event for one window wakes only that window's readers. `EVENT_FD` is pollable and reports POLLIN when an
  event is queued.
- `EVENT_FD` of window 0 gives the compositor its scene descriptor (ENOENT to any other caller, as for an id
  that names no window). It is pollable and reports POLLIN while the scene has changed since the descriptor
  was made or last read: a window op that can change what the next composite shows (the ops other than `LIST`, `ACQUIRE`,
  `READ_EVENTS`, `WRITE_EVENTS`, `EVENT_FD`, `CAPTURE`, `MAP_TREE`, `COMPOSITE_SNAPSHOT`, `COMPOSITE_RECORD`,
  `SCANOUT_READBACK`, `READ_META`, `READ_REMNANT` and the retired ops), a commit's publication or rejection,
  and an owner's exit. A read of at least 8 bytes returns the running total of changes as a `u64` and marks that total read;
  a shorter read is EINVAL. A compositor sleeps on it and uses `COMPOSITE_SNAPSHOT`'s sizing query to learn
  whether there is damage, rather than polling.
- `Resized` arrives only after the buffers have the new size and carries it; a refused resize delivers
  `ResizeFailed` with the old size and never `Resized`.
<!-- claim-lint:ok: Normative owner-mediated resize behavior specified by B41 and #2312. -->
- `ResizeRequested` (ruling B41) is how the compositor resizes a window, for example when the user drags its
  frame's edge: it asks the owner for content of `width` by `height`. Delivering it changes nothing in the
  kernel. The owner answers by resizing its own buffers with `RESIZE` and redrawing; the size it gives
  reaches the compositor as the window's size in `LIST`, and `RESIZE` marks the scene changed. An owner that
  does not answer keeps its buffers and its size. The compositor frames every window at the size `LIST`
  reports, so a client that does not answer is shown whole at its own size, never cropped to the size asked
  for. Breengel delivers the request as `Event::ResizeRequested { width, height }` and never resizes on its
  own; a later request replaces an earlier one, so an owner may answer only the newest it has read.

### 19.3 Serials

The compositor assigns each event a serial that increases strictly per compositor and is never reused.

### 19.4 Dispatch and the processed watermark

Plan §6.4 is the contract. Breengel apps take events with `window.dispatch(handler)`; the handler returns <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
`Redraw` or `NoChange`, and a serial counts as handled once its handler has returned. For `commit(buffer)`,
`processed_serial` is the highest serial whose handler had returned when that buffer was acquired: an event
whose handler returns after frame N was acquired is acknowledged by frame N + 1, never by N, and an event
polled but not yet handled is never acknowledged. For `commit_no_change`, it is the highest serial *s* such
that every serial up to *s* handled since the last pixel commit returned `NoChange`: a `Redraw` stops it until
a frame acquired after its handler returned is committed. Serials Breengel refused at dispatch are listed in
`Meta.rejected` and count as finished. Raw clients set the watermark themselves. `READ_META` reads back the
metadata the newest successfully published commit owns, with that commit's id. Pending acceptance
does not advance this watermark; a rejected accepted id can be skipped. There is no asynchronous
render-failure event: late EIO is delivered once by ACQUIRE, with WindowEvent still exactly 320 bytes.

## 20. Scanout observation

### 20.1 `SCANOUT_READBACK`

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
Root or the current claimant observes the whole composed destination produced by the userspace GPU
compositor through the core's pinned-binding observation service, as canonical BGRX rows and the same **1,024-byte record version 2** as GPU READBACK.
The complete little-endian offset table is normative in `gpu.md` §1.8 and `gpu-abi.md`.
It includes version/size, backend 4 or 5, boundary 4, scanout/composite flags, physical rectangle,
stride/format, actual resource identity, generation, native render/readback/present fences
and statuses, complementary-poison result, composite serial and the 16 window entries
(window, commit, render fence and generation). All reserved bytes are zero.
The record and pixels are atomic; no sampled band represents the destination. More than
16 windows is composite-overflow EIO without partial record or pixels, not a global
window limit. Composition and COMPOSITE_RECORD still include every window.
A pending composition is EAGAIN. Core EIO reasons are never interchanged; a failed GPU
READBACK writes only its argument's reason, with destination and record untouched.
Window capture normalizes opaque X to FF. The observation of a flip names that flip's
actual bound resource, completed present fence and composite, rather than assuming a fixed
resource across frames. Device readback of target 0 remains display-owner-only; a root
window-protocol request is authorized by the window observer check and serviced by the core,
without importing a foreign handle, a daemon request/response protocol, or another process
accessing the observer's user pointers. The helper's gpu-scanout command is an alternative
transport of the display owner's genuine 416 observation.

The 48-byte argument returns an observation serial, distinct from the composite serial. <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
Errors: EPERM unauthorized observer; EINVAL layout/reserved/record length/short destination;
EFAULT inaccessible outputs; ENOMEM charged staging; ENODEV unavailable core; EINTR interruption;
EAGAIN pending composition; EIO with reason 1–12. Unknown binding without a pending composition
is EIO/10. On EIO write only reason: descriptor serial, pixels and record remain untouched.
Core preflight/concurrent-unmap guarantees apply. A destroyed client does not cause ENOENT
when its retained composed destination is still observable.
Reason 11 is composite-overflow; reason 12 is window-render-not-ready (render not complete OK
before the sampling send), never normal pending EAGAIN. Reasons 1–10 retain their core meanings.
Record target is 0; window entries hold native render fences and sampled core target generations,
not window pixel_gen (which remains in COMPOSITE_RECORD). Unused entries and bytes 776–1023
are zero. Only successful device observation listing a sufficient pixel generation acknowledges
presentation; snapshot/provenance metadata or an op-19 success alone cannot certify pixels.

### 20.2 GPU backend rows

| Target | Backend | Device observation | Completion | Boundary |
|---|---|---|---|---|
| ARM64 QEMU; later Parallels catch-up | 4, VirGL | SUBMIT_3D, two TRANSFER_FROM_HOST_3D observations under complementary poisons, cache invalidation | native kinds 0x207 and 0x206, status 0x1100, nonzero render/readback/present fences | 4, host renderer and window outside guest view |
| Later VMware catch-up | 5, SVGA3D DX | DX_DRAW, host SURFACE_COPY to dedicated readback surface/MOB, two poisoned READBACK_GB_SURFACE observations | render 1152, readback 1104, copy 1042; status 1; dedicated nonzero surface/MOB | 4, host renderer and window outside guest view |

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
Backend must equal CAPS and the window's backend. Backend 4 has zero VMware copy fields;
backend 5 has its own native values and a readback surface distinct from the judged resource.
`unwritten` is zero on success. Flags contain only scanout bit 0 and composite bit 1;
a composed scanout sets exactly both. Legacy CPU backends and boundary codes are retired.
The effort runs ARM64 QEMU `--gpu-gl` only (B5); no x86 QEMU evidence is admitted (S23).

### 20.3 Planted faults

Each case requires exactly one firing, the observable fault, and an exact clean next
composition after disarming, through the core's points:

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
| Case | Old point → core point | Exact observation |
|---|---|---|
| fault-skipped-transfer | 6 → 19 | EIO, unwritten reason 1; pixels and record untouched |
| fault-wrong-resource | 7 → 26, wrong-scanout | PRESENT completes; observation names the actual wrong binding/composite and different pixels; compare it to the intended composite, never invent an EIO from knowing the fault fired | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| fault-stale-fence | 8 → 20 | EIO, fence-mismatch reason 2; pixels and record untouched |
| fault-outside-damage | 9 → 22 | device pixel (0,0) is outside the damaged window; full-destination comparison finds it while every in-damage pixel stays exact | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->

Arm point 22 after the client render and before GPU composition, so it plants on the
composed target. Point 26 needs another live scanout target; absent that target the point
stays armed and the case fails. A wrong transfer resource (point 21) is not the binding
negative control: the device reports no resource identity and it is observed as unwritten.

## 21. Suite-mode test controls

`test_control(op: u32, arg: *mut u8, len: usize) -> isize` is system call **415** on both architectures.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
- **The latch.** The kernel enables test controls only when the first program it executes as PID 1 is a suite
  binary (`/sbin/suite-*`); it latches that fact at that exec, before any user code runs, and nothing changes
  it for the rest of the boot. In every other boot every op returns ENOSYS. In a suite boot only root may use
  them (EPERM otherwise).
- Op 0 `STATE` returns 1 (enabled). Op 1 `ARM { point: u32 @0, reserved @4, target: u64 @8, param: u64 @16 }` <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  arms one one-shot fault; op 2 `DISARM_ALL` disarms every point and restarts the gauge
  `gpu_irq_masked_max_us` at 0; op 3 `FIRED { point: u32 @0, reserved @4, out count: u64 @8 }`
  reports how many times a point fired. Arming or reading a point this section does not define is EINVAL. There is no op that sets or clears the latch: op 4 `SET_LATCH` and
  every op above 4 are EINVAL.
- Points: 1 `ignore-damage` (the next composite processes the whole window), 2 `irq-hold` (the next GPU <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
  SUBMIT validation holds IRQs masked for 2 ms), 3 `emit-diagnostic` (param: the writer path, section 22), 4
  `acquire-stall` (target: a window; the next `ACQUIRE` pauses after copying half its replay rows until
  released or 1 s), 5 `backend-error`; old scanout points 6–9 are retired in favour of core points 19,26,20,22; 10 `panel-cursor-no-restore` (the run stage's panel cursor stops restoring the pixels it covered, from its next move until the stage's next frame, §23.9), 11 `release-stall` (resumes a paused point),
  16 `composite-stall` (target: a window; the next composite that includes it pauses before the backend
  composes, with every window's front pinned, until released, a signal reaches the compositor, or 1 s), 17
  `move-pointer` (param: x in bits 0 to 31, y in bits 32 to 63; the next time the kernel reads the pointing
  device's position, for the panel cursor (system call 413, which the run stage reads), the position is (x, y), as if the device had moved there), 18
  `capture-stall` (target: a window; the next `CAPTURE` of it pauses after copying half its rows, with its pins
  held, until released or 1 s). Points 12 to 15 are the run stage's (section 23). Faults are branches in the
  real code paths, disarmed at case end by the suite. GPU points 19–31 have the exact
  definitions in `gpu-abi.md`, including point 31's pending-fence depth mode used by the
  render-handoff check. All watchdog and kill bounds remain unchanged.
- `init`, running as PID 1, checks that every op returns ENOSYS and prints `[init] test controls absent`; run
  as a child (in a suite boot) it prints `[init] not PID 1: test-control check skipped` and makes no absence
  claim.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B35 specifies these requirements. -->
The userspace run stage consumes its one-shot points 10 and 12–14 by reading
`/proc/breenix/stage-control`: a decimal bitset, bit `point` set when that point fired for
this caller's process id (or target 0). Each open snapshots and consumes the points once.
It is ENOSYS outside suite boots and EPERM for non-root; it never changes the latch.
`/proc/breenix/boot-ribbon` lists the actual producer markers in ARM64 boot-path order,
one `0|marker` or `1|marker` line per applicable stage. Announcements do not reach stages.

## 22. Kernel output accounting

Plan §12.1 is the contract. Each thread's `kernel_output_bytes` counts the payload bytes it commits for
output at exactly one accounting boundary per byte, read as decimal text from
`/proc/<pid>/task/<tid>/kernel_output`. Bytes produced where the current thread cannot be read safely (before
per-CPU data, panic and double-fault paths, `emergency_print`) count in the trace counter
`kernel_output_unattributed`. Drains never count; a notice counts toward the thread that posted it; lead
newlines are not payload.

`emit-diagnostic(path)` makes the calling thread's next window op write one fixed 32-byte payload,
`BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD`, through the named writer: 1 `serial_println!`, 2 `log::warn!`,
3 `write_byte`, 4 `write_bytes_atomic`, 5 `try_print`, 6 `write_str_bounded`, 7 `_log_print`, 8 a notice
posted by this thread and written out by another, 9 `emergency_print`. Each adds exactly the bytes its writer
commits, once: 32 for `write_byte` (32 one-byte records), `write_bytes_atomic`, `try_print`,
`write_str_bounded` and `_log_print`, which write the payload as it is; 33 for `serial_println!` and the
notice, which end the payload with a newline (a notice's line is posted with its newline, and it counts when
posted); the logger's whole record for `log::warn!`; and for `emergency_print`, 32 to
`kernel_output_unattributed` and nothing to the thread. The logger's record is `[ WARN] test_control: `
followed by the payload and a newline: `[`, the level right-aligned in five columns, `] `, the target
`test_control`, `: `, the payload, `\n`, which is 55 bytes. A newline a writer adds is output like any other
byte; only the lead newline inserted before a record when another writer's line is open is not. The
aarch64 kernel's `log` sink writes each warning and error as this record.

## 23. The suite panel and run stage

### 23.1 Retired diagnostics panel

R3 removes the old 5×7 diagnostics panel and its counts, fail-rows and layout cases. The
run stage replaces it; handoff, between-case drawing, cursor restoration and kernel-screen
restoration checks stand, with GPU presents replacing framebuffer paint (B7).

### 23.2 Stage layout

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
The run stage uses BreenGL on BGL, with explicit UI time and logical coordinates in a
1280×800 space. R4 multiplies every coordinate, extent, gap and border by display scale;
this suite observes all 2560×1600 device pixels at scale 2. R3 uses pinned DejaVu Sans.
A legacy text scale n below denotes 7n logical pixels per em, rasterized at 7n×display-scale,
with §8.3 advances; expected text comes from that certified lower layer, never from stage
rendering. Thumbnail box-filter and spotlight nearest-scaling formulas below are unchanged.
Rectangles are half-open. The palette, in RGB, is background
(12,19,31), header (24,39,59), inset (18,29,44), white (255,255,255), dim (161,183,204), pass (26,112,77),
fail (145,50,58), not reached (57,67,80), pending outline (48,67,96), and timing bar (64,160,224).

| Element | Rectangle or text origin | Content |
|---|---|---|
| background | (0,0,1280,800) | background |
| header | (0,0,1280,96) | header |
| suite title | (24,16), scale 3 | `BREENIX / <suite>` |
| target and progress | (24,48), scale 1 | `<target>  case <n> of <N>  elapsed <ms> ms  fps <fps>` |
| counts | (24,68), scale 2 | `<P> passed  <F> failed  <R> not reached` |
| tile wall | (16,112,800,588) | adaptive category groups, as §23.3 |
| spotlight | (832,112,432,588) | inset; §23.4 |
| empty footer | (0,700,1280,100) | background; §23.5 |

`n` is one-based while a case runs and N at completion. Not reached counts only cases explicitly not reached;
pending and running cases are not counted as failures or not reached. Text is clipped at its element's right
edge. The renderer reads no clock and does not change the inputs.

### 23.3 Tiles

Cases occupy manifest order, grouped by category. Each category starts with a two-slot heading:
its full category id and `<P>/<N> pass` on separate lines, white on header colour. If only one
slot remains in a row, leave it empty before starting the heading on the next row. Cases follow
immediately; a category may continue onto the next row. No case is hidden or omitted.

Choose the largest integer scale s in 1..4 whose actual packed last tile ends at or above
y=700, with `columns = floor(800/(40s))`. Pack each two-slot heading and its cases in order,
including the one-slot gap only when a heading would straddle a row. At slot k the origin is
`(16 + 40s(k mod columns),112 + pitch floor(k/columns))`; its size is 38s by 24s.
Pitch is 24 at scale 1 and 26s at larger scales. Scale 1 has 24 rows (480 slots), enough for
393 cases, their 16 headings, and the reported expansion to 408 cases and 17 headings.
There is no separate case-count cap or unchecked fallback: a manifest that cannot fit even
at scale 1 is rejected before drawing. Smaller runs expand to the largest fitting scale.

- The one-pixel border is green for pass, red for failure, grey for not reached and pending
  outline colour for pending. Running uses RGB (64,128+floor(127q/500),224), where
  `q = 500 - abs((ui_time_ms mod 1000)-500)`, only between measured windows.
- The one-based manifest ordinal is at tile+(20s,2s), scale s; the key is at tile+(2s,16s),
  scale s. The key strip is (tile+(1,15s),38s-2,9s-1). Keys greater than 99999 use `>`;
  the spotlight prints the full number. Pending tiles have no filled strip or key text.
- The thumbnail is at tile+(2s,2s), at most 16s by 12s. First reduce the judged source to
  min(W,16) by min(H,12) by integer box filtering: each bin spans
  `[floor(xW/w),floor((x+1)W/w))` by `[floor(yH/h),floor((y+1)H/h))`; channel sums are
  divided by the bin pixel count, rounded half up, X=255. Enlarge each result pixel to
  s by s. A sampled source pixel never replaces the bin mean.

### 23.4 Spotlight

The heading `CASE <one-based ordinal> / <category>` is at (840,120), and its case id at (840,132),
scale 1, white.
A picture has `expected`, `actual`, `difference` labels at x=840,984,1128, y=152. Each pane is 128×128
at those x coordinates, y=168. The complete source is nearest-scaled into its pane: source coordinate
`(floor(xW/128),floor(yH/128))`. A difference pixel's largest absolute B,G,R channel difference d gives
BGRX (0,0,0,255) if d=0, otherwise (0,255-d,255,255). On the difference pane, the first mismatching source
pixel in row-major order is marked by white horizontal and vertical lines extending two pixels each way,
clipped to that pane. Its mapped coordinate is `(floor(x128/W),floor(y128/H))`.
At (840,312), scale 1, white, is `<N> pixels differ; bound <M>`.

When no independent reference was supplied, the actual-only pane is (848,168,400,250),
nearest-scaled from the complete verified snapshot and explicitly labelled as such.

A timing spotlight plots at (848,336,400,192). It accepts 1–64 frame times in microseconds and a positive
budget b. Bar i occupies `[floor(400i/n),floor(400(i+1)/n)-1)`, leaving a one-pixel gap, from the bottom
up by `floor(192 min(t,2b)/(2b))` pixels. The budget line is white at y=432. Bars at or below budget are
(64,160,224); over-budget bars are fail red. At (840,312) is `frame times us; budget <b>`.

An input spotlight names the exercised widget/window and its actual handler result, and plots
its recorded pointer trail. The trail maps its min/max x and y to (864,336)..(1216,528),
with each constant axis using an extent of one. Consecutive points are joined in timing blue;
5x5 point markers are white when up, pass green when down. Without pointer samples the heading
says `INPUT / events`, the panel says `Events (no pointer)`, and there is no pointer legend. For an input case with a judged picture, preserve the picture
panes and put the input evidence below them: label at (848,432), handler text at (848,448),
trail at (864,516)..(1216,596). Input metadata is snapshotted after its handler and transported
with a verified SHA-256, published after the timer stops. At most the first 256 pointer samples
are retained. Target text is bounded to 192 bytes with an explicit trailing ellipsis.

A text-only case shows its recorded measurement name at (848,208), scale 2, its value and unit
at (848,280), scale 3, and `Expected exactly <value> <unit>` or `Bound: <lo> to <hi> <unit>`
at (848,392), scale 2. Text wraps within 400 pixels. Prefer the last bounded VALUE, then the
last VALUE. Without a supplied reading, show the measured case execution time against its
runner deadline, labelled `Case execution time`; never invent a picture or comparison bound.
The selected case's verdict appears at (848,640), scale 3, in its stamp colour.

### 23.5 Empty footer

Operator specification change, 2026-10-22: remove the run stage's category ribbon,
its tick band and the observation caption beneath them. Category names, pass counts
and individual verdicts remain in the matrix. Leave the freed footer as background;
the header, matrix, spotlight and display-custody rules are unchanged.
`panel/milestone-ribbon` retains its case identity and now checks the empty footer
and the matrix's category counts after a category completes.

### 23.6 Finale

The finale preserves the selected first-failure spotlight (or last case if no failure) and overlays
(48,240,720,240) with header colour. Text in white: `RUN COMPLETE` at (72,264), scale 3; the same counts
as the header at (72,312), scale 2; `slowest <category>/<case> <ms> ms` at (72,352), scale 1.
It remains up for at least 5,000 ms of UI time before the runner emits DONE. The stage renderer receives
the finale start time; a stepped session must advance its UI clock through that hold. A stalled clock
is a stage failure at the runner deadline, never a successful hold or a lost DONE. The hold is complete iff `ui_time_ms.saturating_sub(start_ms) >= 5000`.

### 23.7 Boot stage ribbon

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B35 specifies these requirements. -->
The kernel draws one segment per architecture-applicable stage in `docs/boot-path.json`, in that file's
order, across (16,752,1248,16). For stage i of m, its segment starts at
x=16+floor(1248i/m) and ends at x=16+floor(1248(i+1)/m)-2, leaving a two-pixel gap.
Reached is pass green, unreached is pending outline colour. A stage lights only when its actual producer
prints the stage marker; loader announcements never light a segment. The serial stage lines remain the record.
This boot ribbon belongs only to the kernel console, not the run stage.
It is restored with the kernel screen after display giveback.
On the GPU core the console draws it over its text, in this band at the display's scale, and its
read-back check expects it there (docs/console/contract.md section 2, ruling B35). Each segment's last
two pixels are the background colour. A full repaint of the kernel screen (after a give-back) takes
point 15, which then shows the first reached stage unreached until the next full repaint.

### 23.8 Agreement

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
The runner derives CASE serial bytes and tile state and numbers from one stored result. A picture case
hands off the exact completed device readback it judged through `judged_readback`, before
modifying its target. A CPU-rendered actual image is never a passing picture. The
handoff canonicalizes BGRX and snapshots it with SHA-256 (FIPS 180-4); the runner verifies the complete
snapshot and appends ` sha256=<64 lowercase hex digits>` after `ms=<n>` on PASS and FAIL CASE lines
(before `msg=` on FAIL). The stage uses only that verified snapshot for the tile and actual pane. An independently constructed
reference may be handed off alongside it; it has the same dimensions and its own verified SHA-256.
When a surface check has no reference (for example a structural predicate), the spotlight labels it
`actual (reference not supplied)` and shows the actual pane alone; it never constructs a matching reference.
A malformed or changed snapshot fails the case. A case with no picture has no hash field.
Snapshot hashing and publication run after the case timer stops. The transported picture includes its
comparison bound; the spotlight prints that bound. Readings come from a hash-verified transport of
the exact emitted serial VALUE lines, independently of the lossy live strip. A case with no VALUE
shows its measured execution time; a skipped case has no fabricated zero-time reading.
A picture takes priority over timing data, so its input overlay cannot overlap the timing plot.
Input headings and legends describe a pointer trail only when pointer samples are present; event-only
cases use a neutral events label. Wrapped labels keep words intact and show an ellipsis when truncated. The stage composes only between cases, never in a
measured case window, except for its panel cursor while a case runs with the display left to the stage, and the frames a live
check asks it to draw (§23.9). Each result is drawn immediately after collection and verification, before
starting another case. `opt/breenix/pace` is an optional unsigned 32-bit millisecond boot value;
`scripts/boot-interactive.sh --mode suite --suite breengel --pace MS` supplies it through fw_cfg
and `run.sh --parallels|--vmware --suite breengel --pace MS` supplies `/etc/breenix/pace` on its
private boot disk. Zero or absence adds no hold. The runner applies pace only to the `breengel` suite, even in a sequence; other suites ignore it.
A present fw_cfg value, including zero, overrides the staged file. The ARM64 QEMU launcher rejects
holds that reach its serial-idle or 120-second classified-progress budget, or leave less than 60 seconds in the suite deadline for
boot, execution, rendering and the finale; use an explicit larger deadline for a longer watch.
Paced START records carry `pace_ms=MS`, and Vigil's profile contains `pace-MSms`.
The runner sleeps only after the completed spotlight has been presented, outside all case timers,
case deadlines and timing samples, with its SHOW mutex released. Elapsed UI time includes the holds.
The finale preserves the last judged picture on an all-pass run as well as the first failure
on a failed run. A stage error fails the affected case and preserves the serial DONE record.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B1-B8 and docs/gui/gpu-abi.md specify these requirements. -->
The stage validates boundary 4 on GPU backends 4 and 5 without drawing a caption.
`panel/boundary-notice` retains its case identity and checks that observation combination
and the empty footer.
An unknown or inconsistent backend/flags/boundary combination is an error, never a stronger observation.
The live showcase checks arm the matching fault, require its detection, disarm it and require the clean result:
12 `stage-lie`, 13 `stage-substitute-frame`, 14 `stage-draw-in-window`, 15 `ribbon-drop-stage`.
Input and measurement spotlights independently detect point-13 substitution, as do picture panes;
a changed thumbnail cannot satisfy their detection checks. Live checks also inspect the picture input
overlay, verdict, category headings and empty footer, with individual pixel substitutions in each region.

### 23.9 Display custody during a run (B29)

The stage holds the display from the first case to the last. It does not give the display
back before a case: the case's own takeover nests above the stage on the ownership stack, so
the stage's frame stays on the screen until the case presents, and when the case's process
ends the stack restores the stage's frame. An owner that took the display without presenting
was shown the frame below it, so that frame is what the restore binds. The console does not
show between cases. Three cases start above the kernel's screen instead, and for them alone
the stage gives the display back before the case: `composition/ownership-stack`, whose
property is the stack's four-owner depth, and `panel/kernel-screens` and `panel/boot-ribbon`,
whose property is the kernel's screen itself. A case that presents a full-screen scanout (the
composition and scanout cases, whose readback judges the device pixels of the whole mode,
section 20) shows it while it owns the display, and the stage's next frame follows when it
ends. After the three cases above the kernel's screen the stage takes the display back and
shows its last frame as soon as the case's process ends, before collecting its result.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B35 specifies these requirements. -->
The stage's compositor draws the panel cursor (ruling B35): the compositor's arrow sprite
(§18) at the pointer, read with system call 413, once the pointer has moved from where it
was when the stage made its GPU objects. Every stage frame shows it there. While
`panel/cursor-restore` or `panel/cursor-trail-detected` runs, the display stays with the
stage (the case's process does not take it over), and the stage follows the pointer on its
live-strip ticks: it restores the old footprint from the stage frame, draws the cursor at the
pointer and presents. Point 10 stops the restores from the cursor's next move until the
stage's next frame. These presents, only when the pointer moved, and the live checks' frames
below are the only presents the stage makes inside a measured window.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-rulings.md B36 and B38 specify these requirements. -->
The live checks `panel/judged-frame-shown`, `panel/serial-agreement`, `panel/quiet-window` and
`panel/boundary-notice` judge the stage's own frames. GPU handles are not inherited across
fork, so the case's process does not draw the stage: the display stays with the stage, the
case sends the runner the variant it judges (the finale, whether the first failure is kept,
the judged frame and the selected entry's stamp, record, timing, input and reading) over a
pipe, the runner draws and presents it with its own context, canvas, atlas and panes, and the
case reads the scanout back with `SCANOUT_READBACK`. A frame the check makes itself has at
most 16 pixels; the runner draws its panes as one solid rectangle per source pixel, and, for
a failing entry, its expected and difference panes the same way. No picture is uploaded for
a live check, and the case's process holds no stage context, so its timed window contains no
context warm-up and no redraw of the run's history.

## 24. Helpers and boots

- `/usr/local/test/bin/kiosk_test` is a test compositor: it claims the compositor, places windows where its
  command pipe tells it, composes on the GPU core after render completion and writes events
  through `WRITE_EVENTS`. Composing automatically, it sleeps on its commands and its scene descriptor
  (section 19.2). `composed <id>` returns the window's commit in its latest successful composite's record,
  and `warm <ms>` makes its compositor and waits for each context of its process to finish its catalogue
  warm-up. `gpu-counters` returns the helper's actual GPU counters;
  `gpu-private-scanout` retains and presents a real private target for the foreign-sample
  negative control; its handle and device binding are distinct identities.
  `gpu-scanout <guest-file>` performs target-0 device readback as its display owner and
  publishes dimensions, record v2 and pixels, with exact status/reason on failure.
  `/usr/local/test/bin/raw-window_test` is a client that uses the raw system calls without Breengel. The
  `windows`, `isolation`, `composition`, `events` and `scanout` cases create their helpers through
  `breenix_suite::suite::spawn`.
- Each case's limit is the runner's 10 s, except `font-parser/corpus`, `inflate/byte-split`,
  `png-decode/independent-fixtures`, `windows/churn` and `composition/present-rate`, which have 20 s
  (plan §19). `composition/irq-bound` arms irq-hold only once its own contexts and the compositor's
  have finished their catalogue warm-up (ruling B38), and its 10 s starts then, as gpu's
  `textures/upload-visibility` does (gpu.md ruling 52); the runner and host bound the warm-up.
- The suite's gate timeout on every target is 900 s.

## 25. Error numbers

The kernel returns Linux's errno values on both architectures: EPERM 1, ENOENT 2, ESRCH 3, EINTR 4, EIO 5,
EAGAIN 11, ENOMEM 12, EACCES 13, EFAULT 14, EBUSY 16, ENODEV 19, EINVAL 22, EMFILE 24, ENOSYS 38, ETIMEDOUT 110, EDQUOT 122.

<!-- claim-lint:ok: Normative reset recovery and provenance rule adopted by coordinator B40 item 2 and #2287. -->
B40 reset recovery: a device reset loses the stage's GPU pictures. Only after its
context reports RESET may the runner reconstruct retained thumbnails and panes from
its verified CPU pixels. These uploads are recovery, never new picture provenance;
UPLOADS reports recovery_bytes, device_resets and failure_artifact_bytes separately,
and the finale reconciles all Fixture bytes against those categories. A quarantine
or mode change does not authorize CPU picture reconstruction. The live
judged-frame-shown check must compare at least one published GPU entry, including
in a panel-only run. A pane retains its verified frame from drawn() through display
publication, so a reset in that interval cannot silently omit it.

After the finale's five-second hold the runner releases its picture snapshots,
readback buffer, painter, caches, contexts and unused targets before recording
reclamation. The display core retains the bound scanout, preserving the finale.
The memory record and its 48-page allowance remain unchanged; OWNERS lines report
the stage's CPU pictures and GPU context/resource/backing/copy totals at the
before, finale and retired boundaries.
