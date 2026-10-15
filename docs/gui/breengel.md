# BreenGL: the written contract

This is the contract the `breengel` suite (`docs/suites/breengel.json`, `/sbin/suite-breengel`) cites, as
the POSIX suites cite the standard. It fixes BreenGL's 2D library, its text, font, image and widget layers,
and the kernel window interface, compositor and display observation path, to the precision the cases need:
the API each case calls, the formula or literal table its expected pixels come from, the parser and decoder
bounds, the window interface's semantics and the error each operation returns. The plan of record is
`docs/gui/plan.md`; section numbers written "plan §N" refer to it. Where the plan already fixes a rule, this
document transcribes it and gives it a section number; where it leaves a detail to this contract, the detail
is fixed here.

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

- **Formats.** A 4-byte pixel is stored `B, G, R, X` when the surface is BGR (the canonical format,
  BGRX8888) and `R, G, B, X` when it is not. A 3-byte pixel is `B, G, R` or `R, G, B` the same way. The
  BGR flag swaps the order of the three colour bytes and changes nothing else.
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

The plan calls the drawing target a surface. In libgfx it is `libgfx::framebuf::FrameBuf`.

- `FrameBuf::new(width, height, format) -> Result<FrameBuf, SurfaceError>` allocates and owns its memory
  (stride `width × bytes_per_pixel`), cleared to zero; `FrameBuf::new_filled(width, height, format, color)`
  clears it to `color`. Dropping an owned surface frees its memory.
- `unsafe FrameBuf::from_raw(base, width, height, stride, bpp, is_bgr) -> Result<FrameBuf, SurfaceError>`
  wraps memory the caller owns. It checks every numeric rule below and returns the same errors as `new`. Its
  one unchecked obligation is that `base` stays valid for reads and writes of `stride × height` bytes, and is
  not accessed through another mutable path, for the surface's lifetime.
- `format` is `{ bpp: 3 | 4, is_bgr: bool, premultiplied: bool }`; `premultiplied` needs `bpp = 4`.

| Rule | Error |
|---|---|
| width or height is 0 | `SurfaceError::Empty` |
| width or height above 16,384 | `SurfaceError::TooLarge` |
| bpp other than 3 or 4 | `SurfaceError::Format` |
| stride below `width × bpp` | `SurfaceError::StrideTooShort` |
| `stride × height` above 2³¹ − 1, or overflowing | `SurfaceError::SizeOverflow` |
| base null | `SurfaceError::Null` |
| base not 4-byte aligned | `SurfaceError::Misaligned` |

Rows are `stride` bytes apart. The bytes of a row past `width × bpp` (the padding) belong to the caller and
no primitive reads or writes them. The suite draws surface cases into memory bracketed by `PROT_NONE` guard
pages, and fills every row's padding with a sentinel pattern of its own that every case checks
(`raster/guard-pages`, `stride-padding`).

## 4. Clipping

- Every primitive draws only inside the surface and inside the current clip.
- `push_clip(rect)` sets the clip to the intersection of `rect` and the current clip; `pop_clip()` restores
  the clip before the matching push. With no clip pushed the clip is the whole surface. An empty clip draws
  nothing and marks no damage (section 6).
- Coordinates anywhere in `i32` are valid input: a primitive clips against the surface and the clip in
  `i64`, and never wraps an end coordinate into the surface (`raster/fill-rect-overflow`).

## 5. Primitives

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

### 5.9 Row copies and single pixels

- `copy_row_from(fb, x, y, src, byte_count)` writes `min(byte_count, src.len(), (width − x) × bpp)` bytes
  starting at pixel `(x, y)`, and nothing when `x ≥ width` or `y ≥ height`. It never writes past the end of
  row `y` into the next row or into the row's padding (`raster/copy-row-clip`).
- `put_pixel(fb, x, y, color)` writes one pixel inside the surface and clip; `get_pixel` reads one.
- `clear(fb, color)` fills the current clip (the whole surface when none is pushed), never the padding.

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
| `copy_row_from(x, y, …)` writing `k` bytes | `(x, y, ⌈k / bpp⌉, 1)` |
| `clear` | the clip |
| text | the union of every drawn glyph's bitmap rectangle |

`raster/damage-sound` checks soundness: over a seeded random scene, every pixel that changed lies inside
the accumulated rectangle.

## 7. Work counters and raster bounds

`libgfx::work`, `libfont::work` and `libimg::work` each keep an always-on step counter: `reset()` and
`steps() -> u64`. A step is one iteration of any loop, including iterations that reject or skip, and one call
of any recursive function (plan §12.3). Allocations are counted by the suite's own global allocator
wrapper, which counts calls and bytes per thread.

| Work | Bound |
|---|---|
| a raster primitive at any coordinates | at most `2 × (A + R + C) + 64` steps, where `A`, `R` and `C` are the area, rows and columns of the primitive's clipped bounding rectangle; a primitive wholly outside the surface or clip does at most 64 |
| font load | at most `16 × file bytes + 65,536` steps and `2 × file bytes + 64 KiB` allocated |
| one glyph's outline | compound depth ≤ 8, ≤ 256 components, ≤ 65,535 points and no more than maxp declares, ≤ `16 × glyph bytes + 4,096` steps, ≤ `64 × points + 64 KiB` allocated |
| one glyph's flattening | quadratic subdivision depth ≤ 9 (≤ 512 segments per curve), ≤ 65,536 segments per glyph (else `GlyphTooComplex`), ≤ `1,028 × P` steps for `P` points after compound expansion |
| one glyph's rasterization at `s` px | bitmap ≤ `4s × 4s`; ≤ `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64` steps and ≤ `H × C + 4C + 48E + 4,096` bytes, for `H` rows, `C` sampled columns (3× width for LCD) and `E` segments; a glyph whose bound exceeds 2²⁸ is refused with `GlyphTooExpensive` before any raster loop |
| inflate | ≤ `4 × output + 16 × input + 1,024 × blocks` steps; 32 KiB window plus ≤ 64 KiB of tables besides the caller's output |
| image dimensions (BMP, PNG decode and encode) | width and height ≤ 16,384 and `width × height` ≤ 16,777,216, checked before any allocation |
| PNG decompressed size | rejected at the first byte beyond `rows × (1 + row bytes)` for the image's own dimensions, per pass for Adam7 |

**The independent square.** `font-parser/work-bound` rasterizes a four-edge square 1,022 px on a side,
built in the suite without libfont, whose bitmap is 1,024 × 1,024 (the outline plus the two-pixel margin).
With `H = C = 1,024` and `E = 4` the bound is **10,699,844** steps, and the case also requires at least
**8,388,608** (8 × 1,024 × 1,024), so a counter that skips the span fill is caught.

## 8. Text

### 8.1 The 5×7 bitmap font (`libgfx::font`)

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

`draw_char(fb, ch, x, y, fg)` draws the `noto-sans-mono-bitmap` 0.3 raster (`Regular`, `Size16`) of `ch`
with its top-left at `(x, y)`, each pixel by the coverage blend (section 2) of `fg` over the pixel already
there. The crate's raster is the independent reference cell (`text/noto-ascii`): the suite links the same
crate version libgfx draws from and reads the raster directly. A character the crate lacks draws the raster of `?`.

### 8.3 TrueType text (`libgfx::ttf_font`, `libfont`)

- **Sizes** are pixels per em. A size below 1 or above 1,024 is rejected: `rasterize_glyph` returns
  `FontError::BadSize` and draws nothing. `size26 = round_half_away(size × 64)`.
- **Advances and kerning** are 26.6 fixed point: `advance26(g) = round_half_away(advance_units(g) × size26 /
  unitsPerEm)` and `kern26(l, r) = round_half_away(kern_units(l, r) × size26 / unitsPerEm)`, using the legacy
  `kern` table's format 0 pairs. Layout is one codepoint at a time: the pen starts at 0, glyph `i` is drawn
  with its origin at `x + floor((pen_i + 32) / 64)`, and `pen_{i+1} = pen_i + advance26(g_i) + kern26(g_i,
  g_{i+1})`. Advances are accumulated in 26.6 and rounded once per glyph position, never truncated per glyph.
- **Width.** `draw_text` returns, and `text_width` equals, `floor((pen_end + 32) / 64)`: the distance from
  the pen start to the end of the last advance, rounded once (`text/ttf-advance`, `ttf-width`,
  `ttf-kerning`).
- **Translation.** A glyph's pixels depend on its pen position only by translation: drawing at
  `(x + dx, y + dy)` moves every pixel by `(dx, dy)`, and text starting off the left or top edge draws exactly
  its visible part (`text/ttf-clip-left`, `ttf-clip-top`).
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

### 8.4 The glyph cache

`CachedFont::new(font, capacity)` caches rasterized glyphs keyed by `(glyph, size26)` and evicts the least
recently used entry when full. `CachedFont::with_default_cache(font)` uses the contract's capacity: at most
256 entries and at most 4 MiB of bitmaps, evicting until both hold. `cache_stats() -> CacheStats { entries,
bytes, hits, misses }` reports the cache. A glyph served from the cache, or evicted and rasterized again, is
identical to a fresh rasterization.

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

Cases whose expectation is a whole rendered scene with no practical formula (`text/raster-identical`,
`ttf-glyph`, `ttf-sizes`, `ttf-subpixel`, `compound-transform`, `icons/assets`) compare against a reviewed PNG
under `docs/suites/frames/breengel/`, rendered by `tools/gfx-refgen` from the same `no_std` code at
authoring time and recorded in `docs/suites/frames/PROVENANCE.json` with the generator commit, reviewer,
date and what was checked (plan §7.3). The guest reads them from `/usr/local/test/share/breengel/frames/`.
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
| more points than `maxp.maxPoints`, or above 65,535 | glyph | `TooManyPoints` |
| a flag repeat or coordinate run past the glyph's data or point count | glyph | `BadGlyph` |
| compound nesting deeper than 8, or more than 256 components | glyph | `CompoundTooDeep` |
| a compound that refers to itself, directly or through another | glyph | `CompoundCycle` |
| a component index at or above `numGlyphs` | glyph | `BadComponent` |
| `hmtx` shorter than `4 × numberOfHMetrics + 2 × (numGlyphs − numberOfHMetrics)` bytes, or `numberOfHMetrics` of 0 or above `numGlyphs` | font | `BadHmtx` |
| a glyph past the flattening or rasterization bounds of section 7 | glyph | `GlyphTooComplex`, `GlyphTooExpensive` |

A valid format 4 cmap with any number of segments, odd counts included (the OpenType specification's
example has 39), maps exactly as the specification describes. A compressed `hmtx` gives glyphs past the
last long metric the last advance and their own left side bearings. The individual `font-parser` cases
build their fonts in the suite from literal tables and require rejection (any `FontError`) of each defect
and correct parsing of each valid variant; `corpus` requires each of its 64 files to give its documented
error. The corpus lives at `/usr/local/test/share/breengel/fonts-corpus/`, with `index.txt` naming each
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
- **Ends**: a stream cut off at any byte is `Truncated`. Output past `limit` is `OutputLimit`, and nothing is
  written past the limit.
- **zlib**: a bad header check value, a preset dictionary, a method other than 8 and a window above 32 KiB
  (CINFO > 7) are `BadZlibHeader`, `PresetDictionary`, `BadMethod` and `WindowTooLarge`; a wrong Adler-32 is
  `Adler32Mismatch`.
- **Fixtures** (`inflate/independent-fixtures`): twelve streams made at authoring time by an independent
  encoder, with the bytes it recorded, under `/usr/local/test/share/breengel/inflate/`.

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
  every other ancillary chunk are ignored (no colour management). A palette index past the palette is
  `BadPaletteIndex`. Zero width or height and unknown compression, filter or interlace methods are
  `BadHeader`; a bad signature is `BadSignature`; a cut-off stream is `Truncated`.
- **Limits**: section 7's dimension limits (`TooLarge`, before inflating) and decompressed size (`Bomb`).
- `encode(image) -> Vec<u8>` writes RGB (opaque images) or RGBA at depth 8, filter 0 on every row, one
  IDAT of stored or fixed-Huffman deflate blocks in a zlib stream with a correct Adler-32, and correct
  CRCs. Encoding the same image twice gives identical bytes. Images past the dimension limits are refused
  with `TooLarge`.
- **Fixtures**: the public PngSuite images, with the pixels an independent decoder recorded at authoring
  time, under `/usr/local/test/share/breengel/png/`; `png-decode/from-disk` reads `pinned.png` there.

## 14. Widgets (`libbui`)

Widgets are driven in-process with explicit `InputState` values (pointer position, button level and its press
and release edges) and drawn into a surface. Text in widgets is the 5×7 font at scale 1 (section 8.1), 7
pixels high, when `Theme::use_bitmap_font` is false.

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

- **Button** `(x, y, w, h)`: background `fill_rect` of the rectangle in `widget_bg` (idle), `widget_bg_hover`
  (pointer inside) or `widget_bg_active` (pressed); a one-pixel `draw_rect` border in `border`; the label in
  `text_primary` at `(x + (w − tw) / 2, y + (h − 7) / 2)`, `tw` its `text_width`, in Rust integer division.
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
- **Label**: text wider than its bounds is drawn cut at the last whole glyph that fits with `...` (three
  dots, 17 pixels at scale 1), and nothing outside the bounds.
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
- **Menu bar** at `(x, y, w, 22)`: menu `i`'s title is `(x_i, y, text_width(label) + 20, 22)`, titles laid left
  to right from `x`. A press on a title opens its menu: the dropdown is `(x_i, y + 22, W, H)`, filled with
  `widget_bg` and bordered in `border`, `W = max(140, widest row)`, `H` the sum of its rows (22 per item, 9
  per separator). An item's row is `18 + 24 + text_width(label)` wide, plus `24 + text_width(shortcut) + 8`
  when it has a shortcut and `24` when it has none; a shortcut is written `Ctrl+`, `Shift+`, `Alt+` (those it
  has, in that order) and its key in upper case. Item `k`'s label is drawn at `(x_i + 18, row_y + 7)` in `text_primary`, or `text_secondary`
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
| 5 | `LIST` | `List` (16) | entries in the registry | — |
| 6 | `ACQUIRE` | `Acquire` (296) | 0 | ETIMEDOUT, EINTR, ENOENT |
| 7 | `COMMIT` | `Commit` (304) | 0 | EBUSY double commit; EINVAL buffer or damage; ENOENT |
| 8 | `COMMIT_NO_CHANGE` | `NoChange` (32) | 0 | ENOENT |
| 9 | `RESIZE` | `Resize` (48) | 0 | EINVAL size; ENOMEM; ENOENT |
| 10 | `READ_EVENTS` | `ReadEvents` (40) | events copied | ETIMEDOUT, EINTR, ENOENT |
| 11 | `WRITE_EVENTS` | `WriteEvents` (24) | events queued | EINVAL type or length; ENOENT |
| 12 | `EVENT_FD` | `WinId` (8) | a descriptor | ENOENT, EMFILE |
| 13 | `SET_GEOMETRY` | `Geometry` (24) | 0 | ENOENT |
| 14 | `MAP_COMMITTED` | `MapBuffers` (40) | 0 | ENOENT |
| 15 | `CAPTURE` | `Capture` (56) | 0 | EINVAL short destination; ENOENT |
| 16 | `MAP_TREE` | `MapTree` (24) | 0 | ENOENT |
| 17 | `CLAIM_COMPOSITOR` | `Claim` (16) | 0 | EBUSY while a claim lives |
| 18 | `RELEASE_COMPOSITOR` | none (0) | 0 | — |
| 19 | `COMPOSITE` | `Composite` (24) | 0 | EIO backend failure, with the record's reason |
| 20 | `COMPOSITE_RECORD` | `RecordQuery` (24) | 0 | ENOENT serial no longer kept |
| 21 | `MAP_COMPOSITOR_TEXTURE` | `MapTree` (24) | 0 | — |
| 22 | `SCANOUT_READBACK` | `Readback` (40) | 0 | EAGAIN completion missing; EIO binding or serial mismatch; EINVAL short destination |
| 23 | `SET_CURSOR` | `Cursor` (16) | 0 | EINVAL shape |
| 24 | `HOLD_EVIDENCE` | `WinId` (8) | 0 | EBUSY another holder; ENOENT |
| 25 | `READ_REMNANT` | `Capture` (56) | 0 | ENOENT |
| 26 | `RELEASE_REMNANT` | `WinId` (8) | 0 | ENOENT |
| 27 | `GIVE_DISPLAY` | `Give` (8) | 0 | ESRCH no such process |

Argument structures (offsets in bytes):

- `WinId`: `id: u64 @0`.
- `Create`: `width: u32 @0`, `height: u32 @4`, `buffers: u32 @8` (2 or 3), `tree_kib: u32 @12` (0 means 64;
  at most 1,024), *out* `id: u64 @16`, *out* `stride: u32 @24`, reserved `@28`, *out* `buffer: [u64; 3] @32`
  (the mapped address of each buffer, 0 past `buffers`), *out* `tree: u64 @56` (the first tree page; buffer
  `i`'s page is `tree + i × tree_kib × 1024`).
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
- `Composite`: `flags: u32 @0`, reserved `@4`, *out* `serial: u64 @8`, *out* `pixels: u64 @16`.
- `RecordQuery`: `serial: u64 @0`, `ptr: u64 @8` (a `CompositeRecord`), `len: u32 @16`, reserved `@20`.
- `Readback`: `dst: u64 @0`, `dst_len: u64 @8`, `record: u64 @16` (an `ObservationRecord`),
  `record_len: u32 @24`, reserved `@28`, *out* `serial: u64 @32`.
- `Cursor`: `shape: u32 @0` (0 arrow, 1 to 4 the resize shapes, 5 hidden), `visible: u32 @4`, `x: i32 @8`,
  `y: i32 @12`.
- `Give`: `pid: u32 @0`, reserved `@4`.

### 17.2 Ids, limits and lifetime

- Window ids, commit ids, composite serials and input serials are 64-bit, start at 1 and are never reused
  within a boot. Every op on a destroyed window's id is ENOENT.
- A window is 1 to 4,096 pixels on each side (EINVAL outside). A title is valid UTF-8 of at most 64 bytes
  (EINVAL otherwise) and is stored and listed byte for byte. A process may own at most 16 windows; the
  seventeenth `CREATE` is EAGAIN and leaves the first sixteen unaffected.
- A new window's buffers are all zero. `LIST` reports registered windows only; `DESTROY` removes a window
  from the listing and unmaps its buffers in the owner.
- Window buffers count against the owner's `RLIMIT_AS`. `CREATE`, `REGISTER` and `RESIZE` each have one
  commit point; a failure before it, at any allocation step, leaves the registry, every mapping and the
  accounting exactly as before. A failed `RESIZE` leaves the old buffers intact.
- Window mappings are not inherited across `fork` (the kernel marks them do-not-copy), so a child has neither
  a mapping nor authority. A process's windows are destroyed at `exec` and at exit.
- Breengel's `Window` destroys its window on `Drop`, and `Window::new` destroys the window it created when
  registration fails.

### 17.3 The commit protocol

Plan §3.1 is the contract, transcribed here.

- A window owns a buffer set of two buffers, or three on request, each `width × height × 4` bytes of BGRX.
  Each buffer is *free* (owned by the client), *front* (committed, immutable) or *retired* (replaced but still
  read by a composite or a capture copy).
- `COMMIT(buffer, damage, meta)` publishes, in one transaction, the buffer's pixels, its tree page,
  `processed_serial`, `epoch` and `damage` (at most 16 rectangles covering every pixel that differs from the
  previous front). It returns a `commit_id` that increases by exactly 1 per commit of the window. A commit with
  non-empty damage starts a new pixel generation (`pixel_gen`, strictly increasing).
- After a commit the buffer is mapped read-only in the owner until it is released; a write to it raises
  SIGSEGV. The previous front becomes retired, and returns to free only when every composite that used it has
  finished, no capture is copying it and no `ACQUIRE` is replaying from it.
- **Bases and the damage log.** A buffer's base is the commit id whose pixels it holds: the id it was last
  committed under, the commit `ACQUIRE` last replayed it to, or none (new, or reallocated by a resize). The
  kernel keeps the damage of each window's last 8 commits.
- **`ACQUIRE` replays in the kernel.** Under the window's commit lock it reads the newest front *F* and takes
  a replay pin on *F*'s buffer, and computes the replay set: the union of the damage of every commit after the
  acquired buffer's base up to *F* (its bounding rectangle when that needs more than 16 rectangles), or the
  whole window (`replay_full = 1`) when the base is none or a commit in that range has left the damage log.
  It drops the lock, copies the replay set from *F* into the acquired buffer, drops the pin, sets the buffer's
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
- **Captures** (`CAPTURE`, `SCANOUT_READBACK`) pin the buffer they read only while copying inside the call. A
  resize or destroy during a capture retires the buffer and frees it after the copy, so a capture returns a
  whole committed generation, or ENOENT when the window was destroyed before the copy began. With a tree
  destination, `CAPTURE` copies the same buffer's tree page under the same pin and returns the one commit id
  both belong to.
- **No pins between system calls.** With two buffers, the longest a client ever waits in `ACQUIRE` is one
  composite plus one capture copy in progress.
- **Remnants.** A root process registered with `HOLD_EVIDENCE` as a window's evidence holder keeps, when the
  owner exits or crashes, the window's last front and tree page as a read-only remnant: outside the dead
  owner's pool, counted against the holder's `RLIMIT_AS`, at most 16 per holder, read with `READ_REMNANT` and
  freed by `RELEASE_REMNANT` or the holder's exit. Nothing is retained while the owner lives.

### 17.4 Authority

Principals: the **owner** (by kernel identity, never by PID number, so PID reuse grants nothing), the
**compositor** (the holder of the current claim), **root** (uid 0) and an **unrelated** process.

| Operation | Owner | Compositor | Root, not compositor | Unrelated |
|---|---|---|---|---|
| `CREATE`, `REGISTER`, `SET_TITLE` | yes | — | — | — |
| `ACQUIRE`, `COMMIT`, `COMMIT_NO_CHANGE`, `DESTROY` | yes | EPERM | EPERM | EPERM |
| `RESIZE` | yes | EPERM (requests go through events) | EPERM | EPERM |
| `READ_EVENTS`, `EVENT_FD` | yes | — | — | EPERM |
| `WRITE_EVENTS`, `SET_GEOMETRY` | EPERM | yes | EPERM | EPERM |
| `MAP_COMMITTED` | yes | yes | EPERM | EPERM |
| `CAPTURE`, `MAP_TREE` | yes | yes | yes | EPERM |
| `COMPOSITE`, `COMPOSITE_RECORD`, `MAP_COMPOSITOR_TEXTURE`, `SET_CURSOR` | — | yes | EPERM | EPERM |
| `CLAIM_COMPOSITOR` | — | — | yes if no live claim, else EBUSY | EPERM |
| `SCANOUT_READBACK` | — | yes | yes | EPERM |
| `HOLD_EVIDENCE`, `READ_REMNANT`, `RELEASE_REMNANT` | EPERM | EPERM | yes (EBUSY if another holder) | EPERM |

### 17.5 Pointers, errors and quiet failure

- Every op copies through `copy_from_user` and `copy_to_user`. A range that overflows, is unmapped, crosses
  into an unmapped page or is read-only for output is EFAULT, and an op that fails with EFAULT has no side
  effect: copies in finish before any state changes, copies out before the commit point. A read of events
  into a bad buffer consumes no event. An output landing in a copy-on-write page after `fork` lands only in
  the caller's copy.
- No window op writes to the serial log or any kernel output on any path (section 22).
- Destroying a window while another thread commits it ends with exactly one of the two outcomes and no fault.
- A blocked `ACQUIRE` or blocking `READ_EVENTS` returns within 250 ms of a SIGKILL, a destroy or a compositor
  death. A timed wait returns no earlier than its timeout and no later than the timeout plus 100 ms.

### 17.6 Retired operations

The graphics system call `fb_draw` (411) keeps op 6 (`FLUSH`) and op 8 (`FLUSH_BATCH`). Its demo ops 0 to 5,
7 and 9 return ENOSYS (`isolation/retired-ops`), and its wait-stress ops 27 to 30 return ENOSYS in the
production kernel; they exist only in `boot_tests` builds (`isolation/test-hooks-absent`). The version 1
window ops (10 to 26 and 31) are removed when version 2 lands.

## 18. Compositor, composition and the display

- **Claims and epochs.** `CLAIM_COMPOSITOR` gives root the compositor role with an epoch that increases at
  every claim; a second claim while one lives is EBUSY; uid 1000 is EPERM. When the holder dies or releases,
  the role can be claimed again with a higher epoch. Windows outlive the compositor with their last geometry,
  stacking and visibility. Each claim delivers `CompositorChanged { epoch }` to every window (section 19).
- **Composition.** `COMPOSITE` composes every visible window at its geometry, in increasing `z` (later over
  earlier), over black, onto the 1280 × 800 screen, clipped at the screen's edges, exactly: each output pixel
  is the topmost visible window's committed pixel there, or black. It processes exactly the pixels of the
  damage union since the previous composite, and the full screen the first time; the frame equals a full
  recomposition. At most one composite per window is in flight.
- **Composite records.** `COMPOSITE_RECORD(serial)` reads `CompositeRecord` (16-byte header `serial: u64 @0`,
  `pixels: u64 @8`, then `count: u32 @16`, `flags: u32 @20`, then `count` entries of 32 bytes: `id: u64`,
  `commit_id: u64`, `pixel_gen: u64`, `pixels: u64`). Every presentation-required window (registered,
  visible, not minimized) is listed with the generation the frame is correct for, a wholly occluded one with
  `pixels = 0`. A commit is presented when a composite with its readback completion lists its window at a
  `pixel_gen` at or above the commit's.
- **Failure.** A backend failure leaves the commit uncomposited, fails `COMPOSITE` with EIO and the record's
  reason, and the next composite retries it.
- **Cursor.** The compositor draws the cursor sprite above every window at its position. Moving it over a
  static scene restores every pixel it covered; hidden, no cursor pixel appears; content committed under a
  stationary cursor is shown with the cursor redrawn above it. Shapes 0 to 4 are the arrow and the four
  resize shapes.
- **Display ownership** is a stack: `take_over_display` (431) pushes the caller as owner, `give_back_display`
  (432) pops it, and `GIVE_DISPLAY(pid)` hands it to another process, which becomes owner above the giver.
  When an owner dies the display returns to the process that gave it, never to the runner, and the previous
  owner's frame is restored exactly.
- **The framebuffer.** In a GUI suite boot every target runs its display at **1280 × 800**: `fbinfo` (410)
  reports width 1280, height 800, 4 bytes per pixel, a stride of at least 5,120 bytes and a pixel format of
  RGB (0) or BGR (1). `fb_mmap` (412) gives the display owner the whole screen, every row of all 1280 columns,
  and `FLUSH` clips its rectangle to the screen.
- **Numbers** (plan §12.2): one 256 × 256 window committing as fast as its buffers are released is composed at
  least 60 times a second (300 commits, the first 30 discarded); the longest IRQ-masked interval in the
  display copy during full-screen damage is at most 500 µs. The counters: trace counters `composites`,
  `composite_pixels`, `damage_rects`, `window_buffer_allocs`, and the gauge `display_irq_masked_max_us`, in
  `/proc/trace/counters`.

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
| 56 | `width: u32` | new width, `Resized` and `ResizeFailed` |
| 60 | `height: u32` | new height |
| 64 | `pressed: u32` | 1 press, 0 release, `PointerButton` only |
| 68 | `text_len: u32` | bytes of `text`, at most 240 |
| 72 | `lost: u32` | events lost before this one, `Overflow` only |
| 76 | reserved | zero |
| 80 | `text: [u8; 240]` | UTF-8, `KeyPress` (the text the compositor attached) and `Text` |

Kinds: 1 `KeyPress`, 2 `KeyRelease`, 3 `Text`, 4 `PointerMove`, 5 `PointerButton`, 6 `Scroll`,
7 `FocusGained`, 8 `FocusLost`, 9 `CloseRequested`, 10 `Resized`, 11 `ResizeFailed`, 12 `FontChanged`
(made by Breengel, section 9.3), 13 `CompositorChanged`, 14 `Overflow`. No field carries another field's
meaning. Breengel maps an unknown kind to `Event::Unknown { kind }`, never to a key press.

### 19.2 Delivery

- `WRITE_EVENTS` (compositor only) queues events on one window's ring of 64. A kind outside 1 to 14 (other
  than those Breengel makes) or a `text_len` above 240 is EINVAL, and nothing in that call is delivered.
  Writing to a destroyed window is ENOENT.
- Events arrive in the order written. A full ring keeps the events it holds, counts the ones it could not
  queue, and delivers an `Overflow` event with the count before the next event; nothing is lost silently.
- An event for one window wakes only that window's readers. `EVENT_FD` is pollable and reports POLLIN when an
  event is queued.
- `Resized` arrives only after the buffers have the new size and carries it; a refused resize delivers
  `ResizeFailed` with the old size and never `Resized`.

### 19.3 Serials

The compositor assigns each event a serial that increases strictly per compositor and is never reused.

### 19.4 Dispatch and the processed watermark

Plan §6.4 is the contract. Breengel apps take events with `window.dispatch(handler)`; the handler returns
`Redraw` or `NoChange`, and a serial counts as handled once its handler has returned. For `commit(buffer)`,
`processed_serial` is the highest serial whose handler had returned when that buffer was acquired: an event
whose handler returns after frame N was acquired is acknowledged by frame N + 1, never by N, and an event
polled but not yet handled is never acknowledged. For `commit_no_change`, it is the highest serial *s* such
that every serial up to *s* handled since the last pixel commit returned `NoChange`: a `Redraw` stops it until
a frame acquired after its handler returned is committed. Serials Breengel refused at dispatch are listed in
`Meta.rejected` and count as finished. Raw clients set the watermark themselves.

## 20. Scanout observation

### 20.1 `SCANOUT_READBACK`

Root or the compositor reads the whole composed destination, as canonical BGRX rows of `1280 × 4` bytes, with
an `ObservationRecord` (512 bytes) produced atomically for one composite serial:

| Offset | Field |
|---|---|
| 0 | `serial: u64` |
| 8 | `backend: u32`: 1 virtio-gpu 2D, 2 GOP framebuffer, 3 SVGA3 screen target, 4 VirGL |
| 12 | `flags: u32`: bit 0 the device's copy is outside the guest's view |
| 16 | `width: u32`, `height: u32`, `stride: u32`, `source_format: u32` (0 RGB, 1 BGR) |
| 32 | `resource: u64`: the scanout resource id, screen-target id or framebuffer address bound to the device |
| 40 | `window_count: u32`, `completion_count: u32` |
| 48 | `windows: [(id: u64, commit_id: u64); 16]` |
| 304 | `completions: [(fence: u64, kind: u32, status: u32); 12]` |
| 496 | `boundary: u32`: what stays outside the guest's view (section 20.2) |
| 500 | reserved |

The full destination is read; no sampled band stands for the frame. A readback never returns pixels without
its record. It refuses with EAGAIN while serial *S*'s completions are unanswered, and with EIO and a named
reason when the device's binding differs from the composed resource or a fence belongs to another serial.
RGB and BGR destinations read back as the same canonical pixels, and X is normalized to 0xFF in every
capture (`CAPTURE` and `SCANOUT_READBACK`).

### 20.2 Per-backend rows (from the entry spikes)

| Target | Backend | What the guest reads | Completion | Boundary code and meaning |
|---|---|---|---|---|
| ARM64 QEMU | virtio-gpu 2D | the full guest backing of the resource bound by `SET_SCANOUT` | a fenced `TRANSFER_TO_HOST_2D` per damaged rectangle of *S*, then a fenced `RESOURCE_FLUSH`, each answered `OK_NODATA` with its fence echoed | 1: the host's copy of the resource and the host window |
| x86-64 | GOP framebuffer, 1280 × 800 set by the kernel through virtio-vga's DISPI registers | the complete framebuffer memory after the copy, converted from BGR | each damaged band's copy returned, with a store barrier | 2: the host's scanning of that memory into its window |
| VMware | SVGA3 screen target, without readback (`READBACK_GB_SURFACE` does not return the device's copy) | the guest backing (MOB) of the surface bound by `DEFINE_GB_SCREENTARGET` | `UPDATE_GB_IMAGE` and `UPDATE_GB_SCREENTARGET` per damaged rectangle, followed by `SVGA_CMD_FENCE`, with `SVGA_REG_FENCE` reaching the fence | 1: the device's copy and the host window |
| Parallels | VirGL | the scanout resource read back by a fenced `TRANSFER_FROM_HOST_3D` into a guest buffer | the composite's `SUBMIT_3D` fence and the transfer fence, both answered | 3: the host window |

The VirGL path reads X as 0xFF; nearest sampling, integer-aligned quads and no sRGB conversion are contract,
so Parallels' frames use the same formula expectations as the software targets. Parallels runs at
1280 × 800 like the others.

### 20.3 Planted faults

Test controls (section 21) plant one fault each, and the matching `scanout` case requires the exact reason,
then recovery on the next composite once the fault is disarmed: `skip-transfer` (a skipped transfer or update:
EAGAIN, missing completion), `wrong-resource` (a flush or scanout bound to another resource: EIO,
`binding`), `stale-fence` (a fence answered for another serial: EIO, `serial`), and `corrupt-outside-damage`
(bytes written into the destination's last row outside any damage, found by comparing the full destination
with the composition formula).

## 21. Suite-mode test controls

`test_control(op: u32, arg: *mut u8, len: usize) -> isize` is system call **415** on both architectures.

- **The latch.** The kernel enables test controls only when the first program it executes as PID 1 is a suite
  binary (`/sbin/suite-*`); it latches that fact at that exec, before any user code runs, and nothing changes
  it for the rest of the boot. In every other boot every op returns ENOSYS. In a suite boot only root may use
  them (EPERM otherwise).
- Op 0 `STATE` returns 1 (enabled). Op 1 `ARM { point: u32 @0, reserved @4, target: u64 @8, param: u64 @16 }`
  arms one one-shot fault; op 2 `DISARM_ALL`; op 3 `FIRED { point: u32 @0, reserved @4, out count: u64 @8 }`
  reports how many times a point fired. There is no op that sets or clears the latch: op 4 `SET_LATCH` and
  every op above 4 are EINVAL.
- Points: 1 `ignore-damage` (the next composite processes the whole window), 2 `irq-hold` (the next display
  copy holds IRQs masked for 2 ms), 3 `emit-diagnostic` (param: the writer path, section 22), 4
  `acquire-stall` (target: a window; the next `ACQUIRE` pauses after copying half its replay rows until
  released or 1 s), 5 `backend-error`, 6 `skip-transfer`, 7 `wrong-resource`, 8 `stale-fence`, 9
  `corrupt-outside-damage`, 10 `panel-cursor-no-restore`, 11 `release-stall` (resumes a paused point).
  Faults are branches in the real code paths, disarmed at case end by the suite.
- `init`, running as PID 1, checks that every op returns ENOSYS and prints `[init] test controls absent`; run
  as a child (in a suite boot) it prints `[init] not PID 1: test-control check skipped` and makes no absence
  claim.

## 22. Kernel output accounting

Plan §12.1 is the contract. Each thread's `kernel_output_bytes` counts the payload bytes it commits for
output at exactly one accounting boundary per byte, read as decimal text from
`/proc/<pid>/task/<tid>/kernel_output`. Bytes produced where the current thread cannot be read safely (before
per-CPU data, panic and double-fault paths, `emergency_print`) count in the trace counter
`kernel_output_unattributed`. Drains never count; a notice counts toward the thread that posted it; lead
newlines are not payload.

`emit-diagnostic(path)` makes the calling thread's next window op write one fixed 32-byte payload,
`BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD!`, through the named writer: 1 `serial_println!`, 2 `log::warn!`,
3 `write_byte`, 4 `write_bytes_atomic`, 5 `try_print`, 6 `write_str_bounded`, 7 `_log_print`, 8 a notice
posted by this thread and written out by another, 9 `emergency_print`. Each adds exactly 32 to the calling
thread's counter, except `log::warn!`, which adds the logger's whole record, and `emergency_print`, which adds
32 to `kernel_output_unattributed` and nothing to the thread. The logger's record is
`[ WARN] test_control: ` followed by the payload and a newline: `[`, the level right-aligned in five columns,
`] `, the target `test_control`, `: `, the payload, `\n`, which is 55 bytes.

## 23. The suite panel

`libgfx::diagnostics::draw` with `scored: true` on a 1280 × 800 surface, two groups and no output or live
strip, draws at scale 2. For the fixture the cases use (title `BREENGEL PANEL`, subtitle `fixture`, verdict
`Running("run")`, group `alpha` with checks `a1` PASS, `a2` FAIL `bad pixel`, `a3` PASS; group `beta` with `b1`
FAIL `no frame`, `b2` pending, `b3` PASS) the rectangles are:

| Element | Rectangle | Content |
|---|---|---|
| background | the whole surface | (12, 19, 31) |
| banner | (0, 0, 1280, 108) | (24, 39, 59) |
| score | text at (32, 124), scale 4, white | `3 / 6 PASSED  50%` |
| counts | text right-aligned to x = 1248 at y = 138, scale 2, (161, 183, 204) | `2 failed  0 skipped  1 to run` |
| score bar | (32, 162, 1216, 18) | passed (32..640) (26, 112, 77), failed (640..1045) (145, 50, 58), the rest (57, 67, 80) |
| group `alpha` | heading at (32, 204), bar (32, 224, 596, 12) | bar: passed (32..429), failed (429..628) |
| group `beta` | heading at (652, 204), bar (652, 224, 596, 12) | bar: passed (652..850), failed (850..1049), pending (1049..1248) |
| cards | group x, y = 248 + 52k, 596 × 46 | fill by state: pass (26, 112, 77), fail (145, 50, 58), pending (57, 67, 80); name at (x + 12, y + 4), state label right-aligned to x + 584 at y + 4, detail at (x + 12, y + 26), all scale 2, white |
| verdict bar | (0, 724, 1280, 76) | (48, 67, 96), text at (32, 746), scale 2, white |

Text is the 5×7 font (section 8.1), which `text/bitmap-glyph` and `bitmap-scale` certify, so the cases
draw the expected text with it. In a GUI suite the runner draws the panel only between cases and restores it
exactly after each; the panel cursor is saved and restored, so moving the pointer over the panel leaves no
trail; the kernel's console screen redraws exactly after the display is given back.

## 24. Helpers and boots

- `/usr/local/test/bin/kiosk_test` is a test compositor: it claims the compositor, places windows where its
  command pipe tells it, composes on every commit and writes events through `WRITE_EVENTS`.
  `/usr/local/test/bin/raw-window_test` is a client that uses the raw system calls without Breengel. The
  `windows`, `isolation`, `composition`, `events` and `scanout` cases create their helpers through
  `libbreenix::suite::spawn`.
- Each case's limit is the runner's 10 s, except `font-parser/corpus`, `inflate/byte-split`,
  `png-decode/independent-fixtures`, `windows/churn` and `composition/present-rate`, which have 20 s
  (plan §19).
- The suite's gate timeout on every target is 900 s.

## 25. Error numbers

The kernel returns Linux's errno values on both architectures: EPERM 1, ENOENT 2, ESRCH 3, EINTR 4, EIO 5,
EAGAIN 11, ENOMEM 12, EACCES 13, EFAULT 14, EBUSY 16, EINVAL 22, EMFILE 24, ENOSYS 38, ETIMEDOUT 110.
