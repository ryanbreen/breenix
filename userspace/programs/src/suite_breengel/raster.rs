//! raster, text and font-parser: shapes, blits, clipping and alpha; TrueType and bitmap
//! text; font parsing safety (contract sections 2 to 8 and 10). The `fontwatch-*` text
//! cases live in `widgets.rs`.

use crate::*;
use libgfx::framebuf::{Format, Rect, SurfaceError};
use libgfx::{alpha, blit, work};

pub(crate) const RASTER: &[Case] = &[
    pictured!("fill-rect", "fill_rect writes exactly the pixels of a half-open rectangle and no others", raster_fill_rect),
    pictured!("fill-rect-clip-left", "A rectangle partly off the left edge fills only its visible columns", raster_fill_rect_clip_left),
    pictured!("fill-rect-offscreen-left", "A rectangle wholly left of the surface changes no pixel", raster_fill_rect_offscreen_left),
    pictured!("fill-rect-offscreen-top", "A rectangle wholly above the surface changes no pixel", raster_fill_rect_offscreen_top),
    pictured!("fill-rect-overflow", "A rectangle whose x + w overflows i32 clips to the surface without wrapping", raster_fill_rect_overflow),
    pictured!("fill-rect-empty", "Zero or negative width or height draws nothing", raster_fill_rect_empty),
    pictured!("draw-rect", "draw_rect writes exactly the one-pixel perimeter", raster_draw_rect),
    pictured!("line-axes", "Horizontal and vertical lines cover exactly their spans, endpoints included", raster_line_axes),
    pictured!("line-bresenham", "Shallow, steep and 45-degree lines write exactly the pixels in the contract's literal tables", raster_line_bresenham),
    pictured!("line-clip", "A line crossing the edges writes only the in-surface pixels of its unclipped path", raster_line_clip),
    pictured!("circle-fill", "fill_circle sets exactly the pixels the contract's x² + y² ≤ r² rule selects", raster_circle_fill),
    pictured!("circle-fill-offscreen-left", "A circle partly or wholly off the left edge writes nothing outside the surface", raster_circle_fill_offscreen_left),
    pictured!("circle-outline", "draw_circle writes the midpoint-algorithm pixels listed in the contract for radii 0 to 40", raster_circle_outline),
    pictured!("guard-pages", "Every primitive called with extreme coordinates leaves the guard pages untouched and the process alive", raster_guard_pages),
    pictured!("extreme-work-bound", "Every primitive at coordinates near ±2³¹ takes no more loop steps than section 12.3's bound for its clipped rectangle, by libgfx's work counter, which counts rejected iterations too", raster_extreme_work_bound),
    pictured!("stride-padding", "Drawing into a surface whose stride exceeds width × bytes per pixel leaves every row's sentinel bytes intact", raster_stride_padding),
    pictured!("surface-validate", "`Surface::new` and `Surface::from_raw` both reject a short stride, an overflowing size, an oversize dimension, a misaligned or null base with section 12.3's errors", raster_surface_validate),
    pictured!("bpp3-bpp4", "The same scene on a 3-byte RGB surface and a 4-byte BGRX surface gives the same colours", raster_bpp3_bpp4),
    pictured!("bgr-order", "The BGR flag swaps the channel order and nothing else", raster_bgr_order),
    pictured!("x-byte", "Every primitive writes the X byte of a BGRX pixel as 0xFF", raster_x_byte),
    pictured!("copy-row-clip", "copy_row_from clips at the end of the row instead of wrapping into the next one", raster_copy_row_clip),
    pictured!("premultiply", "Straight colour and alpha premultiply to div255(c × a) for all 65,536 pairs", raster_premultiply),
    pictured!("alpha-over-examples", "Source-over gives the contract's literal results for fractional source and destination alphas", raster_alpha_over_examples),
    pictured!("alpha-over-sweep", "Source-over matches the premultiplied formula for all 256 alphas over four backgrounds", raster_alpha_over_sweep),
    pictured!("alpha-extremes", "Alpha 255 replaces the destination and alpha 0 leaves it unchanged", raster_alpha_extremes),
    pictured!("alpha-output", "The output alpha over a translucent destination follows the contract's formula", raster_alpha_output),
    pictured!("blit", "blit copies a source surface to a position exactly", raster_blit),
    pictured!("blit-clip", "A blit partly off the surface copies only the visible intersection", raster_blit_clip),
    pictured!("blit-alpha", "A premultiplied blit blends each pixel by the formula", raster_blit_alpha),
    pictured!("blit-straight", "A straight-alpha source is premultiplied exactly once before blending", raster_blit_straight),
    pictured!("blit-scale", "An integer-scale blit replicates each source pixel into an exact square", raster_blit_scale),
    pictured!("clip-rect", "With a clip rectangle set, every primitive draws only inside it", raster_clip_rect),
    pictured!("clip-stack", "Nested clips intersect, and popping one restores the outer clip", raster_clip_stack),
    pictured!("clip-empty", "An empty clip draws nothing and marks no damage", raster_clip_empty),
    pictured!("rounded-rect", "Rounded-rectangle corners match the contract's corner predicate", raster_rounded_rect),
    pictured!("gradient", "A vertical gradient's rows equal the contract's integer interpolation", raster_gradient),
    pictured!("damage-exact", "Each primitive marks exactly the clipped bounding rectangle the contract's table gives", raster_damage_exact),
    pictured!("damage-noop", "A draw clipped to nothing marks nothing, and a same-colour draw still marks its rectangle", raster_damage_noop),
    pictured!("damage-sound", "Over a seeded random scene, every changed pixel lies inside the accumulated damage", raster_damage_sound),
    pictured!("surface-owned", "An owned surface allocates, clears to a colour and frees its memory when dropped", raster_surface_owned),
];

pub(crate) const TEXT: &[Case] = &[
    pictured!("fonts-pinned", "The test fonts parse, and their SHA-256 values match the pins", text_fonts_pinned),
    pictured!("raster-identical", "A fixed set of rasterized glyphs hashes to the pinned value, the same on every target", text_raster_identical),
    pictured!("metrics-independent", "Advance widths, left bearings and bounding boxes of 50 glyphs equal the independently extracted table", text_metrics_independent),
    pictured!("cmap-independent", "Codepoint-to-glyph mappings for 200 codepoints equal the independently extracted table", text_cmap_independent),
    pictured!("bitmap-glyph", "The built-in 5x7 font draws 'A' exactly as its table defines it", text_bitmap_glyph),
    pictured!("bitmap-scale", "Scale 3 turns each glyph pixel into a 3x3 block", text_bitmap_scale),
    pictured!("bitmap-unknown", "A byte without a glyph draws the contract's replacement box", text_bitmap_unknown),
    pictured!("noto-ascii", "noto-sans-mono-bitmap draws every printable ASCII character matching its reference cell", text_noto_ascii),
    pictured!("noto-coverage", "Bitmap coverage blends with the background by the alpha contract", text_noto_coverage),
    pictured!("ttf-glyph", "DejaVu Sans Mono \"Hg\" at 16 px matches its reviewed reference", text_ttf_glyph),
    pictured!("ttf-sizes", "One string at 8, 12, 16, 24 and 48 px matches each size's reviewed reference", text_ttf_sizes),
    pictured!("ttf-advance", "A 200-character line ends where the literal sum of 26.6 advances says, rounded once", text_ttf_advance),
    pictured!("ttf-kerning", "\"AV\" in DejaVu Sans is narrower than \"A\" plus \"V\" by the kern table's literal value", text_ttf_kerning),
    pictured!("ttf-width", "text_width equals the distance from the pen start to the end of the last advance", text_ttf_width),
    pictured!("ttf-clip-left", "Text starting at negative x draws its visible part", text_ttf_clip_left),
    pictured!("ttf-clip-top", "Glyphs partly above row 0 draw their visible part", text_ttf_clip_top),
    pictured!("ttf-subpixel", "LCD subpixel text matches its reviewed reference per channel", text_ttf_subpixel),
    pictured!("ttf-notdef", "A codepoint the font lacks draws glyph 0", text_ttf_notdef),
    pictured!("utf8-text", "UTF-8 input draws \"é\" from the font's glyph for U+00E9", text_utf8_text),
    pictured!("utf8-invalid", "Invalid UTF-8 draws one U+FFFD per maximal invalid subpart and does not panic", text_utf8_invalid),
    pictured!("compound-glyph", "A compound glyph places its components at the literal offsets", text_compound_glyph),
    pictured!("compound-transform", "Scaled and 2x2-transformed components match their reviewed reference", text_compound_transform),
    pictured!("line-metrics", "Ascent, descent and line gap equal the font's hhea values at each size", text_line_metrics),
    pictured!("size-range", "Sizes from 6 to 72 px render, and size 0 or above 1,024 px is rejected; at 6, 12, 24, 48 and 72 px every printable ASCII glyph of the pinned fonts, grayscale and LCD, stays within section 12.3's flattening and rasterization bounds by the work counter", text_size_range),
    pictured!("many-args-abi", "The text calls with ten or more parameters, including trailing f32 or 26.6 values, give the reference output on this target", text_many_args_abi),
    pictured!("cache-hit", "A cached glyph renders identically to a fresh one, and the cache reports a hit", text_cache_hit),
    pictured!("cache-bound", "2,000 distinct glyph and size pairs keep the cache within 256 entries and 4 MiB", text_cache_bound),
    pictured!("cache-evict-correct", "A glyph evicted and rasterized again renders identically", text_cache_evict_correct),
];

pub(crate) const FONT_PARSER: &[Case] = &[
    pictured!("truncated", "A truncated TTF fails to parse with an error instead of panicking", font_parser_truncated),
    pictured!("table-offsets", "Table offsets or lengths past the end of the file are rejected", font_parser_table_offsets),
    pictured!("loca-range", "A loca entry past the glyf table is rejected for that glyph only", font_parser_loca_range),
    pictured!("loca-order", "A loca table that is not monotonic is rejected", font_parser_loca_order),
    pictured!("numglyphs", "A glyph count that disagrees between maxp and loca is rejected", font_parser_numglyphs),
    pictured!("cmap4-segments", "A format 4 cmap whose segCountX2 is odd, or whose end codes are not sorted, is rejected", font_parser_cmap4_segments),
    pictured!("cmap4-odd-segments", "A valid format 4 cmap with an odd number of segments (39, as in the OpenType specification's example) maps 200 codepoints as the independently extracted table says", font_parser_cmap4_odd_segments),
    pictured!("cmap4-range-offset", "A format 4 idRangeOffset that points outside the table is rejected", font_parser_cmap4_range_offset),
    pictured!("cmap12-groups", "Overlapping or unsorted format 12 groups are rejected", font_parser_cmap12_groups),
    pictured!("glyf-endpoints", "Contour end points that do not increase are rejected", font_parser_glyf_endpoints),
    pictured!("glyf-points", "A point count above maxp's maximum is rejected", font_parser_glyf_points),
    pictured!("glyf-flags", "A flag repeat that runs past the glyph's data is rejected", font_parser_glyf_flags),
    pictured!("compound-depth", "A compound nested deeper than the limit is rejected for that glyph", font_parser_compound_depth),
    pictured!("compound-cycle", "A compound glyph that refers to itself, directly or through another, is rejected within bounded work", font_parser_compound_cycle),
    pictured!("compound-missing", "A component index past the glyph count is rejected", font_parser_compound_missing),
    pictured!("hmtx-compressed", "A valid hmtx with fewer long metrics than glyphs gives the remaining glyphs the last advance and their own left side bearings", font_parser_hmtx_compressed),
    pictured!("hmtx-truncated", "An hmtx shorter than 4 × numberOfHMetrics + 2 × (numGlyphs − numberOfHMetrics) bytes, or an hhea numberOfHMetrics of 0 or above the glyph count, is rejected", font_parser_hmtx_truncated),
    pictured!("units-per-em", "unitsPerEm of 0 or above 16,384 is rejected", font_parser_units_per_em),
    pictured!("coords-extreme", "Glyphs with coordinates at the i16 limits either flatten and rasterize within section 12.3's bounds by the work counter, or are refused with `GlyphTooComplex` or `GlyphTooExpensive` before any raster loop runs, never after it", font_parser_coords_extreme),
    pictured!("work-bound", "Every corpus file loads, or fails to, within section 12.3's step bounds for a load and for each glyph's outline, flattening and rasterization; and the independent 1,024 × 1,024 square fixture rasterizes in at least 8,388,608 and at most 10,699,844 steps", font_parser_work_bound),
    pictured!("alloc-bound", "No corpus file makes the parser allocate more than section 12.3's limits", font_parser_alloc_bound),
    pictured!("corpus", "Each of the 64 malformed corpus files yields its documented error, never a panic", font_parser_corpus),
];

// ---------------------------------------------------------------------------------------
// raster: shapes, blits, clipping & alpha (contract sections 2 to 7)
// ---------------------------------------------------------------------------------------

/// Round-to-nearest of `x / 255` as `(2x + 255) / 510`, written independently of the
/// shift form section 1 gives libgfx (`x / 255` is never halfway between integers).
fn nearest255(x: u32) -> u32 {
    (2 * x + 255) / 510
}

/// Section 2's source-over on premultiplied `[b, g, r, a]`, by `nearest255`, unclamped.
fn source_over(s: [u8; 4], d: [u8; 4]) -> [u32; 4] {
    let inv = 255 - s[3] as u32;
    [0, 1, 2, 3].map(|i| s[i] as u32 + nearest255(d[i] as u32 * inv))
}

/// A colour of its own for each pixel (i, j) of a blit source.
fn tile_colour(i: usize, j: usize) -> Color {
    Color::rgb((40 * i + 7) as u8, (60 * j + 11) as u8, (13 * i + 29 * j + 100) as u8)
}

/// A 5 × 4 owned source of `format` holding `tile_colour`.
fn tile(format: Format) -> Result<FrameBuf, String> {
    let mut fb = FrameBuf::new(5, 4, format).map_err(|e| format!("FrameBuf::new(5, 4, {format:?}) failed: {e}"))?;
    for j in 0..4 {
        for i in 0..5 {
            fb.put_pixel(i, j, tile_colour(i, j));
        }
    }
    Ok(fb)
}

/// Write `[b, g, r, a]` at (x, y) of a 4-byte surface in its own memory order, by hand.
fn poke(fb: &FrameBuf, x: usize, y: usize, [b, g, r, a]: [u8; 4]) {
    let bytes = if fb.is_bgr { [b, g, r, a] } else { [r, g, b, a] };
    // SAFETY: (x, y) is inside the 4-byte surface.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), fb.raw_ptr().add(y * fb.stride + x * 4), 4) }
}

/// The four bytes at (x, y) of a 4-byte surface as `[b, g, r, a]`, by hand.
fn peek(fb: &FrameBuf, x: usize, y: usize) -> [u8; 4] {
    let mut p = [0u8; 4];
    // SAFETY: (x, y) is inside the 4-byte surface.
    unsafe { std::ptr::copy_nonoverlapping(fb.raw_ptr().add(y * fb.stride + x * 4), p.as_mut_ptr(), 4) }
    if fb.is_bgr { p } else { [p[2], p[1], p[0], p[3]] }
}

/// A rectangle on a 32 × 24 surface, intersected with it.
fn on_surface(x: i64, y: i64, w: i64, h: i64) -> Option<(i32, i32, i32, i32)> {
    let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + w).min(32), (y + h).min(24));
    (x0 < x1 && y0 < y1).then(|| (x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// Primitives that draw something inside (5, 4, 12, 9) on a 32 × 24 surface.
fn clip_calls() -> Vec<(&'static str, Box<dyn Fn(&mut FrameBuf)>)> {
    vec![
        ("fill_rect(0, 0, 32, 24)", Box::new(|fb| shapes::fill_rect(fb, 0, 0, 32, 24, INK))),
        ("fill_rect(10, 6, 20, 20)", Box::new(|fb| shapes::fill_rect(fb, 10, 6, 20, 20, INK))),
        ("draw_rect(7, 6, 20, 15)", Box::new(|fb| shapes::draw_rect(fb, 7, 6, 20, 15, INK))),
        ("draw_line(0, 0, 31, 23)", Box::new(|fb| shapes::draw_line(fb, 0, 0, 31, 23, INK))),
        ("draw_line(30, 2, 1, 20)", Box::new(|fb| shapes::draw_line(fb, 30, 2, 1, 20, INK))),
        ("fill_circle(12, 10, 6)", Box::new(|fb| shapes::fill_circle(fb, 12, 10, 6, INK))),
        ("draw_circle(11, 8, 5)", Box::new(|fb| shapes::draw_circle(fb, 11, 8, 5, INK))),
        ("rounded_rect(2, 2, 25, 18, 6)", Box::new(|fb| shapes::rounded_rect(fb, 2, 2, 25, 18, 6, INK))),
        ("gradient_v(0, 0, 32, 24)", Box::new(|fb| shapes::gradient_v(fb, 0, 0, 32, 24, INK, WHITE))),
        ("put_pixel(8, 6) and (2, 2)", Box::new(|fb| { fb.put_pixel(8, 6, INK); fb.put_pixel(2, 2, INK); })),
        ("clear", Box::new(|fb| fb.clear(INK))),
        ("font::draw_text(\"A1\", 4, 5, scale 2)", Box::new(|fb| font5x7::draw_text(fb, b"A1", 4, 5, INK, 2))),
        ("bitmap_font::draw_char('A', 6, 3)", Box::new(|fb| bitmap_font::draw_char(fb, 'A', 6, 3, INK))),
        ("copy_row_from(0, 7, 32 pixels)", Box::new(|fb| {
            let row: Vec<u8> = (0..32).flat_map(|_| [INK.b, INK.g, INK.r, 0xFF]).collect();
            fb.copy_row_from(0, 7, &row, row.len());
        })),
        ("blit(tile, 3, 3)", Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit(fb, &t, 3, 3) })),
        ("blit_over(tile, 9, 8)", Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit_over(fb, &t, 9, 8) })),
        ("blit_scaled(tile, 0, 0, 3)", Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit_scaled(fb, &t, 0, 0, 3) })),
    ]
}

/// A padded BGRX surface: stride 12 bytes past the pixels, so every row has a sentinel.
fn padded(width: usize, height: usize) -> Guarded {
    let mut g = Guarded::new(width, height, width * 4 + 12, 4, true);
    g.reset(PAPER);
    g
}

/// Draw `f` on a fresh padded surface and require exactly the pixels `inside` in INK.
fn exact(section: &str, what: &str, width: usize, height: usize, f: impl Fn(&mut FrameBuf),
    inside: impl Fn(i64, i64) -> bool) -> CaseResult {
    let g = padded(width, height);
    f(&mut g.fb());
    step(section, g.compare(what, |x, y| if inside(x as i64, y as i64) { INK } else { PAPER }))
}

fn raster_fill_rect() -> CaseResult {
    exact("5.1", "fill_rect(5, 3, 10, 7)", 32, 24, |fb| shapes::fill_rect(fb, 5, 3, 10, 7, INK),
        |x, y| in_rect(x, y, 5, 3, 10, 7))?;
    exact("5.1", "fill_rect(0, 0, 32, 24)", 32, 24, |fb| shapes::fill_rect(fb, 0, 0, 32, 24, INK),
        |x, y| in_rect(x, y, 0, 0, 32, 24))
}

fn raster_fill_rect_clip_left() -> CaseResult {
    exact("5.1, 4", "fill_rect(-4, 2, 10, 5)", 32, 24, |fb| shapes::fill_rect(fb, -4, 2, 10, 5, INK),
        |x, y| in_rect(x, y, -4, 2, 10, 5))
}

fn raster_fill_rect_offscreen_left() -> CaseResult {
    for &(x, w) in &[(-20, 10), (-11, 10), (-1_000, 999)] {
        step("5.1, 4", isolated(&format!("fill_rect({x}, 2, {w}, 5)"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, 2, w, 5, INK);
            g.compare(&format!("fill_rect({x}, 2, {w}, 5) wholly left of the surface"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_offscreen_top() -> CaseResult {
    for &(y, h) in &[(-20, 10), (-6, 6), (-1_000, 999)] {
        step("5.1, 4", isolated(&format!("fill_rect(3, {y}, 10, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), 3, y, 10, h, INK);
            g.compare(&format!("fill_rect(3, {y}, 10, {h}) wholly above the surface"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_overflow() -> CaseResult {
    let cases: [(i32, i32, i32, i32); 5] = [
        (i32::MAX - 3, 2, 10, 5),
        (2, i32::MAX - 3, 5, 10),
        (10, 2, i32::MAX, 3),
        (2, 10, 3, i32::MAX),
        (i32::MIN + 2, 4, i32::MAX, 2),
    ];
    for (x, y, w, h) in cases {
        step("5.1, 1", isolated(&format!("fill_rect({x}, {y}, {w}, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, y, w, h, INK);
            g.compare(&format!("fill_rect({x}, {y}, {w}, {h})"),
                |px, py| if in_rect(px as i64, py as i64, x as i64, y as i64, w as i64, h as i64) { INK } else { PAPER })
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_empty() -> CaseResult {
    for &(x, y, w, h) in &[(5, 5, 0, 4), (5, 5, -3, 4), (1, 5, -3, 4), (5, 5, 4, 0), (5, 1, 4, -3), (1, 1, -5, -5)] {
        step("5.1", isolated(&format!("fill_rect({x}, {y}, {w}, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, y, w, h, INK);
            g.compare(&format!("fill_rect({x}, {y}, {w}, {h}) is empty"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_draw_rect() -> CaseResult {
    for &(x, y, w, h) in &[(4, 3, 9, 6), (2, 2, 1, 1), (6, 6, 2, 5), (-3, -2, 12, 8)] {
        exact("5.2", &format!("draw_rect({x}, {y}, {w}, {h})"), 32, 24,
            |fb| shapes::draw_rect(fb, x, y, w, h, INK),
            |px, py| in_rect(px, py, x as i64, y as i64, w as i64, h as i64)
                && !in_rect(px, py, x as i64 + 1, y as i64 + 1, w as i64 - 2, h as i64 - 2))?;
    }
    Ok(())
}

/// The pixels the contract's line algorithm walks (section 5.3).
fn line_path(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i64, i64)> {
    let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut err, mut x, mut y) = (dx + dy, x0, y0);
    let mut pts = Vec::new();
    loop {
        pts.push((x, y));
        if x == x1 && y == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
    pts
}

fn raster_line_axes() -> CaseResult {
    for &(x0, y0, x1, y1) in &[(2, 5, 12, 5), (12, 8, 3, 8), (7, 1, 7, 9), (9, 20, 9, 11), (4, 4, 4, 4)] {
        exact("5.3", &format!("draw_line({x0}, {y0}, {x1}, {y1})"), 32, 24,
            |fb| shapes::draw_line(fb, x0, y0, x1, y1, INK),
            |x, y| x >= x0.min(x1) as i64 && x <= x0.max(x1) as i64 && y >= y0.min(y1) as i64 && y <= y0.max(y1) as i64)?;
    }
    Ok(())
}

fn raster_line_bresenham() -> CaseResult {
    for &((x0, y0, x1, y1), pixels) in LINE_TABLE {
        // The literal table is the expectation; the walk here only confirms the transcription.
        if line_path(x0, y0, x1, y1).iter().map(|&(x, y)| (x as i32, y as i32)).collect::<Vec<_>>() != pixels {
            return at("5.3", format!("the suite's transcription of line {x0},{y0} -> {x1},{y1} disagrees with its table"));
        }
        let (ox, oy) = (3, 3);
        exact("5.3", &format!("draw_line({x0}, {y0}, {x1}, {y1}) at offset (3, 3)"), 20, 20,
            |fb| shapes::draw_line(fb, x0 + ox, y0 + oy, x1 + ox, y1 + oy, INK),
            |x, y| pixels.iter().any(|&(px, py)| (px + ox) as i64 == x && (py + oy) as i64 == y))?;
    }
    Ok(())
}

fn raster_line_clip() -> CaseResult {
    for &(x0, y0, x1, y1) in &[(-20, -10, 40, 15), (31, -5, -4, 30), (-100_000, -3, 100_000, 40), (5, 23, 5, 1_000)] {
        let path = line_path(x0, y0, x1, y1);
        step("5.3", isolated(&format!("draw_line({x0}, {y0}, {x1}, {y1})"), 4_000, || {
            let g = padded(32, 24);
            shapes::draw_line(&mut g.fb(), x0, y0, x1, y1, INK);
            let on: std::collections::HashSet<(i64, i64)> =
                path.iter().copied().filter(|&(x, y)| in_rect(x, y, 0, 0, 32, 24)).collect();
            g.compare(&format!("draw_line({x0}, {y0}, {x1}, {y1})"),
                |x, y| if on.contains(&(x as i64, y as i64)) { INK } else { PAPER })
        }))?;
    }
    Ok(())
}

fn raster_circle_fill() -> CaseResult {
    for r in 0..=12 {
        exact("5.4", &format!("fill_circle(16, 16, {r})"), 40, 40,
            |fb| shapes::fill_circle(fb, 16, 16, r, INK),
            |x, y| (x - 16) * (x - 16) + (y - 16) * (y - 16) <= (r as i64) * (r as i64))?;
    }
    exact("5.4", "fill_circle(16, 16, -1)", 40, 40, |fb| shapes::fill_circle(fb, 16, 16, -1, INK), |_, _| false)
}

fn raster_circle_fill_offscreen_left() -> CaseResult {
    for &(cx, cy, r) in &[(-3, 10, 6), (-30, 10, 5), (-6, 10, 5), (-1_000, 12, 20)] {
        step("5.4", isolated(&format!("fill_circle({cx}, {cy}, {r})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_circle(&mut g.fb(), cx, cy, r, INK);
            g.compare(&format!("fill_circle({cx}, {cy}, {r})"), |x, y| {
                let (dx, dy) = (x as i64 - cx as i64, y as i64 - cy as i64);
                if dx * dx + dy * dy <= (r as i64) * (r as i64) { INK } else { PAPER }
            })
        }))?;
    }
    Ok(())
}

/// The midpoint circle's pixels from the contract's octant table (section 5.5).
fn circle_set(r: i32) -> std::collections::HashSet<(i64, i64)> {
    let mut set = std::collections::HashSet::new();
    if r < 0 { return set; }
    for (y, &x) in CIRCLE_OCTANTS[r as usize].iter().enumerate() {
        let (x, y) = (x as i64, y as i64);
        for (a, b) in [(x, y), (y, x)] {
            for (sa, sb) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                set.insert((sa * a, sb * b));
            }
        }
    }
    set
}

fn raster_circle_outline() -> CaseResult {
    let mut g = padded(91, 91);
    for r in 0..=40 {
        g.reset(PAPER);
        shapes::draw_circle(&mut g.fb(), 45, 45, r, INK);
        let set = circle_set(r);
        step("5.5", g.compare(&format!("draw_circle(45, 45, {r})"),
            |x, y| if set.contains(&(x as i64 - 45, y as i64 - 45)) { INK } else { PAPER }))?;
    }
    Ok(())
}

/// Each primitive at extreme coordinates, by name.
fn extreme_calls() -> Vec<(&'static str, Box<dyn Fn(&mut FrameBuf)>)> {
    vec![
        ("fill_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("fill_rect(MAX, MAX, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MAX, i32::MAX, i32::MAX, i32::MAX, INK))),
        ("fill_rect(-MAX, 3, 5, 5)", Box::new(|fb| shapes::fill_rect(fb, -i32::MAX, 3, 5, 5, INK))),
        ("draw_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::draw_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("draw_rect(MAX - 1, 0, 9, 9)", Box::new(|fb| shapes::draw_rect(fb, i32::MAX - 1, 0, 9, 9, INK))),
        ("fill_circle(MAX - 1, 5, 5)", Box::new(|fb| shapes::fill_circle(fb, i32::MAX - 1, 5, 5, INK))),
        ("fill_circle(MIN + 1, 5, 5)", Box::new(|fb| shapes::fill_circle(fb, i32::MIN + 1, 5, 5, INK))),
        ("fill_circle(5, 5, 46340)", Box::new(|fb| shapes::fill_circle(fb, 5, 5, 46_340, INK))),
        ("draw_circle(MIN + 9, 5, 5)", Box::new(|fb| shapes::draw_circle(fb, i32::MIN + 9, 5, 5, INK))),
        ("draw_circle(5, 5, 1000000)", Box::new(|fb| shapes::draw_circle(fb, 5, 5, 1_000_000, INK))),
        ("draw_line(MIN/2, 3, MAX/2, 4)", Box::new(|fb| shapes::draw_line(fb, i32::MIN / 2, 3, i32::MAX / 2, 4, INK))),
        ("draw_line(5, MIN/2, 6, MAX/2)", Box::new(|fb| shapes::draw_line(fb, 5, i32::MIN / 2, 6, i32::MAX / 2, INK))),
        ("copy_row_from(width - 1, height - 1, 64 bytes)", Box::new(|fb| {
            let (w, h) = (fb.width, fb.height);
            fb.copy_row_from(w - 1, h - 1, &[0x5Au8; 64], 64)
        })),
        ("font::draw_text at (width - 3, height - 3)", Box::new(|fb| {
            let (w, h) = (fb.width, fb.height);
            font5x7::draw_text(fb, b"WWWW", w - 3, h - 3, INK, 3)
        })),
    ]
}

// gpu-raster: GPU copies must respect guarded user buffers; device draws must preserve a canary target.
fn raster_guard_pages() -> CaseResult {
crate::gpu_checks::guard_pages()
}

// gpu-raster: §2.2 replaces CPU per-pixel loop accounting with setup and GPU estimates.
fn raster_extreme_work_bound() -> CaseResult {
    // gpu-raster: §7 bounds setup separately; COUNTERS certifies emitted device work.
    const MAX: i32 = i32::MAX;
    const MIN: i32 = i32::MIN;
    const HALF: i32 = 1 << 30;
    type Extreme = (&'static str, Option<Rect>, Box<dyn Fn(&mut FrameBuf)>, Option<(i64, i64, i64, i64)>);
    let small_clip = Some(Rect::new(10, 8, 4, 3));
    let pixel_clip = Some(Rect::new(10, 8, 1, 1));
    // Each call, an optional clip, and the clipped bounding rectangle the contract's damage
    // table gives it on a 32 × 24 surface (sections 6 and 7), worked out by hand.
    let calls: Vec<Extreme> = vec![
        ("fill_rect(MIN, MIN, MAX, MAX)", None, Box::new(|fb| shapes::fill_rect(fb, MIN, MIN, MAX, MAX, INK)), None),
        ("fill_rect(MAX, MAX, MAX, MAX)", None, Box::new(|fb| shapes::fill_rect(fb, MAX, MAX, MAX, MAX, INK)), None),
        ("fill_rect(-MAX, 3, 5, 5)", None, Box::new(|fb| shapes::fill_rect(fb, -MAX, 3, 5, 5, INK)), None),
        ("fill_rect(-2^30, -2^30, MAX, MAX)", None, Box::new(|fb| shapes::fill_rect(fb, -HALF, -HALF, MAX, MAX, INK)), Some((0, 0, 32, 24))),
        ("draw_rect(MIN, MIN, MAX, MAX)", None, Box::new(|fb| shapes::draw_rect(fb, MIN, MIN, MAX, MAX, INK)), None),
        ("draw_rect(-2^30, -2^30, MAX, MAX)", None, Box::new(|fb| shapes::draw_rect(fb, -HALF, -HALF, MAX, MAX, INK)), Some((0, 0, 32, 24))),
        ("draw_rect(MAX - 1, 0, 9, 9)", None, Box::new(|fb| shapes::draw_rect(fb, MAX - 1, 0, 9, 9, INK)), None),
        ("fill_circle(MAX - 1, 5, 5)", None, Box::new(|fb| shapes::fill_circle(fb, MAX - 1, 5, 5, INK)), None),
        ("fill_circle(MIN + 1, 5, 5)", None, Box::new(|fb| shapes::fill_circle(fb, MIN + 1, 5, 5, INK)), None),
        ("fill_circle(5, 5, 46340)", None, Box::new(|fb| shapes::fill_circle(fb, 5, 5, 46_340, INK)), Some((0, 0, 32, 24))),
        ("fill_circle(16, 12, MAX)", None, Box::new(|fb| shapes::fill_circle(fb, 16, 12, MAX, INK)), Some((0, 0, 32, 24))),
        ("fill_circle(16, 12, MAX) clipped to (10, 8, 4, 3)", small_clip, Box::new(|fb| shapes::fill_circle(fb, 16, 12, MAX, INK)), Some((10, 8, 4, 3))),
        ("draw_circle(MIN + 9, 5, 5)", None, Box::new(|fb| shapes::draw_circle(fb, MIN + 9, 5, 5, INK)), None),
        ("draw_circle(5, 5, 1000000)", None, Box::new(|fb| shapes::draw_circle(fb, 5, 5, 1_000_000, INK)), Some((0, 0, 32, 24))),
        ("draw_circle(16, 12, MAX)", None, Box::new(|fb| shapes::draw_circle(fb, 16, 12, MAX, INK)), Some((0, 0, 32, 24))),
        ("draw_circle(16, 12, 1000000) clipped to (10, 8, 4, 3)", small_clip, Box::new(|fb| shapes::draw_circle(fb, 16, 12, 1_000_000, INK)), Some((10, 8, 4, 3))),
        ("draw_circle(16, 12, 1000000) clipped to (10, 8, 1, 1)", pixel_clip, Box::new(|fb| shapes::draw_circle(fb, 16, 12, 1_000_000, INK)), Some((10, 8, 1, 1))),
        ("draw_line(MIN/2, 3, MAX/2, 4)", None, Box::new(|fb| shapes::draw_line(fb, MIN / 2, 3, MAX / 2, 4, INK)), Some((0, 3, 32, 2))),
        ("draw_line(5, MIN/2, 6, MAX/2)", None, Box::new(|fb| shapes::draw_line(fb, 5, MIN / 2, 6, MAX / 2, INK)), Some((5, 0, 2, 24))),
        ("draw_line(MIN, MIN, MAX, MAX)", None, Box::new(|fb| shapes::draw_line(fb, MIN, MIN, MAX, MAX, INK)), Some((0, 0, 32, 24))),
        ("draw_line(MIN, 30, MAX, 40)", None, Box::new(|fb| shapes::draw_line(fb, MIN, 30, MAX, 40, INK)), None),
        ("draw_line(MAX, MIN, MIN, MAX) clipped to (10, 8, 4, 3)", small_clip, Box::new(|fb| shapes::draw_line(fb, MAX, MIN, MIN, MAX, INK)), Some((10, 8, 4, 3))),
        ("rounded_rect(MIN, MIN, MAX, MAX, MAX)", None, Box::new(|fb| shapes::rounded_rect(fb, MIN, MIN, MAX, MAX, MAX, INK)), None),
        ("rounded_rect(-2^30, -2^30, MAX, MAX, MAX)", None, Box::new(|fb| shapes::rounded_rect(fb, -HALF, -HALF, MAX, MAX, MAX, INK)), Some((0, 0, 32, 24))),
        ("gradient_v(-2^30, -2^30, MAX, MAX)", None, Box::new(|fb| shapes::gradient_v(fb, -HALF, -HALF, MAX, MAX, INK, WHITE)), Some((0, 0, 32, 24))),
        ("gradient_v(MIN, 0, 10, 24)", None, Box::new(|fb| shapes::gradient_v(fb, MIN, 0, 10, 24, INK, WHITE)), None),
        ("blit_scaled(5 x 4, MIN, 0, 2^30)", None, Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit_scaled(fb, &t, MIN, 0, HALF) }), Some((0, 0, 32, 24))),
        ("blit(5 x 4, MAX - 2, 0)", None, Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit(fb, &t, MAX - 2, 0) }), None),
        ("blit_over(5 x 4, MIN, MIN)", None, Box::new(|fb| if let Ok(t) = tile(Format::BGRX) { blit::blit_over(fb, &t, MIN, MIN) }), None),
        ("clear", None, Box::new(|fb| fb.clear(INK)), Some((0, 0, 32, 24))),
        ("copy_row_from(31, 23, 64 bytes)", None, Box::new(|fb| fb.copy_row_from(31, 23, &[0x5Au8; 64], 64)), Some((31, 23, 1, 1))),
        ("font::draw_text(\"WWWW\", 29, 21, scale 3)", None, Box::new(|fb| font5x7::draw_text(fb, b"WWWW", 29, 21, INK, 3)), Some((29, 21, 3, 3))),
        ("font::draw_text(1,000 bytes, 0, 0, scale 1)", None, Box::new(|fb| font5x7::draw_text(fb, &[b'W'; 1_000], 0, 0, INK, 1)), Some((0, 0, 32, 7))),
        ("font::draw_text(1,000 bytes, 2^31, 3, scale 1)", None, Box::new(|fb| font5x7::draw_text(fb, &[b'W'; 1_000], 1 << 31, 3, INK, 1)), None),
    ];
    for (name, clip, call, rect) in calls {
        step("7", isolated(name, 2_000, || {
            let g = padded(32, 24);
            let mut fb = g.fb();
            if let Some(c) = clip {
                fb.push_clip(c);
            }
            let before = crate::gpu_checks::counters()?;
            work::reset();
            call(&mut fb);
            let after = crate::gpu_checks::counters()?;
            let steps = work::steps();
            if steps > 64 {
                return Err(format!("gpu-raster: {name} took {steps} CPU geometry steps; the per-primitive bound is 64"));
            }
            let area = rect.map_or(0, |(_, _, w, h)| (w*h) as u64);
            let emitted = after.fragment_area - before.fragment_area;
            let draws = after.draws - before.draws;
            if emitted > area || (area == 0 && draws != 0) || (area != 0 && draws == 0) {
                return Err(format!("gpu-raster: {name} emitted {emitted} fragments in {draws} draws for clipped area {area}"));
            }
            if after.work_estimate - before.work_estimate < emitted {
                return Err(format!("gpu-raster: {name} undercounted its GPU work estimate"));
            }
            g.sentinels()
        }))?;
    }
    Ok(())
}

/// A scene touching every primitive, in bounds.
fn scene(fb: &mut FrameBuf) {
    shapes::fill_rect(fb, 2, 2, 9, 5, Color::rgb(250, 10, 20));
    shapes::draw_rect(fb, 1, 9, 12, 7, Color::rgb(10, 240, 30));
    shapes::draw_line(fb, 0, 0, 13, 17, Color::rgb(20, 30, 250));
    shapes::fill_circle(fb, 7, 12, 3, Color::rgb(200, 200, 10));
    shapes::draw_circle(fb, 6, 6, 4, Color::rgb(10, 200, 200));
    fb.put_pixel(13, 1, Color::rgb(123, 45, 67));
    font5x7::draw_text(fb, b"A1", 1, 11, Color::rgb(90, 80, 70), 1);
}

// gpu-raster: §2.2 keeps padding sentinels around UPLOAD and READBACK rows.
fn raster_stride_padding() -> CaseResult {
crate::gpu_checks::stride_padding()
}

// gpu-raster: §2.2 removes pointer-only errors; target limits and renderable formats remain exact.
fn raster_surface_validate() -> CaseResult {
use libbreenix::gpu::{self, abi as ga};
    let (ret,caps)=gpu::caps();
    ensure(ret==0 && caps.status==ga::status::ADMITTED,"3","gpu-raster: no admitted GPU target implementation")?;
    for (width,height,format) in [(0,4,0),(4,0,0),(caps.max_width+1,1,0),(1,caps.max_height+1,0),(4,4,u32::MAX),(4,4,ga::format::R8)] {
        let got=gpu::target_create(format,width,height);
        if let Ok(handle)=got {gpu::resource_destroy(handle);}
        ensure(got==Err(-22),"3",format!("gpu-raster: target {width}x{height} format {format} returned {got:?}; Empty, TooLarge and Format are EINVAL"))?;
    }
    for (width,height,format) in [(caps.max_width,1,0),(1,caps.max_height,1),(1,1,0)] {
        let handle=gpu::target_create(format,width,height).map_err(|e|format!("gpu-raster: valid target {width}x{height}: {}",outcome(e))).cite("3")?;
        ensure(gpu::resource_destroy(handle)==0,"3","gpu-raster: cannot destroy valid target")?;
    }Ok(())
}

fn raster_bpp3_bpp4() -> CaseResult {
    let three = Guarded::new(14, 18, 14 * 3 + 2, 3, false);
    let four = Guarded::new(14, 18, 14 * 4, 4, true);
    scene(&mut three.fb());
    scene(&mut four.fb());
    ensure(drawn_pixels(&four, Color::rgb(0, 0, 0)) > 0, "5", "the scene drew nothing on the 4-byte surface")?;
    *three.reference.borrow_mut() = Some(canonical_pixels(&four));
    three.judge();
    for y in 0..18 {
        for x in 0..14 {
            if three.color(x, y) != four.color(x, y) {
                return at("2", format!("pixel ({x},{y}) is {} on the 3-byte RGB surface and {} on the 4-byte BGRX one",
                    hex(three.color(x, y)), hex(four.color(x, y))));
            }
        }
    }
    step("3", three.sentinels())
}

fn raster_bgr_order() -> CaseResult {
    let bgr = Guarded::new(14, 18, 14 * 4, 4, true);
    let rgb = Guarded::new(14, 18, 14 * 4, 4, false);
    scene(&mut bgr.fb());
    scene(&mut rgb.fb());
    ensure(drawn_pixels(&bgr, Color::rgb(0, 0, 0)) > 0, "5", "the scene drew nothing on the BGR surface")?;
    *bgr.reference.borrow_mut() = Some(canonical_pixels(&rgb));
    bgr.judge();
    for y in 0..18 {
        for x in 0..14 {
            // SAFETY: (x, y) is inside both surfaces.
            let (a, b) = unsafe {
                let a = bgr.base.add(y * bgr.stride + x * 4);
                let b = rgb.base.add(y * rgb.stride + x * 4);
                ([*a, *a.add(1), *a.add(2), *a.add(3)], [*b, *b.add(1), *b.add(2), *b.add(3)])
            };
            if a != [b[2], b[1], b[0], b[3]] {
                return at("2", format!("pixel ({x},{y}) is {a:02x?} on the BGR surface and {b:02x?} on the RGB one; only bytes 0 and 2 may swap"));
            }
        }
    }
    Ok(())
}

fn raster_x_byte() -> CaseResult {
    let calls: Vec<(&str, Box<dyn Fn(&mut FrameBuf)>)> = vec![
        ("put_pixel", Box::new(|fb| fb.put_pixel(3, 3, INK))),
        ("fill_rect", Box::new(|fb| shapes::fill_rect(fb, 2, 2, 5, 4, INK))),
        ("draw_rect", Box::new(|fb| shapes::draw_rect(fb, 1, 1, 9, 7, INK))),
        ("draw_line", Box::new(|fb| shapes::draw_line(fb, 0, 0, 11, 6, INK))),
        ("fill_circle", Box::new(|fb| shapes::fill_circle(fb, 6, 6, 3, INK))),
        ("draw_circle", Box::new(|fb| shapes::draw_circle(fb, 6, 6, 4, INK))),
        ("clear", Box::new(|fb| fb.clear(INK))),
        ("font::draw_text", Box::new(|fb| font5x7::draw_text(fb, b"A", 1, 1, INK, 1))),
        ("bitmap_font::draw_char", Box::new(|fb| bitmap_font::draw_char(fb, 'A', 0, 0, INK))),
    ];
    for (name, call) in calls {
        let g = Guarded::bgrx(16, 16);
        call(&mut g.fb());
        for y in 0..16 {
            for x in 0..16 {
                if g.x_byte(x, y) != 0xFF {
                    return at("2", format!("{name} wrote X byte {:#04x} at ({x},{y}); the contract requires 0xFF", g.x_byte(x, y)));
                }
            }
        }
    }
    Ok(())
}

fn raster_copy_row_clip() -> CaseResult {
    step("5.9", isolated("copy_row_from(5, 1, 40 bytes)", 2_000, || {
        let mut g = Guarded::new(8, 4, 8 * 4 + 8, 4, true);
        g.reset(PAPER);
        let src: Vec<u8> = (0..40u8).map(|i| [INK.b, INK.g, INK.r, 0xFF][i as usize % 4]).collect();
        g.fb().copy_row_from(5, 1, &src, 40);
        g.compare("copy_row_from(5, 1, 40 bytes) on an 8-pixel row",
            |x, y| if y == 1 && x >= 5 { INK } else { PAPER })?;
        g.fb().copy_row_from(8, 2, &src, 40);
        g.compare("copy_row_from(8, 2, ...) at x = width",
            |x, y| if y == 1 && x >= 5 { INK } else { PAPER })
    }))
}

fn raster_premultiply() -> CaseResult {
    for a in 0..=255u32 {
        for c in 0..=255u32 {
            let (got, want) = (alpha::premultiply(c as u8, a as u8) as u32, nearest255(c * a));
            if got != want {
                return at("2", format!("premultiply({c}, {a}) is {got}; div255({}) is {want}", c * a));
            }
        }
    }
    Ok(())
}

fn raster_alpha_over_examples() -> CaseResult {
    for ((sc, sa), (dc, da), (oc, oa)) in [
        ((128u8, 128u8), (0u8, 255u8), (128u8, 255u8)),
        ((128, 128), (255, 255), (255, 255)),
        ((64, 128), (200, 255), (164, 255)),
        ((64, 128), (100, 100), (114, 178)),
        ((0, 0), (77, 77), (77, 77)),
        ((200, 255), (13, 50), (200, 255)),
    ] {
        let got = alpha::over([sc, sc, sc, sa], [dc, dc, dc, da]);
        if got != [oc, oc, oc, oa] {
            return at("2", format!("over(({sc}, {sa}), ({dc}, {da})) is {got:?}; the contract's table gives colour {oc}, alpha {oa}"));
        }
    }
    let got = alpha::over([64, 0, 32, 128], [100, 200, 0, 255]);
    ensure(got == [114, 100, 32, 255], "2", format!("over([64, 0, 32, 128], [100, 200, 0, 255]) is {got:?}; channel by channel it is [114, 100, 32, 255]"))
}

fn raster_alpha_over_sweep() -> CaseResult {
    let backgrounds = [[0u8, 0, 0, 255], [255, 255, 255, 255], [240, 128, 10, 255], [60, 30, 90, 120]];
    for sa in 0..=255u32 {
        let s = [nearest255(37 * sa) as u8, nearest255(200 * sa) as u8, nearest255(255 * sa) as u8, sa as u8];
        for d in backgrounds {
            let want = source_over(s, d);
            if let Some(i) = want.iter().position(|&v| v > 255) {
                return at("2", format!("source-over of {s:?} on {d:?} reaches {} in channel {i}; the contract's formula never exceeds 255 for valid inputs", want[i]));
            }
            let got = alpha::over(s, d);
            if got.map(|v| v as u32) != want {
                return at("2", format!("over({s:?}, {d:?}) is {got:?}; section 2's formula gives {want:?}"));
            }
        }
    }
    Ok(())
}

fn raster_alpha_extremes() -> CaseResult {
    let dsts = [[0u8, 0, 0, 0], [9, 8, 7, 10], [255, 255, 255, 255], [3, 200, 90, 255]];
    for d in dsts {
        for s in [[0u8, 0, 0, 255], [17, 34, 51, 255], [255, 255, 255, 255]] {
            let got = alpha::over(s, d);
            ensure(got == s, "2", format!("over({s:?}, {d:?}) is {got:?}; an alpha-255 source replaces the destination"))?;
        }
        let got = alpha::over([0; 4], d);
        ensure(got == d, "2", format!("over([0, 0, 0, 0], {d:?}) is {got:?}; an alpha-0 source leaves it unchanged"))?;
    }
    let src = FrameBuf::new(4, 1, Format::BGRA_PREMULTIPLIED).cite("3")?;
    poke(&src, 0, 0, [INK.b, INK.g, INK.r, 255]);
    poke(&src, 2, 0, [WHITE.b, WHITE.g, WHITE.r, 255]);
    let g = padded(32, 24);
    blit::blit_over(&mut g.fb(), &src, 6, 9);
    step("2, 5.8", g.compare("blit_over of [opaque, transparent, opaque, transparent] at (6, 9)", |x, y| match (x, y) {
        (6, 9) => INK,
        (8, 9) => WHITE,
        _ => PAPER,
    }))
}

fn raster_alpha_output() -> CaseResult {
    let dst = FrameBuf::new(16, 16, Format::BGRA_PREMULTIPLIED).cite("3")?;
    let src = FrameBuf::new(16, 16, Format::BGRA_PREMULTIPLIED).cite("3")?;
    let d_of = |x: usize, y: usize| { let a = (x * 16 + y) as u8; [a / 2, a / 3, a, a] };
    let s_of = |x: usize, y: usize| { let a = (y * 17) as u8; [a / 3, a.saturating_sub(x as u8), a / 5, a] };
    for y in 0..16 {
        for x in 0..16 {
            poke(&dst, x, y, d_of(x, y));
            poke(&src, x, y, s_of(x, y));
        }
    }
    let mut dst = dst;
    blit::blit_over(&mut dst, &src, 0, 0);
    for y in 0..16 {
        for x in 0..16 {
            let (got, want) = (peek(&dst, x, y), source_over(s_of(x, y), d_of(x, y)));
            if got.map(|v| v as u32) != want {
                return at("2, 3", format!("blit_over of {:?} onto the premultiplied pixel {:?} at ({x},{y}) stored {got:?}; section 2 gives {want:?}",
                    s_of(x, y), d_of(x, y)));
            }
        }
    }
    Ok(())
}

fn raster_blit() -> CaseResult {
    for format in [Format::BGRX, Format::RGBX, Format::RGB, Format::BGR] {
        let src = tile(format).cite("3")?;
        let g = padded(32, 24);
        blit::blit(&mut g.fb(), &src, 7, 3);
        step("5.8", g.compare(&format!("blit of a 5 x 4 {format:?} source at (7, 3)"),
            |x, y| if in_rect(x as i64, y as i64, 7, 3, 5, 4) { tile_colour(x - 7, y - 3) } else { PAPER }))?;
        for (x, y) in [(7, 3), (11, 6)] {
            ensure(g.x_byte(x, y) == 0xFF, "2", format!("blit of a {format:?} source wrote X byte {:#04x} at ({x},{y})", g.x_byte(x, y)))?;
        }
        let three = Guarded::new(32, 24, 32 * 3 + 6, 3, false);
        blit::blit(&mut three.fb(), &src, 20, 18);
        step("5.8", three.compare(&format!("blit of a {format:?} source onto a 3-byte RGB surface at (20, 18)"),
            |x, y| if in_rect(x as i64, y as i64, 20, 18, 5, 4) { tile_colour(x - 20, y - 18) } else { Color::rgb(0, 0, 0) }))?;
    }
    Ok(())
}

fn raster_blit_clip() -> CaseResult {
    let src = tile(Format::BGRX).cite("3")?;
    for (x, y) in [(-2, -1), (29, 21), (-4, 10), (30, -3), (-5, 0), (32, 0), (0, 24), (0, -4), (i32::MIN, i32::MIN)] {
        let g = padded(32, 24);
        let mut fb = g.fb();
        blit::blit(&mut fb, &src, x, y);
        step("5.8, 4", g.compare(&format!("blit of a 5 x 4 source at ({x}, {y}) on a 32 x 24 surface"), |px, py| {
            if in_rect(px as i64, py as i64, x as i64, y as i64, 5, 4) {
                tile_colour((px as i64 - x as i64) as usize, (py as i64 - y as i64) as usize)
            } else {
                PAPER
            }
        }))?;
        let (got, want) = (dirty(&mut fb), on_surface(x as i64, y as i64, 5, 4));
        ensure(got == want, "6", format!("blit at ({x}, {y}) marked {got:?}; the contract marks (x, y, 5, 4) intersected with the surface, {want:?}"))?;
    }
    Ok(())
}

fn raster_blit_alpha() -> CaseResult {
    let src = FrameBuf::new(5, 4, Format::BGRA_PREMULTIPLIED).cite("3")?;
    let s_of = |i: usize, j: usize| {
        let a = [0u32, 64, 128, 200, 255][i];
        let c = tile_colour(i, j);
        [nearest255(c.b as u32 * a) as u8, nearest255(c.g as u32 * a) as u8, nearest255(c.r as u32 * a) as u8, a as u8]
    };
    for j in 0..4 {
        for i in 0..5 {
            poke(&src, i, j, s_of(i, j));
        }
    }
    let g = padded(32, 24);
    blit::blit_over(&mut g.fb(), &src, 6, 5);
    step("2, 5.8", g.compare("blit_over of a premultiplied 5 x 4 source at (6, 5)", |x, y| {
        if in_rect(x as i64, y as i64, 6, 5, 5, 4) {
            let [b, g, r, _] = source_over(s_of(x - 6, y - 5), [PAPER.b, PAPER.g, PAPER.r, 255]);
            Color::rgb(r as u8, g as u8, b as u8)
        } else {
            PAPER
        }
    }))
}

fn raster_blit_straight() -> CaseResult {
    let alphas = [0u32, 1, 128, 254, 255];
    let straight = |i: usize, j: usize| Color::rgb(200, (100 + 30 * j) as u8, (30 + 40 * i) as u8);
    for is_bgr in [false, true] {
        let mut mem = vec![0u32; 5 * 4];
        let base = mem.as_mut_ptr() as *mut u8;
        for j in 0..4 {
            for i in 0..5 {
                let (c, a) = (straight(i, j), alphas[i] as u8);
                let bytes = if is_bgr { [c.b, c.g, c.r, a] } else { [c.r, c.g, c.b, a] };
                // SAFETY: (i, j) is inside the 5 × 4, 20-byte-stride buffer.
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(j * 20 + i * 4), 4) };
            }
        }
        // SAFETY: `mem` holds 5 × 4 four-byte pixels and outlives `src`.
        let src = unsafe { FrameBuf::from_raw(base, 5, 4, 20, 4, is_bgr) }.cite("3")?;
        let g = padded(32, 24);
        blit::blit_straight(&mut g.fb(), &src, 20, 2);
        let order = if is_bgr { "B, G, R, A" } else { "R, G, B, A" };
        step("2, 5.8", g.compare(&format!("blit_straight of a straight {order} source at (20, 2)"), |x, y| {
            if in_rect(x as i64, y as i64, 20, 2, 5, 4) {
                let (c, a) = (straight(x - 20, y - 2), alphas[x - 20]);
                let s = [nearest255(c.b as u32 * a) as u8, nearest255(c.g as u32 * a) as u8, nearest255(c.r as u32 * a) as u8, a as u8];
                let [b, g, r, _] = source_over(s, [PAPER.b, PAPER.g, PAPER.r, 255]);
                Color::rgb(r as u8, g as u8, b as u8)
            } else {
                PAPER
            }
        }))?;
        drop(src);
        drop(mem);
    }
    Ok(())
}

fn raster_blit_scale() -> CaseResult {
    let src = tile(Format::BGRX).cite("3")?;
    for (x, y, n) in [(2, 1, 3), (4, 4, 1), (-6, -5, 4), (25, 19, 2)] {
        let g = padded(32, 24);
        let mut fb = g.fb();
        blit::blit_scaled(&mut fb, &src, x, y, n);
        let (x, y, n) = (x as i64, y as i64, n as i64);
        step("5.8", g.compare(&format!("blit_scaled of a 5 x 4 source at ({x}, {y}), scale {n}"), |px, py| {
            if in_rect(px as i64, py as i64, x, y, 5 * n, 4 * n) {
                tile_colour(((px as i64 - x) / n) as usize, ((py as i64 - y) / n) as usize)
            } else {
                PAPER
            }
        }))?;
        let (got, want) = (dirty(&mut fb), on_surface(x, y, 5 * n, 4 * n));
        ensure(got == want, "6", format!("blit_scaled at ({x}, {y}), scale {n} marked {got:?}; the contract gives {want:?}"))?;
    }
    for n in [0, -2] {
        let g = padded(32, 24);
        let mut fb = g.fb();
        blit::blit_scaled(&mut fb, &src, 3, 3, n);
        step("5.8", g.compare(&format!("blit_scaled with scale {n}"), |_, _| PAPER))?;
        ensure(dirty(&mut fb).is_none(), "6", format!("blit_scaled with scale {n} draws nothing, so marks nothing"))?;
    }
    Ok(())
}

fn raster_clip_rect() -> CaseResult {
    for (name, call) in clip_calls() {
        let free = padded(32, 24);
        call(&mut free.fb());
        let inside = (4..13).flat_map(|y| (5..17).map(move |x| (x, y))).filter(|&(x, y)| free.color(x, y) != PAPER).count();
        ensure(inside > 0, "4", format!("{name} drew nothing inside (5, 4, 12, 9) unclipped, so it cannot show the clip"))?;
        let g = padded(32, 24);
        let mut fb = g.fb();
        fb.push_clip(Rect::new(5, 4, 12, 9));
        call(&mut fb);
        step("4", g.compare(&format!("{name} with the clip (5, 4, 12, 9)"),
            |x, y| if in_rect(x as i64, y as i64, 5, 4, 12, 9) { free.color(x, y) } else { PAPER }))?;
        if let Some((dx, dy, dw, dh)) = dirty(&mut fb) {
            ensure(dx >= 5 && dy >= 4 && dx + dw <= 17 && dy + dh <= 13, "4, 6",
                format!("{name} with the clip (5, 4, 12, 9) marked ({dx}, {dy}, {dw}, {dh}), outside the clip"))?;
        }
    }
    Ok(())
}

fn raster_clip_stack() -> CaseResult {
    let (c1, c2, c3, c4) = (Color::rgb(250, 0, 0), Color::rgb(0, 250, 0), Color::rgb(0, 0, 250), Color::rgb(250, 250, 0));
    let g = padded(32, 24);
    let mut fb = g.fb();
    let clip = |fb: &FrameBuf| { let c = fb.clip(); (c.x, c.y, c.w, c.h) };
    ensure(clip(&fb) == (0, 0, 32, 24), "4", format!("with no clip pushed the clip is {:?}; the contract gives the whole surface", clip(&fb)))?;
    fb.push_clip(Rect::new(2, 2, 20, 15));
    fb.push_clip(Rect::new(10, -5, 30, 12));
    ensure(clip(&fb) == (10, 2, 12, 5), "4", format!("(10, -5, 30, 12) pushed inside (2, 2, 20, 15) gave the clip {:?}; their intersection is (10, 2, 12, 5)", clip(&fb)))?;
    shapes::fill_rect(&mut fb, 0, 0, 32, 24, c1);
    fb.pop_clip();
    ensure(clip(&fb) == (2, 2, 20, 15), "4", format!("one pop gave the clip {:?}; the contract restores (2, 2, 20, 15)", clip(&fb)))?;
    shapes::fill_rect(&mut fb, 0, 12, 32, 12, c2);
    fb.pop_clip();
    ensure(clip(&fb) == (0, 0, 32, 24), "4", format!("the second pop gave the clip {:?}; the contract restores the whole surface", clip(&fb)))?;
    shapes::fill_rect(&mut fb, 0, 20, 32, 4, c3);
    fb.pop_clip();
    ensure(clip(&fb) == (0, 0, 32, 24), "4", format!("a pop with nothing pushed changed the clip to {:?}", clip(&fb)))?;
    shapes::fill_rect(&mut fb, 30, 0, 2, 2, c4);
    step("4", g.compare("fills through two nested clips, each pop, and an extra pop", |x, y| {
        let (x, y) = (x as i64, y as i64);
        if in_rect(x, y, 30, 0, 2, 2) { c4 }
        else if in_rect(x, y, 0, 20, 32, 4) { c3 }
        else if in_rect(x, y, 2, 12, 20, 5) { c2 }
        else if in_rect(x, y, 10, 2, 12, 5) { c1 }
        else { PAPER }
    }))?;
    fb.push_clip(Rect::new(0, 0, 10, 10));
    fb.push_clip(Rect::new(20, 20, 5, 5));
    ensure(fb.clip().w <= 0 || fb.clip().h <= 0, "4", format!("disjoint nested clips gave {:?}; their intersection is empty", fb.clip()))
}

fn raster_clip_empty() -> CaseResult {
    let clips: [(&str, &[Rect]); 4] = [
        ("(5, 5, 0, 10)", &[Rect::new(5, 5, 0, 10)]),
        ("(5, 5, 10, -1)", &[Rect::new(5, 5, 10, -1)]),
        ("(40, 2, 5, 5) outside the surface", &[Rect::new(40, 2, 5, 5)]),
        ("(0, 0, 10, 10) then (20, 20, 5, 5)", &[Rect::new(0, 0, 10, 10), Rect::new(20, 20, 5, 5)]),
    ];
    for (what, rects) in clips {
        for (name, call) in clip_calls() {
            let g = padded(32, 24);
            let mut fb = g.fb();
            for &r in rects {
                fb.push_clip(r);
            }
            call(&mut fb);
            step("4", g.compare(&format!("{name} with the empty clip {what}"), |_, _| PAPER))?;
            let d = dirty(&mut fb);
            ensure(d.is_none(), "4, 6", format!("{name} with the empty clip {what} marked {d:?}; an empty clip marks no damage"))?;
        }
    }
    Ok(())
}

fn raster_rounded_rect() -> CaseResult {
    for (x, y, w, h, radius) in [(2, 3, 26, 18, 6), (0, 0, 9, 9, 100), (5, 5, 1, 7, 3), (-4, -6, 25, 30, 9), (3, 3, 12, 5, 0), (1, 2, 30, 21, 15), (20, 15, 20, 20, 7)] {
        let r = (radius as i64).min(w as i64 / 2).min(h as i64 / 2);
        let (xl, yl, wl, hl) = (x as i64, y as i64, w as i64, h as i64);
        // The corner predicate of section 5.6, mirrored to all four corners.
        let corner = |p: i64, lo: i64, len: i64| if p < lo + r { Some(2 * (lo + r - p) - 1) } else if p >= lo + len - r { Some(2 * (p - (lo + len - r)) + 1) } else { None };
        exact("5.6", &format!("rounded_rect({x}, {y}, {w}, {h}, {radius})"), 32, 24,
            |fb| shapes::rounded_rect(fb, x, y, w, h, radius, INK),
            |px, py| in_rect(px, py, xl, yl, wl, hl) && match (corner(px, xl, wl), corner(py, yl, hl)) {
                (Some(u), Some(v)) if r > 0 => u * u + v * v <= 4 * r * r,
                _ => true,
            })?;
        let g = padded(32, 24);
        let mut fb = g.fb();
        shapes::rounded_rect(&mut fb, x, y, w, h, radius, INK);
        let (got, want) = (dirty(&mut fb), on_surface(xl, yl, wl, hl));
        ensure(got == want, "6", format!("rounded_rect({x}, {y}, {w}, {h}, {radius}) marked {got:?}; the contract gives {want:?}"))?;
    }
    Ok(())
}

fn raster_gradient() -> CaseResult {
    let (top, bottom) = (Color::rgb(250, 0, 17), Color::rgb(3, 255, 200));
    for (x, y, w, h) in [(0, 0, 32, 1), (0, 0, 32, 2), (3, 4, 9, 3), (0, 0, 32, 24), (-3, 2, 10, 7), (5, -100, 20, 300), (1, 1, 30, 256)] {
        let (xl, yl, hl) = (x as i64, y as i64, h as i64);
        let colour = |py: i64| {
            let i = py - yl;
            let ch = |t: u8, b: u8| if hl == 1 { t } else { ((t as i64 * (hl - 1 - i) + b as i64 * i + (hl - 1) / 2) / (hl - 1)) as u8 };
            Color::rgb(ch(top.r, bottom.r), ch(top.g, bottom.g), ch(top.b, bottom.b))
        };
        let g = padded(32, 24);
        let mut fb = g.fb();
        shapes::gradient_v(&mut fb, x, y, w, h, top, bottom);
        step("5.7", g.compare(&format!("gradient_v({x}, {y}, {w}, {h})"),
            |px, py| if in_rect(px as i64, py as i64, xl, yl, w as i64, hl) { colour(py as i64) } else { PAPER }))?;
        let (got, want) = (dirty(&mut fb), on_surface(xl, yl, w as i64, hl));
        ensure(got == want, "6", format!("gradient_v({x}, {y}, {w}, {h}) marked {got:?}; the contract gives {want:?}"))?;
    }
    Ok(())
}

/// The damage a primitive must mark on a 32 × 24 surface with no clip (section 6).
fn damage_calls() -> Vec<(&'static str, Box<dyn Fn(&mut FrameBuf)>, Option<(i32, i32, i32, i32)>)> {
    vec![
        ("put_pixel(3, 4)", Box::new(|fb| fb.put_pixel(3, 4, INK)), Some((3, 4, 1, 1))),
        ("fill_rect(5, 3, 10, 7)", Box::new(|fb| shapes::fill_rect(fb, 5, 3, 10, 7, INK)), Some((5, 3, 10, 7))),
        ("fill_rect(-4, 2, 10, 5)", Box::new(|fb| shapes::fill_rect(fb, -4, 2, 10, 5, INK)), Some((0, 2, 6, 5))),
        ("fill_rect(28, 20, 10, 10)", Box::new(|fb| shapes::fill_rect(fb, 28, 20, 10, 10, INK)), Some((28, 20, 4, 4))),
        ("draw_rect(4, 3, 9, 6)", Box::new(|fb| shapes::draw_rect(fb, 4, 3, 9, 6, INK)), Some((4, 3, 9, 6))),
        ("draw_line(20, 2, 6, 9)", Box::new(|fb| shapes::draw_line(fb, 20, 2, 6, 9, INK)), Some((6, 2, 15, 8))),
        ("draw_line(-5, 3, 40, 3)", Box::new(|fb| shapes::draw_line(fb, -5, 3, 40, 3, INK)), Some((0, 3, 32, 1))),
        ("fill_circle(10, 10, 4)", Box::new(|fb| shapes::fill_circle(fb, 10, 10, 4, INK)), Some((6, 6, 9, 9))),
        ("fill_circle(30, 22, 5)", Box::new(|fb| shapes::fill_circle(fb, 30, 22, 5, INK)), Some((25, 17, 7, 7))),
        ("draw_circle(12, 11, 6)", Box::new(|fb| shapes::draw_circle(fb, 12, 11, 6, INK)), Some((6, 5, 13, 13))),
        ("draw_circle(12, 11, 0)", Box::new(|fb| shapes::draw_circle(fb, 12, 11, 0, INK)), Some((12, 11, 1, 1))),
        ("clear", Box::new(|fb| fb.clear(INK)), Some((0, 0, 32, 24))),
    ]
}

fn dirty(fb: &mut FrameBuf) -> Option<(i32, i32, i32, i32)> {
    fb.take_dirty().map(|d| (d.x, d.y, d.w, d.h))
}

fn raster_damage_exact() -> CaseResult {
    for (name, call, want) in damage_calls() {
        let g = padded(32, 24);
        let mut fb = g.fb();
        call(&mut fb);
        let got = dirty(&mut fb);
        if got != want {
            return at("6", format!("{name} marked {got:?}; the contract's table gives {want:?}"));
        }
        let again = dirty(&mut fb);
        if again.is_some() {
            return at("6", format!("take_dirty after {name} did not reset the damage: a second take gave {again:?}"));
        }
    }
    Ok(())
}

fn raster_damage_noop() -> CaseResult {
    let nothing: Vec<(&str, Box<dyn Fn(&mut FrameBuf)>)> = vec![
        ("fill_rect(40, 2, 5, 5) right of the surface", Box::new(|fb| shapes::fill_rect(fb, 40, 2, 5, 5, INK))),
        ("fill_rect(3, 30, 5, 5) below the surface", Box::new(|fb| shapes::fill_rect(fb, 3, 30, 5, 5, INK))),
        ("fill_rect(3, 3, 0, 5)", Box::new(|fb| shapes::fill_rect(fb, 3, 3, 0, 5, INK))),
        ("fill_circle(200, 10, 5) right of the surface", Box::new(|fb| shapes::fill_circle(fb, 200, 10, 5, INK))),
        ("draw_circle(10, 200, 5) below the surface", Box::new(|fb| shapes::draw_circle(fb, 10, 200, 5, INK))),
        ("draw_line(40, 30, 50, 40) beyond the surface", Box::new(|fb| shapes::draw_line(fb, 40, 30, 50, 40, INK))),
    ];
    for (name, call) in nothing {
        step("6", isolated(name, 2_000, || {
            let g = padded(32, 24);
            let mut fb = g.fb();
            call(&mut fb);
            match dirty(&mut fb) {
                None => Ok(()),
                Some(d) => Err(format!("{name} marked {d:?}; the contract marks nothing for a draw clipped to nothing")),
            }
        }))?;
    }
    let g = padded(32, 24);
    let mut fb = g.fb();
    shapes::fill_rect(&mut fb, 4, 4, 6, 3, PAPER);
    let got = dirty(&mut fb);
    ensure(got == Some((4, 4, 6, 3)), "6",
        format!("fill_rect of the colour already there marked {got:?}; damage means written, so (4, 4, 6, 3)"))
}

/// A seeded generator for scenes (xorshift64*).
fn raster_damage_sound() -> CaseResult {
    step("6", isolated("a seeded scene of 300 primitives", 8_000, || {
        let g = padded(64, 48);
        let mut fb = g.fb();
        let mut rng = Seeded(0x00B2_EE46_E15E_ED01);
        let mut changed = 0usize;
        for n in 0..300 {
            let before: Vec<Color> = (0..48).flat_map(|y| (0..64).map(move |x| (x, y))).map(|(x, y)| g.color(x, y)).collect();
            let c = Color::rgb(rng.range(0, 255) as u8, rng.range(0, 255) as u8, rng.range(0, 255) as u8);
            let (a, b, w, h) = (rng.range(-20, 80), rng.range(-20, 60), rng.range(-5, 40), rng.range(-5, 40));
            let name = match rng.range(0, 4) {
                0 => { shapes::fill_rect(&mut fb, a, b, w, h, c); format!("fill_rect({a}, {b}, {w}, {h})") }
                1 => { shapes::draw_rect(&mut fb, a, b, w, h, c); format!("draw_rect({a}, {b}, {w}, {h})") }
                2 => { shapes::draw_line(&mut fb, a, b, w * 2 - 10, h * 2 - 10, c); format!("draw_line({a}, {b}, {}, {})", w * 2 - 10, h * 2 - 10) }
                3 => { let r = w.rem_euclid(15); shapes::fill_circle(&mut fb, a.max(0), b, r, c); format!("fill_circle({}, {b}, {r})", a.max(0)) }
                _ => { let r = h.rem_euclid(15); shapes::draw_circle(&mut fb, a, b, r, c); format!("draw_circle({a}, {b}, {r})") }
            };
            let d = dirty(&mut fb);
            for y in 0..48 {
                for x in 0..64 {
                    if g.color(x, y) != before[y * 64 + x] {
                        changed += 1;
                        let inside = d.is_some_and(|(dx, dy, dw, dh)| in_rect(x as i64, y as i64, dx as i64, dy as i64, dw as i64, dh as i64));
                        if !inside {
                            return Err(format!("primitive {n}, {name}, changed pixel ({x},{y}) outside its damage {d:?}"));
                        }
                    }
                }
            }
        }
        if changed == 0 { return Err("the seeded scene changed no pixel".into()); }
        g.sentinels()
    }))
}

// gpu-raster: §3 owned surfaces are GPU targets; deferred unref must return exactly to the memory baseline.
fn raster_surface_owned() -> CaseResult {
use libbreenix::gpu;
    let before=crate::gpu_checks::counters().cite("3")?;
    let target=gpu::target_create(gpu::abi::format::B8G8R8A8,13,7)
        .map_err(|e|format!("gpu-raster: owned target {}",outcome(e))).cite("3")?;
    let during=crate::gpu_checks::counters().cite("3")?;
    let destroyed=gpu::resource_destroy(target);
    ensure(during.resources==before.resources+1 && during.resource_bytes>=before.resource_bytes+364,
        "3","gpu-raster: owned target did not charge its resource and backing")?;
    ensure(destroyed==0,"3","gpu-raster: owned target destruction failed")?;
    let start=now_ms();
    loop {
        let after=crate::gpu_checks::counters().cite("3")?;
        if after.resources==before.resources && after.resource_bytes==before.resource_bytes {break;}
        ensure(now_ms()-start<1_000,"3",format!("gpu-raster: dropping the target left {} resources and {} bytes; expected {} and {}",
            after.resources,after.resource_bytes,before.resources,before.resource_bytes))?;
        let _=time::sleep_ms(5);
    }Ok(())
}
const LINE_TABLE: &[((i32, i32, i32, i32), &[(i32, i32)])] = &[
    ((0, 0, 7, 3), &[(0, 0), (1, 0), (2, 1), (3, 1), (4, 2), (5, 2), (6, 3), (7, 3)]),
    ((7, 3, 0, 0), &[(7, 3), (6, 3), (5, 2), (4, 2), (3, 1), (2, 1), (1, 0), (0, 0)]),
    ((0, 0, 3, 7), &[(0, 0), (0, 1), (1, 2), (1, 3), (2, 4), (2, 5), (3, 6), (3, 7)]),
    ((3, 7, 0, 0), &[(3, 7), (3, 6), (2, 5), (2, 4), (1, 3), (1, 2), (0, 1), (0, 0)]),
    ((0, 0, 5, 5), &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]),
    ((0, 5, 5, 0), &[(0, 5), (1, 4), (2, 3), (3, 2), (4, 1), (5, 0)]),
    ((0, 0, 9, 2), &[(0, 0), (1, 0), (2, 0), (3, 1), (4, 1), (5, 1), (6, 1), (7, 2), (8, 2), (9, 2)]),
    ((2, 9, 0, 0), &[(2, 9), (2, 8), (2, 7), (1, 6), (1, 5), (1, 4), (1, 3), (0, 2), (0, 1), (0, 0)]),
];
const CIRCLE_OCTANTS: [&[i32]; 41] = [
    &[0],
    &[1, 1],
    &[2, 2],
    &[3, 3, 2],
    &[4, 4, 4, 3],
    &[5, 5, 5, 4],
    &[6, 6, 6, 5, 5],
    &[7, 7, 7, 6, 6, 5],
    &[8, 8, 8, 7, 7, 6],
    &[9, 9, 9, 9, 8, 8, 7],
    &[10, 10, 10, 10, 9, 9, 8, 7],
    &[11, 11, 11, 11, 10, 10, 9, 9, 8],
    &[12, 12, 12, 12, 11, 11, 10, 10, 9],
    &[13, 13, 13, 13, 12, 12, 12, 11, 10, 9],
    &[14, 14, 14, 14, 13, 13, 13, 12, 12, 11, 10],
    &[15, 15, 15, 15, 14, 14, 14, 13, 13, 12, 11],
    &[16, 16, 16, 16, 16, 15, 15, 14, 14, 13, 13, 12],
    &[17, 17, 17, 17, 17, 16, 16, 16, 15, 14, 14, 13, 12],
    &[18, 18, 18, 18, 18, 17, 17, 17, 16, 16, 15, 14, 13],
    &[19, 19, 19, 19, 19, 18, 18, 18, 17, 17, 16, 16, 15, 14],
    &[20, 20, 20, 20, 20, 19, 19, 19, 18, 18, 17, 17, 16, 15, 14],
    &[21, 21, 21, 21, 21, 20, 20, 20, 19, 19, 18, 18, 17, 17, 16, 15],
    &[22, 22, 22, 22, 22, 21, 21, 21, 21, 20, 20, 19, 18, 18, 17, 16],
    &[23, 23, 23, 23, 23, 22, 22, 22, 22, 21, 21, 20, 20, 19, 18, 17, 17],
    &[24, 24, 24, 24, 24, 23, 23, 23, 23, 22, 22, 21, 21, 20, 20, 19, 18, 17],
    &[25, 25, 25, 25, 25, 25, 24, 24, 24, 23, 23, 22, 22, 21, 21, 20, 19, 18],
    &[26, 26, 26, 26, 26, 26, 25, 25, 25, 24, 24, 24, 23, 23, 22, 21, 21, 20, 19],
    &[27, 27, 27, 27, 27, 27, 26, 26, 26, 25, 25, 25, 24, 24, 23, 22, 22, 21, 20, 19],
    &[28, 28, 28, 28, 28, 28, 27, 27, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21, 21, 20],
    &[29, 29, 29, 29, 29, 29, 28, 28, 28, 28, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21],
    &[30, 30, 30, 30, 30, 30, 29, 29, 29, 29, 28, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22, 21],
    &[31, 31, 31, 31, 31, 31, 30, 30, 30, 30, 29, 29, 29, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22],
    &[32, 32, 32, 32, 32, 32, 31, 31, 31, 31, 30, 30, 30, 29, 29, 28, 28, 27, 26, 26, 25, 24, 23],
    &[33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 31, 31, 31, 30, 30, 29, 29, 28, 28, 27, 26, 25, 25, 24],
    &[34, 34, 34, 34, 34, 34, 33, 33, 33, 33, 33, 32, 32, 31, 31, 31, 30, 29, 29, 28, 28, 27, 26, 25, 24],
    &[35, 35, 35, 35, 35, 35, 34, 34, 34, 34, 34, 33, 33, 33, 32, 32, 31, 31, 30, 29, 29, 28, 27, 26, 25, 25],
    &[36, 36, 36, 36, 36, 36, 36, 35, 35, 35, 35, 34, 34, 34, 33, 33, 32, 32, 31, 31, 30, 29, 29, 28, 27, 26],
    &[37, 37, 37, 37, 37, 37, 37, 36, 36, 36, 36, 35, 35, 35, 34, 34, 33, 33, 32, 32, 31, 30, 30, 29, 28, 27, 26],
    &[38, 38, 38, 38, 38, 38, 38, 37, 37, 37, 37, 36, 36, 36, 35, 35, 34, 34, 33, 33, 32, 32, 31, 30, 29, 29, 28, 27],
    &[39, 39, 39, 39, 39, 39, 39, 38, 38, 38, 38, 37, 37, 37, 36, 36, 36, 35, 35, 34, 33, 33, 32, 32, 31, 30, 29, 28],
    &[40, 40, 40, 40, 40, 40, 40, 39, 39, 39, 39, 38, 38, 38, 37, 37, 37, 36, 36, 35, 35, 34, 33, 33, 32, 31, 30, 30, 29],
];

// ---------------------------------------------------------------------------------------
// text: TrueType & bitmap text (contract section 8)
// ---------------------------------------------------------------------------------------

const MONO_PATH: &str = "/usr/share/fonts/DejaVuSansMono.ttf";
const SANS_PATH: &str = "/usr/share/fonts/DejaVuSans.ttf";
const MONO_SHA256: &str = "b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582";
const SANS_SHA256: &str = "7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954";

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn parse_font(path: &str) -> Result<Font, String> {
    Font::parse(&read_file(path)?).map_err(|e| format!("{path} does not parse: {e}"))
}

fn cached(path: &str) -> Result<CachedFont, String> {
    Ok(CachedFont::new(parse_font(path)?, 256))
}

fn hex_digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Section 7's raster bound, `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64`, worked
/// here from its formula.
fn raster_steps_bound(e: u64, h: u64, c: u64) -> u64 {
    let mut log = 0u64;
    while (1u64 << log) < e + 1 {
        log += 1;
    }
    e + h * (10 * c + 8 * e * (3 + log) + 17) + 64
}

/// The bytes and points of `glyph` by an independent reading of its records, for section 7's
/// outline bounds: the bytes of every record its expansion reaches, each ending at the glyf
/// table's end as section 10 scopes a record that runs past it, and the points the simple
/// glyphs among them declare. A record that cannot be read further adds its bytes and
/// nothing more, so a malformed glyph is held to what its own records hold. Expansion stops
/// at depth 8 and after 256 components, as libfont's must.
fn independent_glyph(file: &[u8], glyph: u16) -> Result<(u64, u64), String> {
    let (head_at, _) = ttf_table(file, b"head").ok_or("no head table")?;
    let (loca_at, _) = ttf_table(file, b"loca").ok_or("no loca table")?;
    let (glyf_at, glyf_len) = ttf_table(file, b"glyf").ok_or("no glyf table")?;
    let be16 = |o: usize| file.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let long = be16(head_at + 50) == Some(1);
    let entry = |i: usize| -> Option<usize> {
        if long {
            file.get(loca_at + 4 * i..loca_at + 4 * i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        } else {
            be16(loca_at + 2 * i).map(|v| 2 * v as usize)
        }
    };
    let mut components = 0usize;
    let mut stack = vec![(glyph, 0u32)];
    let (mut bytes, mut points) = (0u64, 0u64);
    while let Some((g, depth)) = stack.pop() {
        let (Some(start), Some(end)) = (entry(g as usize), entry(g as usize + 1)) else { continue };
        let end = end.min(glyf_len);
        if start >= end || depth > 8 {
            continue;
        }
        let len = end - start;
        bytes += len as u64;
        // A two-byte read inside this record only.
        let rd = |o: usize| if o + 2 <= len { be16(glyf_at + start + o) } else { None };
        let Some(contours) = rd(0).map(|v| v as i16) else { continue };
        if contours >= 0 {
            if contours > 0 {
                points += rd(10 + 2 * (contours as usize - 1)).map_or(0, |e| e as u64 + 1);
            }
            continue;
        }
        let mut p = 10;
        while let (Some(flags), Some(child)) = (rd(p), rd(p + 2)) {
            components += 1;
            if components > 256 {
                break;
            }
            stack.push((child, depth + 1));
            p += 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
            p += if flags & 0x0008 != 0 { 2 } else if flags & 0x0040 != 0 { 4 } else if flags & 0x0080 != 0 { 8 } else { 0 };
            if flags & 0x0020 == 0 {
                break;
            }
        }
    }
    Ok((bytes, points))
}

/// Each stage of rasterizing `glyph` at `size` against section 7's bounds, by
/// libfont::work: the outline within `16 × glyph bytes + 8 × P + 4,096`, the flattening within
/// `1,028 × P`, and the grayscale and LCD rasterizations within the raster bound. The
/// stages together give exactly `rasterize_glyph`'s bitmap.
fn staged_within_bounds(font: &Font, file: &[u8], glyph: u16, size: f32, what: &str) -> Result<(), String> {
    use libfont::{work as fwork, GlyphLayout};
    let (bytes, points) = independent_glyph(file, glyph)?;
    let size26 = Font::size26(size).map_err(|e| format!("{what}: size {size} refused: {e}"))?;
    fwork::reset();
    let outline = font.glyph_outline(glyph).map_err(|e| format!("{what}: no outline: {e}"))?;
    let steps = fwork::steps();
    if steps > 16 * bytes + 8 * points + 4_096 {
        return Err(format!("{what}: its outline took {steps} steps; section 7 allows 16 x {bytes} + 8 x {points} + 4096"));
    }
    fwork::reset();
    let layout = font.layout_glyph(&outline, glyph, size26).map_err(|e| format!("{what}: no layout: {e}"))?;
    let steps = fwork::steps();
    if steps > 1_028 * points {
        return Err(format!("{what}: flattening its {points} points took {steps} steps; section 7 allows {}", 1_028 * points));
    }
    let GlyphLayout::Outline { segments, width, height, x_offset, y_offset } = layout else { return Ok(()) };
    let e = segments.len() as u64;
    fwork::reset();
    let gray = libfont::rasterizer::rasterize(&segments, width, height, x_offset, y_offset).map_err(|e| format!("{what}: {e}"))?;
    let steps = fwork::steps();
    let bound = raster_steps_bound(e, height as u64, width as u64);
    if steps > bound {
        return Err(format!("{what}: rasterizing {e} segments into {width} x {height} took {steps} steps; section 7 allows {bound}"));
    }
    if steps < 8 * height as u64 {
        return Err(format!("{what}: rasterizing {height} rows counted only {steps} steps; each row samples 8 sub-scanlines"));
    }
    let whole = font.rasterize_glyph(glyph, size).map_err(|e| format!("{what}: {e}"))?;
    if whole != gray {
        return Err(format!("{what}: rasterize_glyph differs from its outline, layout and raster stages run one by one"));
    }
    fwork::reset();
    libfont::rasterizer::rasterize_subpixel(&segments, width, height, x_offset, y_offset).map_err(|e| format!("{what}: LCD: {e}"))?;
    let steps = fwork::steps();
    let bound = e + raster_steps_bound(e, height as u64, 3 * width as u64);
    if steps > bound {
        return Err(format!("{what}: the LCD rasterization took {steps} steps; section 7 allows {bound} for 3 x {width} sampled columns"));
    }
    Ok(())
}

/// Draw `text` with `draw` on a black 96 × 48 surface in white, so the coverage blend is
/// the coverage itself under any rounding.
fn white_on_black(width: usize, height: usize, draw: impl FnOnce(&mut FrameBuf)) -> Guarded {
    let g = Guarded::bgrx(width, height);
    draw(&mut g.fb());
    g
}

/// The pixels a glyph bitmap gives in white on black with its origin at (x, y).
fn glyph_pixels(bitmap: &libfont::Bitmap, x: i32, y: i32, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for row in 0..bitmap.height {
        for col in 0..bitmap.width {
            let (px, py) = (x + bitmap.x_offset + col as i32, y + bitmap.y_offset + row as i32);
            if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                out[py as usize * width + px as usize] = bitmap.coverage[row * bitmap.width + col];
            }
        }
    }
    out
}

fn same_as_glyph(g: &Guarded, want: &[u8], what: &str, section: &str) -> CaseResult {
    if !want.iter().any(|&c| c > 0) {
        return at(section, format!("{what}: the glyph it is compared with has no coverage on the surface"));
    }
    for y in 0..g.height {
        for x in 0..g.width {
            let [r, gr, b] = g.rgb(x, y);
            let w = want[y * g.width + x];
            if [r, gr, b] != [w, w, w] {
                return at(section, format!("{what}: pixel ({x},{y}) is #{r:02x}{gr:02x}{b:02x}, the glyph gives coverage {w}"));
            }
        }
    }
    Ok(())
}

fn text_fonts_pinned() -> CaseResult {
    // FIPS 180-4's known answer for "abc" shows the hasher itself is right first.
    ensure(hex_digest(b"abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "8.5",
        "SHA-256 of \"abc\" differs from FIPS 180-4's known answer")?;
    for (path, pin) in [(MONO_PATH, MONO_SHA256), (SANS_PATH, SANS_SHA256)] {
        let bytes = read_file(path).cite("8.5")?;
        let got = hex_digest(&bytes);
        ensure(got == pin, "8.5", format!("{path} has SHA-256 {got}; the pin is {pin}"))?;
        Font::parse(&bytes).map_err(|e| format!("{path} does not parse: {e}")).cite("8.5")?;
    }
    Ok(())
}

fn text_raster_identical() -> CaseResult {
    // The glyph set and serialization tools/gfx-refgen hashed (PROVENANCE.json).
    let mut bytes = Vec::new();
    let fonts = [parse_font(MONO_PATH).cite("8.5")?, parse_font(SANS_PATH).cite("8.5")?];
    let mut put = |w: usize, h: usize, x: i32, y: i32, c: &[u8]| {
        bytes.extend_from_slice(&(w as u32).to_le_bytes());
        bytes.extend_from_slice(&(h as u32).to_le_bytes());
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(c);
    };
    for f in &fonts {
        for size in [7.0f32, 12.0, 16.0, 23.5, 40.0] {
            for cp in 0x21u32..=0x7E {
                let b = f.rasterize_glyph(f.glyph_for(cp), size).map_err(|e| format!("U+{cp:04X} at {size} px: {e}")).cite("8.3")?;
                put(b.width, b.height, b.x_offset, b.y_offset, &b.coverage);
            }
        }
    }
    for f in &fonts {
        for cp in 0x21u32..=0x7E {
            let b = f.rasterize_glyph_subpixel(f.glyph_for(cp), 16.0).map_err(|e| format!("U+{cp:04X} LCD: {e}")).cite("8.3")?;
            put(b.width, b.height, b.x_offset, b.y_offset, &b.coverage);
        }
    }
    let got = hex_digest(&bytes);
    let path = format!("{FRAMES}/text/raster-identical.sha256");
    let pinned = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}")).cite("8.6")?;
    ensure(got == pinned.trim(), "8.6", format!("the glyph set hashes to {got} on this target; the reviewed value is {}", pinned.trim()))
}

fn text_metrics_independent() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    for &(cp, gid, adv, lsb, x_min, y_min, x_max, y_max) in MONO_METRICS.iter() {
        let ch = char::from_u32(cp).unwrap();
        ensure(font.glyph_index(ch) == gid, "8.5", format!("U+{cp:04X} maps to glyph {}, the table says {gid}", font.glyph_index(ch)))?;
        let m = font.glyph_metrics(gid).map_err(|e| format!("glyph {gid} has no metrics: {e}")).cite("8.3")?;
        let got = (m.advance, m.lsb, m.x_min, m.y_min, m.x_max, m.y_max);
        let want = (adv, lsb as i16, x_min, y_min, x_max, y_max);
        ensure(got == want, "8.3, 8.5", format!("glyph {gid} (U+{cp:04X}): advance, lsb and box are {got:?}; the independent table gives {want:?}"))?;
    }
    Ok(())
}

fn text_cmap_independent() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    for &(cp, gid) in MONO_CMAP.iter() {
        let ch = char::from_u32(cp).unwrap();
        let got = font.glyph_index(ch);
        ensure(got == gid, "8.5", format!("U+{cp:04X} maps to glyph {got}; the independent table says {gid}"))?;
    }
    Ok(())
}

/// The pixels of a 5×7 glyph from its rows (section 8.1), drawn at (x, y) at `scale`.
fn glyph5x7(rows: [u8; 7], x: i64, y: i64, scale: i64) -> impl Fn(i64, i64) -> bool {
    move |px, py| {
        if px < x || py < y || px >= x + 5 * scale || py >= y + 7 * scale { return false; }
        let (col, row) = ((px - x) / scale, (py - y) / scale);
        rows[row as usize] & (0x10 >> col) != 0
    }
}

const GLYPH_A: [u8; 7] = [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11];
const GLYPH_BOX: [u8; 7] = [0x1F; 7];

fn text_bitmap_glyph() -> CaseResult {
    exact("8.1", "font::draw_char('A', 2, 3, scale 1)", 12, 12,
        |fb| font5x7::draw_char(fb, b'A', 2, 3, INK, 1), glyph5x7(GLYPH_A, 2, 3, 1))
}

fn text_bitmap_scale() -> CaseResult {
    exact("8.1", "font::draw_char('A', 1, 1, scale 3)", 20, 24,
        |fb| font5x7::draw_char(fb, b'A', 1, 1, INK, 3), glyph5x7(GLYPH_A, 1, 1, 3))
}

fn text_bitmap_unknown() -> CaseResult {
    for byte in [0x7Fu8, 0xC8, 0x01] {
        exact("8.1", &format!("font::draw_char({byte:#04x})"), 10, 10,
            |fb| font5x7::draw_char(fb, byte, 2, 1, INK, 1), glyph5x7(GLYPH_BOX, 2, 1, 1))?;
    }
    let w = font5x7::text_width(b"A\x7f", 2);
    ensure(w == 22, "8.1", format!("text_width of two glyphs at scale 2 is {w}, the contract gives (2 - 1) x 12 + 10 = 22"))
}

fn text_noto_ascii() -> CaseResult {
    use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
    for ch in (0x20u8..0x7F).map(char::from) {
        let raster = get_raster(ch, FontWeight::Regular, RasterHeight::Size16)
            .ok_or_else(|| format!("the crate has no raster for {ch:?}")).cite("8.2")?;
        let g = white_on_black(16, 20, |fb| bitmap_font::draw_char(fb, ch, 0, 2, WHITE));
        for y in 0..20 {
            for x in 0..16 {
                let want = if y >= 2 && y - 2 < raster.raster().len() && x < raster.width() { raster.raster()[y - 2][x] } else { 0 };
                let [r, gr, b] = g.rgb(x, y);
                if [r, gr, b] != [want; 3] {
                    return at("8.2", format!("{ch:?}: pixel ({x},{y}) is #{r:02x}{gr:02x}{b:02x}, the reference cell has coverage {want}"));
                }
            }
        }
    }
    Ok(())
}

/// The coverage blend (section 2), computed here from its formula.
fn div255(x: u32) -> u8 {
    ((x + 128 + ((x + 128) >> 8)) >> 8) as u8
}

fn text_noto_coverage() -> CaseResult {
    use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
    let (fg, bg) = (Color::rgb(200, 100, 50), Color::rgb(20, 40, 60));
    for ch in ['@', 'g', 'W', '%', '&'] {
        let raster = get_raster(ch, FontWeight::Regular, RasterHeight::Size16).unwrap();
        let mut g = Guarded::bgrx(16, 16);
        g.reset(bg);
        bitmap_font::draw_char(&mut g.fb(), ch, 0, 0, fg);
        for (y, row) in raster.raster().iter().enumerate().take(16) {
            for (x, &a) in row.iter().enumerate().take(raster.width().min(16)) {
                let a = a as u32;
                let want = Color::rgb(div255(fg.r as u32 * a + bg.r as u32 * (255 - a)),
                    div255(fg.g as u32 * a + bg.g as u32 * (255 - a)), div255(fg.b as u32 * a + bg.b as u32 * (255 - a)));
                if g.color(x, y) != want {
                    return at("2, 8.2", format!("{ch:?} at ({x},{y}), coverage {a}: blended to {} where the contract gives {}",
                        hex(g.color(x, y)), hex(want)));
                }
            }
        }
    }
    Ok(())
}

fn text_ttf_glyph() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let g = white_on_black(48, 32, |fb| { ttf_font::draw_text(fb, &mut font, "Hg", 8, 6, 16.0, WHITE); });
    matches_reference(&g, "text/ttf-glyph.png", "8.6")
}

fn text_ttf_sizes() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let g = white_on_black(256, 160, |fb| {
        for (size, y) in [(8.0, 2), (12.0, 14), (16.0, 30), (24.0, 52), (48.0, 86)] {
            ttf_font::draw_text(fb, &mut font, "Breenix", 4, y, size, WHITE);
        }
    });
    matches_reference(&g, "text/ttf-sizes.png", "8.6")
}

fn text_ttf_advance() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let line: String = "The quick brown fox jumps over the lazy dog 0123456789 ".chars().cycle().take(200).collect();
    let g = Guarded::bgrx(64, 24);
    let drawn = ttf_font::draw_text(&mut g.fb(), &mut font, &line, 0, 0, 16.0, WHITE);
    let width = ttf_font::text_width(&mut font, &line, 16.0);
    value("advance", drawn as i64, "px", Some((1928, 1928)));
    ensure(drawn == 1928, "8.3, 8.5", format!(
        "a 200-character DejaVu Sans Mono line at 16 px ends at {drawn} px; 200 advances of 617/64 px, rounded once, end at 1928"))?;
    ensure(width == 1928, "8.3", format!("text_width of the line is {width}; the contract's line ends at 1928"))
}

fn text_ttf_kerning() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let (av, a, v) = (ttf_font::text_width(&mut font, "AV", 32.0), ttf_font::text_width(&mut font, "A", 32.0),
        ttf_font::text_width(&mut font, "V", 32.0));
    ensure((av, a, v) == (42, 22, 22), "8.3, 8.5", format!(
        "at 32 px \"AV\", \"A\" and \"V\" measure {av}, {a} and {v} px; the kern pair -131 gives 42, 22 and 22"))?;
    // Literal kern tables, each subtable holding the pair (1, 2): every horizontal format 0
    // subtable applies, adding its value, or replacing the sum so far with the override bit.
    let kerned = |table: Vec<u8>| {
        let mut t = Ttf::base();
        t.kern = Some(table);
        t.build()
    };
    for (what, subtables, want) in [
        ("two additive subtables of -40 and -10", vec![(0x0001u16, -40i16), (0x0001, -10)], -50),
        ("-40, an override subtable of -10, then -5", vec![(0x0001, -40), (0x0009, -10), (0x0001, -5)], -15),
    ] {
        let font = Font::parse(&kerned(kern_table(&subtables))).map_err(|e| format!("a font with {what} was rejected: {e}")).cite("8.3")?;
        let got = font.kern_units(1, 2);
        ensure(got == want, "8.3", format!("the pair (1, 2) under {what} kerns {got} units; the kern table gives {want}"))?;
    }
    // The first subtable's length (14) stops before its pair, which the next subtable's
    // bytes would supply: a read past the subtable.
    let mut short = kern_table(&[(0x0001, -40), (0x0001, -10)]);
    short[6..8].copy_from_slice(&14u16.to_be_bytes());
    rejected("10", "a kern subtable whose pair runs past its length", kerned(short))
}

/// A version 0 kern table of format 0 subtables `(coverage, value)`, each holding the
/// single pair (1, 2).
fn kern_table(subtables: &[(u16, i16)]) -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 0);
    be16(&mut v, subtables.len() as u16);
    for &(coverage, value) in subtables {
        for x in [0u16, 20, coverage, 1, 6, 0, 0, 1, 2, value as u16] {
            be16(&mut v, x);
        }
    }
    v
}

fn text_ttf_width() -> CaseResult {
    for (path, text, size, want) in [(MONO_PATH, "Hg", 16.0f32, 19), (SANS_PATH, "AV", 32.0, 42), (MONO_PATH, "", 16.0, 0)] {
        let mut font = cached(path).cite("8.5")?;
        let g = Guarded::bgrx(64, 48);
        let drawn = ttf_font::draw_text(&mut g.fb(), &mut font, text, 0, 0, size, WHITE);
        let width = ttf_font::text_width(&mut font, text, size);
        ensure(drawn == want && width == want, "8.3", format!(
            "{text:?} at {size} px: draw_text returned {drawn} and text_width {width}; the contract's pen ends at {want} px"))?;
    }
    Ok(())
}

/// Text drawn at (x0, y0) and at (x0 + dx, y0 + dy) is the same pixels translated.
fn translated(dx: i32, dy: i32, section: &str) -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let (w, h) = (64usize, 40usize);
    let base = white_on_black(w, h, |fb| { ttf_font::draw_text(fb, &mut font, "Hg", 20, 12, 16.0, WHITE); });
    ensure(drawn_pixels(&base, Color::rgb(0, 0, 0)) > 0, "8.3", "\"Hg\" drawn at (20, 12) left the surface black")?;
    let moved = white_on_black(w, h, |fb| { ttf_font::draw_text(fb, &mut font, "Hg", 20 + dx, 12 + dy, 16.0, WHITE); });
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (sx, sy) = (x - dx, y - dy);
            let want = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { base.rgb(sx as usize, sy as usize) } else { [0; 3] };
            if moved.rgb(x as usize, y as usize) != want {
                return at(section, format!("\"Hg\" moved by ({dx},{dy}): pixel ({x},{y}) is {:?}, the translated text gives {want:?}",
                    moved.rgb(x as usize, y as usize)));
            }
        }
    }
    Ok(())
}

fn text_ttf_clip_left() -> CaseResult {
    translated(-25, 0, "8.3")?;
    wholly_outside(&[(i32::MAX, 10), (i32::MIN, 10), (-20_000, 10), (64, 10)])
}

fn text_ttf_clip_top() -> CaseResult {
    translated(0, -18, "8.3")?;
    wholly_outside(&[(10, i32::MIN), (10, -1_000), (10, 1_000), (10, i32::MAX)])
}

/// 1,000 characters started at each position lie wholly outside a 64 × 32 surface: they are
/// laid out but no glyph is rasterized, and no pixel changes (section 8.3).
fn wholly_outside(starts: &[(i32, i32)]) -> CaseResult {
    let line = "W".repeat(1_000);
    for &(x, y) in starts {
        let mut font = CachedFont::with_default_cache(parse_font(MONO_PATH).cite("8.5")?);
        let g = white_on_black(64, 32, |fb| { ttf_font::draw_text(fb, &mut font, &line, x, y, 16.0, WHITE); });
        let misses = font.cache_stats().misses;
        ensure(misses == 0, "8.3", format!(
            "1,000 characters at ({x}, {y}), wholly outside a 64 x 32 surface, rasterized {misses} glyphs; the contract rasterizes none"))?;
        ensure(drawn_pixels(&g, Color::rgb(0, 0, 0)) == 0, "8.3", format!("1,000 characters at ({x}, {y}) changed pixels of a 64 x 32 surface"))?;
    }
    Ok(())
}

fn text_ttf_subpixel() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let g = white_on_black(160, 28, |fb| { ttf_font::draw_text_subpixel(fb, &mut font, "Subpixel LCD", 4, 4, 16.0, WHITE); });
    matches_reference(&g, "text/ttf-subpixel.png", "8.6")
}

fn text_ttf_notdef() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let ch = '\u{E000}';
    ensure(font.glyph_index(ch) == 0, "8.5", "U+E000 should be unmapped in DejaVu Sans Mono")?;
    let notdef = font.font().rasterize_glyph(0, 24.0).map_err(|e| format!("glyph 0 does not rasterize: {e}")).cite("8.3")?;
    let g = white_on_black(48, 40, |fb| { ttf_font::draw_char(fb, &mut font, ch, 10, 4, 24.0, WHITE); });
    let want = glyph_pixels(&notdef, 10, 4, 48, 40);
    if same_as_glyph(&g, &want, "", "8.3").is_err() {
        let question = font.glyph_index('?');
        let q = font.font().rasterize_glyph(question, 24.0).map_err(|e| e.to_string())?;
        let substitute = if same_as_glyph(&g, &glyph_pixels(&q, 10, 4, 48, 40), "", "8.3").is_ok() { " (it drew '?' instead)" } else { "" };
        return at("8.3", format!("U+E000 did not draw glyph 0 (.notdef){substitute}"));
    }
    Ok(())
}

fn text_utf8_text() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    ensure(font.glyph_index('é') == 171, "8.5", format!("U+00E9 maps to glyph {}, the table says 171", font.glyph_index('é')))?;
    let bitmap = font.font().rasterize_glyph(171, 24.0).map_err(|e| format!("glyph 171 does not rasterize: {e}")).cite("8.3")?;
    let g = white_on_black(48, 40, |fb| { ttf_font::draw_text(fb, &mut font, "é", 10, 4, 24.0, WHITE); });
    same_as_glyph(&g, &glyph_pixels(&bitmap, 10, 4, 48, 40), "\"é\" drawn from UTF-8", "8.3")
}

fn text_utf8_invalid() -> CaseResult {
    const R: u32 = 0xFFFD;
    // The Unicode Standard's own example (section 3.9, Table 3-8), then literal edge cases.
    let table: [(&[u8], &[u32]); 9] = [
        (&[0x61, 0xF1, 0x80, 0x80, 0xE1, 0x80, 0xC2, 0x62, 0x80, 0x63, 0x80, 0xBF, 0x64],
            &[0x61, R, R, R, 0x62, R, 0x63, R, R, 0x64]),
        (&[0xC0, 0x80], &[R, R]),
        (&[0xE0, 0x80, 0x80], &[R, R, R]),
        (&[0xED, 0xA0, 0x80], &[R, R, R]),
        (&[0xF4, 0x90, 0x80, 0x80], &[R, R, R, R]),
        (&[0x41, 0xF0, 0x9F, 0x98], &[0x41, R]),
        (&[0xE2, 0x82], &[R]),
        (&[0xF0, 0x9F, 0x98, 0x80, 0xFF, 0x7F], &[0x1F600, R, 0x7F]),
        (&[0xF5, 0xC2, 0xA9, 0xEF, 0xBF, 0xBD], &[R, 0xA9, R]),
    ];
    for (bytes, want) in table {
        let got: Vec<u32> = ttf_font::decode_utf8(bytes).collect();
        ensure(got == want, "8.3", format!("UTF-8 {bytes:02x?} decoded to {got:x?}; one U+FFFD per maximal invalid subpart gives {want:x?}"))?;
    }
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let (bytes, want) = table[0];
    let text: String = want.iter().map(|&c| char::from_u32(c).unwrap()).collect();
    let direct = white_on_black(160, 24, |fb| { ttf_font::draw_text(fb, &mut font, &text, 2, 2, 14.0, WHITE); });
    ensure(drawn_pixels(&direct, Color::rgb(0, 0, 0)) > 0, "8.3", "the decoded text drew nothing")?;
    let mut ret = 0;
    let bytes_drawn = white_on_black(160, 24, |fb| { ret = ttf_font::draw_text_bytes(fb, &mut font, bytes, 2, 2, 14.0, WHITE); });
    let width = ttf_font::text_width(&mut font, &text, 14.0);
    *bytes_drawn.reference.borrow_mut() = Some(canonical_pixels(&direct));
    bytes_drawn.judge();
    ensure(ret == width, "8.3", format!("draw_text_bytes returned {ret}; the decoded text is {width} px wide"))?;
    for y in 0..24 {
        for x in 0..160 {
            if bytes_drawn.rgb(x, y) != direct.rgb(x, y) {
                return at("8.3", format!("draw_text_bytes of {bytes:02x?} differs at ({x},{y}) from drawing {want:x?}"));
            }
        }
    }
    step("8.3", isolated("draw_text_bytes over every byte value and 1,000 seeded byte strings", 8_000, move || {
        let g = Guarded::bgrx(64, 24);
        let mut rng = Seeded(0x0BAD_0B17E5);
        for b in 0..=255u8 {
            ttf_font::draw_text_bytes(&mut g.fb(), &mut font, &[b, b, 0x80, b], 0, 0, 10.0, WHITE);
        }
        for _ in 0..1_000 {
            let s: Vec<u8> = (0..rng.range(1, 12)).map(|_| rng.range(0x70, 0xFF) as u8).collect();
            ttf_font::draw_text_bytes(&mut g.fb(), &mut font, &s, 0, 0, 10.0, WHITE);
        }
        g.sentinels()
    }))
}

/// A table's (offset, length) from a TrueType file's directory, read here independently.
fn ttf_table(file: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let be16 = |o: usize| u16::from_be_bytes([file[o], file[o + 1]]) as usize;
    let be32 = |o: usize| u32::from_be_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]) as usize;
    (0..be16(4)).map(|i| 12 + 16 * i).find(|&r| &file[r..r + 4] == tag).map(|r| (be32(r + 8), be32(r + 12)))
}

/// A simple glyph's points in order (TrueType glyf format), decoded here independently.
fn simple_points(glyf: &[u8], off: usize) -> Vec<(i32, i32)> {
    let be16 = |o: usize| u16::from_be_bytes([glyf[o], glyf[o + 1]]);
    let contours = be16(off) as i16 as usize;
    let mut p = off + 10;
    let last = (0..contours).map(|i| be16(p + 2 * i)).last().unwrap_or(0) as usize;
    let points = if contours == 0 { 0 } else { last + 1 };
    p += 2 * contours;
    p += 2 + be16(p) as usize;
    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let f = glyf[p];
        p += 1;
        flags.push(f);
        if f & 0x08 != 0 {
            for _ in 0..glyf[p] { flags.push(f); }
            p += 1;
        }
    }
    let mut coords = |short: u8, same: u8| {
        let mut v = 0i32;
        let mut out = Vec::with_capacity(points);
        for &f in &flags {
            if f & short != 0 {
                let d = glyf[p] as i32;
                p += 1;
                v += if f & same != 0 { d } else { -d };
            } else if f & same == 0 {
                v += be16(p) as i16 as i32;
                p += 2;
            }
            out.push(v);
        }
        out
    };
    let xs = coords(0x02, 0x10);
    let ys = coords(0x04, 0x20);
    xs.into_iter().zip(ys).collect()
}

fn text_compound_glyph() -> CaseResult {
    let file = read_file(MONO_PATH).cite("8.5")?;
    let (glyf_at, glyf_len) = ttf_table(&file, b"glyf").ok_or_else(|| "no glyf table".to_string()).cite("8.5")?;
    let (loca_at, _) = ttf_table(&file, b"loca").ok_or_else(|| "no loca table".to_string()).cite("8.5")?;
    let glyf = &file[glyf_at..glyf_at + glyf_len];
    // DejaVu Sans Mono's loca is long (indexToLocFormat 1).
    let loca = |g: usize| u32::from_be_bytes(file[loca_at + 4 * g..loca_at + 4 * g + 4].try_into().unwrap());
    let font = Font::parse(&file).cite("8.5")?;
    let outline = font.glyph_outline(171).map_err(|e| format!("glyph 171 (é) does not resolve: {e}")).cite("10")?;
    let got: Vec<(i32, i32)> = outline.points.iter().map(|p| (p.x as i32, p.y as i32)).collect();
    let mut want = simple_points(glyf, loca(72) as usize);
    want.extend(simple_points(glyf, loca(118) as usize).into_iter().map(|(x, y)| (x + 14, y)));
    ensure(want.len() > 8, "8.5", format!("the independent decoder found only {} points in e and acute", want.len()))?;
    ensure(got == want, "8.5", format!(
        "glyph 171 resolves to {} points; e (glyph 72) at (0, 0) and acute (glyph 118) at (14, 0) give {}{}",
        got.len(), want.len(),
        got.iter().zip(&want).position(|(a, b)| a != b).map(|i| format!(", first differing at point {i}: {:?} vs {:?}", got[i], want[i])).unwrap_or_default()))?;
    // The square (glyph 1) as a component at scale 0.5 with offset (100, 40). The offset is in
    // the compound's units, unless SCALED_COMPONENT_OFFSET (0x0800) alone puts it in the
    // component's, where the scale applies to it too; UNSCALED_COMPONENT_OFFSET (0x1000), or
    // both flags, keep the default.
    for (flags, (dx, dy)) in [(0u16, (100, 40)), (0x0800, (50, 20)), (0x1000, (100, 40)), (0x1800, (100, 40))] {
        let mut g = Vec::new();
        be16(&mut g, 0xFFFF);
        for b in [0i16, 0, 1000, 1000] { be16(&mut g, b as u16); }
        for x in [0x0001 | 0x0002 | 0x0008 | flags, 1, 100, 40, 0x2000] { be16(&mut g, x); }
        let font = Font::parse(&with_glyph2(g)).cite("10")?;
        let got: Vec<(i32, i32)> = font.glyph_outline(2).cite("10")?.points.iter().map(|p| (p.x as i32, p.y as i32)).collect();
        let want: Vec<(i32, i32)> = SQUARE.iter().map(|&(x, y)| (x as i32 / 2 + dx, y as i32 / 2 + dy)).collect();
        ensure(got == want, "10", format!("the square at scale 0.5, offset (100, 40), flags {flags:#06x} resolves to {got:?}; the component offset rule gives {want:?}"))?;
    }
    Ok(())
}

fn text_compound_transform() -> CaseResult {
    let mut font = cached("/usr/local/test/share/breengel/fonts/compound-transform.ttf").cite("8.6")?;
    let g = white_on_black(200, 64, |fb| { ttf_font::draw_text(fb, &mut font, "BA", 4, 8, 48.0, WHITE); });
    matches_reference(&g, "text/compound-transform.png", "8.6")
}

/// `round_half_away(v × 64)`, the 26.6 value of a pixel quantity.
fn to_26_6(v: f32) -> i32 {
    let s = v as f64 * 64.0;
    if s >= 0.0 { (s + 0.5).floor() as i32 } else { -((-s + 0.5).floor() as i32) }
}

fn text_line_metrics() -> CaseResult {
    const TABLE: [(f32, i32, i32, i32); 5] = [(8.0, 475, -121, 0), (12.0, 713, -181, 0), (16.0, 951, -242, 0),
        (24.0, 1426, -362, 0), (48.0, 2852, -725, 0)];
    for path in [MONO_PATH, SANS_PATH] {
        let font = parse_font(path).cite("8.5")?;
        for (size, asc, desc, gap) in TABLE {
            let m = font.metrics(size);
            let got = (to_26_6(m.ascender), to_26_6(m.descender), to_26_6(m.line_gap));
            ensure(got == (asc, desc, gap), "8.3, 8.5", format!("{path} at {size} px: ascent, descent and gap are {got:?} in 26.6; hhea gives {:?}", (asc, desc, gap)))?;
        }
    }
    Ok(())
}

fn text_size_range() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    let h = font.glyph_index('H');
    for size in 6..=72 {
        let b = font.rasterize_glyph(h, size as f32).map_err(|e| format!("'H' at {size} px does not rasterize: {e}")).cite("8.3")?;
        ensure(b.coverage.iter().any(|&c| c > 0), "8.3", format!("'H' at {size} px rasterized to nothing"))?;
    }
    for size in [0.0f32, -3.0, 0.5, 1024.5, 1025.0, 4096.0, f32::NAN] {
        let r = isolated(&format!("rasterize_glyph('H', {size})"), 3_000, || {
            match font.rasterize_glyph(h, size) {
                Err(libfont::FontError::BadSize) => Ok(()),
                Err(e) => Err(format!("size {size} px gave {e}; the contract's error is BadSize")),
                Ok(b) => Err(format!("size {size} px rasterized a {}x{} bitmap; the contract rejects sizes below 1 or above 1,024", b.width, b.height)),
            }
        });
        step("8.3", r)?;
    }
    for path in [MONO_PATH, SANS_PATH] {
        let file = read_file(path).cite("8.5")?;
        let font = Font::parse(&file).cite("8.5")?;
        for size in [6.0f32, 12.0, 24.0, 48.0, 72.0] {
            for cp in 0x21u32..0x7F {
                let g = font.glyph_index(char::from_u32(cp).unwrap());
                step("7", staged_within_bounds(&font, &file, g, size, &format!("{path} U+{cp:04X} at {size} px")))?;
            }
        }
    }
    Ok(())
}

/// Ten or more arguments with trailing f32 and 26.6 values, as the text entry points take.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn text_with_many(fb: &mut FrameBuf, font: &mut CachedFont, text: &str, x: i32, y: i32, r: u8, g: u8, b: u8,
    pad: i32, size26: i32, size: f32, scale: f32) -> i32 {
    let size = if size26 > 0 { size26 as f32 / 64.0 } else { size * scale };
    ttf_font::draw_text(fb, font, text, x + pad, y, size, Color::rgb(r, g, b))
}

fn text_many_args_abi() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let reference = white_on_black(96, 40, |fb| { ttf_font::draw_text(fb, &mut font, "Ag", 6, 4, 22.5, WHITE); });
    ensure(drawn_pixels(&reference, Color::rgb(0, 0, 0)) > 0, "8.3", "the direct call drew nothing")?;
    fn bb<T>(t: T) -> T { std::hint::black_box(t) }
    for (size26, size, scale) in [(0, 11.25f32, 2.0f32), (1440, 0.0, 0.0)] {
        let g = Guarded::bgrx(96, 40);
        let ret = text_with_many(&mut g.fb(), &mut font, bb("Ag"), bb(4), bb(4), bb(255), bb(255), bb(255), bb(2), bb(size26), bb(size), bb(scale));
        *g.reference.borrow_mut() = Some(canonical_pixels(&reference));
        g.judge();
        for y in 0..40 {
            for x in 0..96 {
                if g.rgb(x, y) != reference.rgb(x, y) {
                    return at("8.3", format!("a 12-argument call with size26 {size26}, size {size}, scale {scale} drew pixel ({x},{y}) as {:?}; the direct call at 22.5 px gives {:?} (returned {ret})",
                        g.rgb(x, y), reference.rgb(x, y)));
                }
            }
        }
    }
    Ok(())
}

fn text_cache_hit() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let g = font.glyph_index('Q');
    let fresh = font.font().rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?;
    let first = font.rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?.clone();
    let after_first = font.cache_stats();
    let second = font.rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?.clone();
    let after_second = font.cache_stats();
    for (name, b) in [("first", &first), ("cached", &second)] {
        ensure((b.width, b.height, b.x_offset, b.y_offset, &b.coverage) == (fresh.width, fresh.height, fresh.x_offset, fresh.y_offset, &fresh.coverage),
            "8.4", format!("the {name} rasterization of 'Q' differs from a fresh one"))?;
    }
    ensure((after_first.hits, after_first.misses, after_first.entries) == (0, 1, 1), "8.4",
        format!("after the first 'Q' the cache reports {after_first:?}; a first use is one miss and one entry"))?;
    ensure((after_second.hits, after_second.misses, after_second.entries) == (1, 1, 1), "8.4",
        format!("after the second 'Q' the cache reports {after_second:?}; the second use is a hit"))?;
    ensure(after_second.bytes == fresh.coverage.len(), "8.4",
        format!("the cache reports {} bytes holding one {} x {} glyph", after_second.bytes, fresh.width, fresh.height))
}

fn text_cache_bound() -> CaseResult {
    let mut font = CachedFont::with_default_cache(parse_font(MONO_PATH).cite("8.5")?);
    // 1,800 small pairs (glyphs 3 to 362 at five sizes), then U+2588 at 200 sizes from 150 px,
    // whose bitmaps outgrow 4 MiB long before 256 entries.
    let block = font.glyph_index('\u{2588}');
    ensure(block != 0, "8.5", "DejaVu Sans Mono does not map U+2588")?;
    let pairs: Vec<(u16, f32)> = (3..363u16).flat_map(|g| [8.0f32, 10.0, 12.0, 14.0, 16.0].map(|s| (g, s)))
        .chain((150..350).map(|s| (block, s as f32))).collect();
    ensure(pairs.len() == 2_000, "8.4", format!("the suite built {} pairs, not 2,000", pairs.len()))?;
    let (mut most_entries, mut most_bytes) = (0usize, 0usize);
    for &(g, size) in &pairs {
        font.rasterize_glyph(g, size).map_err(|e| format!("glyph {g} at {size} px: {e}")).cite("8.3")?;
        let s = font.cache_stats();
        ensure(s.entries <= 256 && s.bytes <= 4 << 20, "8.4", format!("after glyph {g} at {size} px the cache holds {} entries and {} bytes; the contract allows 256 and 4 MiB", s.entries, s.bytes))?;
        most_entries = most_entries.max(s.entries);
        most_bytes = most_bytes.max(s.bytes);
    }
    ensure(most_entries == 256, "8.4", format!("the cache never held more than {most_entries} entries; 2,000 distinct pairs fill all 256"))?;
    ensure(most_bytes > 3 << 20, "8.4", format!("the cache never held more than {most_bytes} bytes; the large glyphs reach the 4 MiB budget"))?;
    let s = font.cache_stats();
    ensure(s.misses == 2_000 && s.hits == 0, "8.4", format!("2,000 distinct pairs gave {} misses and {} hits", s.misses, s.hits))
}

fn text_cache_evict_correct() -> CaseResult {
    let mut font = CachedFont::new(parse_font(MONO_PATH).cite("8.5")?, 2);
    let (a, b, c) = (font.glyph_index('A'), font.glyph_index('B'), font.glyph_index('C'));
    let first = font.rasterize_glyph(a, 18.0).map_err(|e| e.to_string())?.clone();
    ensure(first.coverage.iter().any(|&c| c > 0), "8.4", "'A' at 18 px rasterized to nothing")?;
    for g in [b, c, b, c] {
        font.rasterize_glyph(g, 18.0).map_err(|e| e.to_string())?;
    }
    // With two entries, 'A' is the least recently used and has been evicted by now.
    let again = font.rasterize_glyph(a, 18.0).map_err(|e| e.to_string())?.clone();
    ensure((again.width, again.height, again.x_offset, again.y_offset, &again.coverage)
        == (first.width, first.height, first.x_offset, first.y_offset, &first.coverage),
        "8.4", "'A' rasterized again after eviction differs from its first rasterization")
}

const MONO_METRICS: [(u32, u16, u16, i16, i16, i16, i16, i16); 50] = [
    (0x0021, 4, 1233, 516, 516, 0, 719, 1493),
    (0x0022, 5, 1233, 338, 338, 938, 895, 1493),
    (0x0023, 6, 1233, 2, 2, 0, 1229, 1470),
    (0x0024, 7, 1233, 190, 190, -301, 1114, 1556),
    (0x0025, 8, 1233, 33, 33, 0, 1200, 1432),
    (0x0026, 9, 1233, 57, 57, -29, 1221, 1520),
    (0x0027, 10, 1233, 528, 528, 938, 702, 1493),
    (0x0028, 11, 1233, 426, 426, -270, 885, 1554),
    (0x0029, 12, 1233, 348, 348, -270, 807, 1554),
    (0x002A, 13, 1233, 166, 166, 586, 1067, 1520),
    (0x002B, 14, 1233, 88, 88, 113, 1145, 1171),
    (0x002C, 15, 1233, 403, 403, -287, 754, 303),
    (0x002D, 16, 1233, 356, 356, 479, 877, 643),
    (0x002E, 17, 1233, 489, 489, 0, 741, 305),
    (0x002F, 18, 1233, 102, 102, -190, 1079, 1493),
    (0x0030, 19, 1233, 133, 133, -29, 1100, 1520),
    (0x0031, 20, 1233, 246, 246, 0, 1094, 1493),
    (0x0032, 21, 1233, 152, 152, 0, 1059, 1520),
    (0x0033, 22, 1233, 137, 137, -29, 1079, 1520),
    (0x0034, 23, 1233, 102, 102, 0, 1135, 1493),
    (0x0035, 24, 1233, 143, 143, -29, 1069, 1493),
    (0x0036, 25, 1233, 133, 133, -29, 1100, 1520),
    (0x0037, 26, 1233, 139, 139, 0, 1079, 1493),
    (0x0038, 27, 1233, 131, 131, -29, 1102, 1520),
    (0x0039, 28, 1233, 127, 127, -29, 1094, 1520),
    (0x003A, 29, 1233, 489, 489, 0, 741, 1063),
    (0x003B, 30, 1233, 403, 403, -287, 754, 1063),
    (0x003C, 31, 1233, 88, 88, 141, 1145, 1143),
    (0x003D, 32, 1233, 88, 88, 352, 1145, 930),
    (0x003E, 33, 1233, 88, 88, 141, 1145, 1143),
    (0x003F, 34, 1233, 244, 244, 0, 1040, 1520),
    (0x0040, 35, 1233, 27, 27, -319, 1178, 1395),
    (0x0041, 36, 1233, 37, 37, 0, 1196, 1493),
    (0x0042, 37, 1233, 166, 166, 0, 1137, 1493),
    (0x0043, 38, 1233, 139, 139, -29, 1073, 1520),
    (0x0044, 39, 1233, 137, 137, 0, 1106, 1493),
    (0x0045, 40, 1233, 197, 197, 0, 1102, 1493),
    (0x0046, 41, 1233, 233, 233, 0, 1112, 1493),
    (0x0047, 42, 1233, 102, 102, -29, 1104, 1520),
    (0x0048, 43, 1233, 137, 137, 0, 1096, 1493),
    (0x0049, 44, 1233, 201, 201, 0, 1030, 1493),
    (0x004A, 45, 1233, 109, 109, -29, 956, 1493),
    (0x004B, 46, 1233, 137, 137, 0, 1225, 1493),
    (0x004C, 47, 1233, 215, 215, 0, 1139, 1493),
    (0x004D, 48, 1233, 86, 86, 0, 1145, 1493),
    (0x004E, 49, 1233, 139, 139, 0, 1094, 1493),
    (0x004F, 50, 1233, 117, 117, -29, 1116, 1520),
    (0x0050, 51, 1233, 197, 197, 0, 1141, 1493),
    (0x0051, 52, 1233, 117, 117, -270, 1116, 1520),
    (0x0052, 53, 1233, 143, 143, 0, 1233, 1493),
];
const MONO_CMAP: [(u32, u16); 200] = [
    (0x0020, 3), (0x0021, 4), (0x0022, 5), (0x0023, 6), (0x0024, 7), (0x0025, 8), (0x0026, 9), (0x0027, 10),
    (0x0028, 11), (0x0029, 12), (0x002A, 13), (0x002B, 14), (0x002C, 15), (0x002D, 16), (0x002E, 17), (0x002F, 18),
    (0x0030, 19), (0x0031, 20), (0x0032, 21), (0x0033, 22), (0x0034, 23), (0x0035, 24), (0x0036, 25), (0x0037, 26),
    (0x0038, 27), (0x0039, 28), (0x003A, 29), (0x003B, 30), (0x003C, 31), (0x003D, 32), (0x003E, 33), (0x003F, 34),
    (0x0040, 35), (0x0041, 36), (0x0042, 37), (0x0043, 38), (0x0044, 39), (0x0045, 40), (0x0046, 41), (0x0047, 42),
    (0x0048, 43), (0x0049, 44), (0x004A, 45), (0x004B, 46), (0x004C, 47), (0x004D, 48), (0x004E, 49), (0x004F, 50),
    (0x0050, 51), (0x0051, 52), (0x0052, 53), (0x0053, 54), (0x0054, 55), (0x0055, 56), (0x0056, 57), (0x0057, 58),
    (0x0058, 59), (0x0059, 60), (0x005A, 61), (0x005B, 62), (0x005C, 63), (0x005D, 64), (0x005E, 65), (0x005F, 66),
    (0x0060, 67), (0x0061, 68), (0x0062, 69), (0x0063, 70), (0x0064, 71), (0x0065, 72), (0x0066, 73), (0x0067, 74),
    (0x0068, 75), (0x0069, 76), (0x006A, 77), (0x006B, 78), (0x006C, 79), (0x006D, 80), (0x006E, 81), (0x006F, 82),
    (0x0070, 83), (0x0071, 84), (0x0072, 85), (0x0073, 86), (0x0074, 87), (0x0075, 88), (0x0076, 89), (0x0077, 90),
    (0x0078, 91), (0x0079, 92), (0x007A, 93), (0x007B, 94), (0x007C, 95), (0x007D, 96), (0x007E, 97), (0x00A0, 98),
    (0x00A1, 99), (0x00A2, 100), (0x00A3, 101), (0x00A4, 102), (0x00A5, 103), (0x00A6, 104), (0x00A7, 105), (0x00A8, 106),
    (0x00A9, 107), (0x00AA, 108), (0x00AB, 109), (0x00AC, 110), (0x00AD, 111), (0x00AE, 112), (0x00AF, 113), (0x00B0, 114),
    (0x00B1, 115), (0x00B2, 116), (0x00B3, 117), (0x00B4, 118), (0x00B5, 119), (0x00B6, 120), (0x00B7, 121), (0x00B8, 122),
    (0x00B9, 123), (0x00BA, 124), (0x00BB, 125), (0x00BC, 126), (0x00BD, 127), (0x00BE, 128), (0x00BF, 129), (0x00C0, 130),
    (0x00C1, 131), (0x00C2, 132), (0x00C3, 133), (0x00C4, 134), (0x00C5, 135), (0x00C6, 136), (0x00C7, 137), (0x00C8, 138),
    (0x00C9, 139), (0x00CA, 140), (0x00CB, 141), (0x00CC, 142), (0x00CD, 143), (0x00CE, 144), (0x00CF, 145), (0x00D0, 146),
    (0x00D1, 147), (0x00D2, 148), (0x00D3, 149), (0x00D4, 150), (0x00D5, 151), (0x00D6, 152), (0x00D7, 153), (0x00D8, 154),
    (0x00D9, 155), (0x00DA, 156), (0x00DB, 157), (0x00DC, 158), (0x00DD, 159), (0x00DE, 160), (0x00DF, 161), (0x00E0, 162),
    (0x00E1, 163), (0x00E2, 164), (0x00E3, 165), (0x00E4, 166), (0x00E5, 167), (0x00E6, 168), (0x00E7, 169), (0x00E8, 170),
    (0x00E9, 171), (0x00EA, 172), (0x00EB, 173), (0x00EC, 174), (0x00ED, 175), (0x00EE, 176), (0x00EF, 177), (0x00F0, 178),
    (0x00F1, 179), (0x00F2, 180), (0x00F3, 181), (0x00F4, 182), (0x00F5, 183), (0x00F6, 184), (0x00F7, 185), (0x00F8, 186),
    (0x00F9, 187), (0x00FA, 188), (0x00FB, 189), (0x00FC, 190), (0x00FD, 191), (0x00FE, 192), (0x00FF, 193), (0x0100, 194),
    (0x2013, 1822), (0x2014, 1823), (0x2018, 1827), (0x2019, 1828), (0x201C, 1831), (0x201D, 1832), (0xE000, 0), (0x10FFFF, 0),
];

// ---------------------------------------------------------------------------------------
// font-parser: font parsing safety (contract sections 7 and 10)
// ---------------------------------------------------------------------------------------

/// A TrueType font assembled here from literal tables, for one rule at a time.
#[derive(Clone)]
struct Ttf {
    units_per_em: u16,
    /// Raw glyf entries, one per glyph.
    glyphs: Vec<Vec<u8>>,
    /// maxp numGlyphs (normally glyphs.len()), maxPoints and maxCompositePoints.
    num_glyphs: u16,
    max_points: u16,
    max_composite_points: u16,
    /// hmtx: long metrics, then trailing left side bearings.
    long_metrics: Vec<(u16, i16)>,
    trailing_lsb: Vec<i16>,
    num_h_metrics: u16,
    cmap: Vec<u8>,
    /// loca entries overriding the computed ones, and whole-table overrides.
    loca: Option<Vec<u32>>,
    hmtx: Option<Vec<u8>>,
    /// An optional kern table, and directory records (tag, offset, length) after the tables.
    kern: Option<Vec<u8>>,
    records: Vec<([u8; 4], u32, u32)>,
}

fn be16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_be_bytes()); }
fn be32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_be_bytes()); }

/// A simple glyph: contours of on-curve points, coordinates as 16-bit deltas.
fn simple_glyph(contours: &[&[(i16, i16)]]) -> Vec<u8> {
    let pts: Vec<(i16, i16)> = contours.iter().flat_map(|c| c.iter().copied()).collect();
    let mut v = Vec::new();
    be16(&mut v, contours.len() as u16);
    let (xs, ys) = (pts.iter().map(|p| p.0), pts.iter().map(|p| p.1));
    for b in [xs.clone().min().unwrap_or(0), ys.clone().min().unwrap_or(0), xs.max().unwrap_or(0), ys.max().unwrap_or(0)] {
        be16(&mut v, b as u16);
    }
    let mut end = 0usize;
    for c in contours {
        end += c.len();
        be16(&mut v, (end - 1) as u16);
    }
    be16(&mut v, 0);
    v.extend(std::iter::repeat_n(0x01u8, pts.len()));
    let (mut px, mut py) = (0i16, 0i16);
    for &(x, _) in &pts { be16(&mut v, (x - px) as u16); px = x; }
    for &(_, y) in &pts { be16(&mut v, (y - py) as u16); py = y; }
    v
}

/// A compound glyph of components `(glyph, dx, dy)`, offsets as words.
fn compound_glyph(parts: &[(u16, i16, i16)]) -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 0xFFFF);
    for b in [0i16, 0, 1000, 1000] { be16(&mut v, b as u16); }
    for (i, &(g, dx, dy)) in parts.iter().enumerate() {
        let more = if i + 1 < parts.len() { 0x0020 } else { 0 };
        be16(&mut v, 0x0001 | 0x0002 | more);
        be16(&mut v, g);
        be16(&mut v, dx as u16);
        be16(&mut v, dy as u16);
    }
    v
}

/// A cmap table holding one (3, 1) format 4 subtable of segments `(start, end, delta, glyph_ids)`.
fn cmap4(segments: &[(u16, u16, i16, Option<Vec<u16>>)], seg_count_x2: Option<u16>) -> Vec<u8> {
    let n = segments.len();
    let mut sub = Vec::new();
    let arrays_len: usize = segments.iter().map(|s| s.3.as_ref().map_or(0, |a| a.len())).sum();
    be16(&mut sub, 4);
    be16(&mut sub, (16 + 8 * n + 2 * arrays_len) as u16);
    be16(&mut sub, 0);
    be16(&mut sub, seg_count_x2.unwrap_or(2 * n as u16));
    for _ in 0..3 { be16(&mut sub, 0); }
    for s in segments { be16(&mut sub, s.1); }
    be16(&mut sub, 0);
    for s in segments { be16(&mut sub, s.0); }
    for s in segments { be16(&mut sub, s.2 as u16); }
    // idRangeOffset: from each entry to its glyph array, which follows every offset.
    let mut array_at = 0usize;
    for (i, s) in segments.iter().enumerate() {
        match &s.3 {
            Some(a) => { be16(&mut sub, (2 * (n - i) + 2 * array_at) as u16); array_at += a.len(); }
            None => be16(&mut sub, 0),
        }
    }
    for s in segments { if let Some(a) = &s.3 { for &g in a { be16(&mut sub, g); } } }
    let mut v = Vec::new();
    be16(&mut v, 0);
    be16(&mut v, 1);
    be16(&mut v, 3);
    be16(&mut v, 1);
    be32(&mut v, 12);
    v.extend(sub);
    v
}

/// A cmap table holding one (3, 10) format 12 subtable of groups `(start, end, glyph)`.
fn cmap12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [0u16, 1, 3, 10] { be16(&mut v, x); }
    be32(&mut v, 12);
    be16(&mut v, 12);
    be16(&mut v, 0);
    be32(&mut v, (16 + 12 * groups.len()) as u32);
    be32(&mut v, 0);
    be32(&mut v, groups.len() as u32);
    for &(a, b, g) in groups { be32(&mut v, a); be32(&mut v, b); be32(&mut v, g); }
    v
}

const SQUARE: &[(i16, i16)] = &[(100, 0), (100, 700), (600, 700), (600, 0)];
const TRIANGLE: &[(i16, i16)] = &[(0, 0), (350, 700), (700, 0)];

impl Ttf {
    /// Glyph 0 (.notdef) and glyphs 1 to 3: a square, a triangle and a compound of the square.
    fn base() -> Ttf {
        let glyphs = vec![simple_glyph(&[SQUARE]), simple_glyph(&[SQUARE]), simple_glyph(&[TRIANGLE]), compound_glyph(&[(1, 100, 0)])];
        Ttf {
            units_per_em: 1000,
            num_glyphs: glyphs.len() as u16,
            glyphs,
            max_points: 4,
            max_composite_points: 8,
            long_metrics: vec![(700, 100), (700, 100), (720, 0), (800, 100)],
            trailing_lsb: Vec::new(),
            num_h_metrics: 4,
            cmap: cmap4(&[(0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)], None),
            loca: None,
            hmtx: None,
            kern: None,
            records: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for g in &self.glyphs {
            loca.push(glyf.len() as u32);
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 { glyf.push(0); }
        }
        loca.push(glyf.len() as u32);
        let loca = self.loca.clone().unwrap_or(loca);
        let mut loca_bytes = Vec::new();
        for o in loca { be32(&mut loca_bytes, o); }
        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0);
        be32(&mut head, 0x5F0F_3CF5);
        be16(&mut head, 0);
        be16(&mut head, self.units_per_em);
        head.extend_from_slice(&[0; 16]);
        for b in [0i16, 0, 1000, 1000] { be16(&mut head, b as u16); }
        be16(&mut head, 0);
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, 1);
        be16(&mut head, 0);
        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        for x in [800i16, -200, 0] { be16(&mut hhea, x as u16); }
        be16(&mut hhea, 800);
        hhea.extend_from_slice(&[0; 22]);
        be16(&mut hhea, self.num_h_metrics);
        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0001_0000);
        be16(&mut maxp, self.num_glyphs);
        be16(&mut maxp, self.max_points);
        be16(&mut maxp, 4);
        be16(&mut maxp, self.max_composite_points);
        be16(&mut maxp, 2);
        be16(&mut maxp, 2);
        maxp.extend_from_slice(&[0; 12]);
        be16(&mut maxp, 4);
        be16(&mut maxp, 2);
        let hmtx = self.hmtx.clone().unwrap_or_else(|| {
            let mut v = Vec::new();
            for &(a, l) in &self.long_metrics { be16(&mut v, a); be16(&mut v, l as u16); }
            for &l in &self.trailing_lsb { be16(&mut v, l as u16); }
            v
        });
        let mut tables: Vec<(&[u8; 4], &Vec<u8>)> = vec![(b"cmap", &self.cmap), (b"glyf", &glyf), (b"head", &head), (b"hhea", &hhea),
            (b"hmtx", &hmtx)];
        if let Some(k) = &self.kern {
            tables.push((b"kern", k));
        }
        tables.extend([(b"loca", &loca_bytes), (b"maxp", &maxp)]);
        let count = tables.len() + self.records.len();
        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        be16(&mut out, count as u16);
        for x in [64u16, 2, 48] { be16(&mut out, x); }
        let mut offset = 12 + 16 * count;
        let mut body = Vec::new();
        for (tag, data) in tables {
            out.extend_from_slice(tag);
            be32(&mut out, 0);
            be32(&mut out, offset as u32);
            be32(&mut out, data.len() as u32);
            body.extend_from_slice(data);
            while body.len() % 4 != 0 { body.push(0); }
            offset = 12 + 16 * count + body.len();
        }
        for &(tag, at_, len) in &self.records {
            out.extend_from_slice(&tag);
            be32(&mut out, 0);
            be32(&mut out, at_);
            be32(&mut out, len);
        }
        out.extend(body);
        out
    }
}

const CORPUS_DIR: &str = "/usr/local/test/share/breengel/fonts-corpus";

/// One line of the corpus index: the file, the glyph and size a glyph-scoped defect is
/// read at, and the documented error.
struct CorpusEntry {
    file: String,
    glyph: Option<(u16, f32)>,
    error: String,
}

fn corpus_index() -> Result<Vec<CorpusEntry>, String> {
    let path = format!("{CORPUS_DIR}/index.txt");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read the corpus index {path}: {e}"))?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let entry = match f.as_slice() {
            [file, "font", error] => CorpusEntry { file: file.to_string(), glyph: None, error: error.to_string() },
            [file, "glyph", g, size, error] => CorpusEntry {
                file: file.to_string(),
                glyph: Some((g.parse().map_err(|_| format!("bad glyph in {line:?}"))?, size.parse().map_err(|_| format!("bad size in {line:?}"))?)),
                error: error.to_string(),
            },
            _ => return Err(format!("{path}: malformed line {line:?}")),
        };
        out.push(entry);
    }
    if out.len() != 64 {
        return Err(format!("{path} lists {} files; the contract's corpus has 64", out.len()));
    }
    Ok(out)
}

/// A glyph record with the given header box and contour of `(x, y, on_curve)` points,
/// coordinates as wrapping 16-bit deltas.
fn raw_glyph(contour: &[(i16, i16, bool)], bbox: [i16; 4]) -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 1);
    for b in bbox { be16(&mut v, b as u16); }
    be16(&mut v, contour.len() as u16 - 1);
    be16(&mut v, 0);
    v.extend(contour.iter().map(|p| if p.2 { 0x01u8 } else { 0x00 }));
    let (mut px, mut py) = (0i16, 0i16);
    for &(x, _, _) in contour { be16(&mut v, x.wrapping_sub(px) as u16); px = x; }
    for &(_, y, _) in contour { be16(&mut v, y.wrapping_sub(py) as u16); py = y; }
    v
}

/// Parse `bytes` in a child: Ok(whether it parsed) or the crash.
fn parses(what: &str, bytes: Vec<u8>) -> Result<bool, String> {
    let r = isolated(what, 3_000, move || match Font::parse(&bytes) {
        Ok(_) => Err("parsed".into()),
        Err(_) => Ok(()),
    });
    match r {
        Ok(()) => Ok(false),
        Err(msg) if msg == "parsed" => Ok(true),
        Err(msg) => Err(msg),
    }
}

/// The font must be rejected whole.
fn rejected(section: &str, what: &str, bytes: Vec<u8>) -> CaseResult {
    match parses(what, bytes) {
        Ok(false) => Ok(()),
        Ok(true) => at(section, format!("{what} parsed; the contract rejects it")),
        Err(msg) => at(section, msg),
    }
}

/// The font parses and only `bad` is rejected: every other glyph rasterizes.
fn glyph_rejected(section: &str, what: &str, bytes: Vec<u8>, bad: u16, others: &[u16]) -> CaseResult {
    let others = others.to_vec();
    step(section, isolated(what, 3_000, move || {
        let font = Font::parse(&bytes).map_err(|e| format!("{what}: the font was rejected whole ({e}); only glyph {bad} is bad"))?;
        if let Ok(b) = font.rasterize_glyph(bad, 24.0) {
            return Err(format!("{what}: glyph {bad} rasterized ({}x{}); the contract rejects that glyph", b.width, b.height));
        }
        for g in others {
            font.rasterize_glyph(g, 24.0).map_err(|e| format!("{what}: glyph {g}, which is valid, was rejected too ({e})"))?;
        }
        Ok(())
    }))
}

fn font_parser_truncated() -> CaseResult {
    let full = Ttf::base().build();
    let len = full.len();
    Font::parse(&full).map_err(|e| format!("the complete test font does not parse: {e}")).cite("10")?;
    step("10", isolated("parsing every proper prefix of the test font", 8_000, move || {
        for cut in 0..len {
            if Font::parse(&full[..cut]).is_ok() {
                return Err(format!("the first {cut} of {len} bytes parsed; the contract rejects a truncated font"));
            }
        }
        Ok(())
    }))
}

fn font_parser_table_offsets() -> CaseResult {
    let good = Ttf::base().build();
    let len = good.len() as u32;
    Font::parse(&good).map_err(|e| format!("the unmodified test font does not parse: {e}")).cite("10")?;
    // Records start at 12 and are 16 bytes: tag, checksum, offset, length.
    for (record, field, value) in [(1usize, 8usize, len + 4), (1, 12, len), (6, 8, u32::MAX - 2), (0, 12, 0x7FFF_FFFF)] {
        let mut bad = good.clone();
        let at_ = 12 + 16 * record + field;
        bad[at_..at_ + 4].copy_from_slice(&value.to_be_bytes());
        let tag = String::from_utf8_lossy(&good[12 + 16 * record..12 + 16 * record + 4]).into_owned();
        rejected("10", &format!("a font whose {tag} {} is {value:#x}", if field == 8 { "offset" } else { "length" }), bad)?;
    }
    Ok(())
}

fn font_parser_loca_range() -> CaseResult {
    let mut t = Ttf::base();
    let glyf_len: u32 = t.glyphs.iter().map(|g| g.len().div_ceil(4) as u32 * 4).sum();
    // Glyph 3 is the last; its entry and the end entry both point past the glyf table.
    let mut loca: Vec<u32> = Vec::new();
    let mut o = 0;
    for g in &t.glyphs[..3] { loca.push(o); o += g.len().div_ceil(4) as u32 * 4; }
    loca.extend([glyf_len + 40, glyf_len + 80]);
    t.loca = Some(loca.clone());
    glyph_rejected("10", "a font whose glyph 3 lies past the glyf table", t.build(), 3, &[1, 2])?;
    // Equal entries past the table: glyph 3 would be empty if they were in range.
    loca[3..].copy_from_slice(&[glyf_len + 40, glyf_len + 40]);
    t.loca = Some(loca);
    glyph_rejected("10", "a font whose glyph 3 starts and ends past the glyf table", t.build(), 3, &[1, 2])
}

fn font_parser_loca_order() -> CaseResult {
    let mut t = Ttf::base();
    let mut loca: Vec<u32> = Vec::new();
    let mut o = 0;
    for g in &t.glyphs { loca.push(o); o += g.len().div_ceil(4) as u32 * 4; }
    loca.push(o);
    loca.swap(1, 2);
    t.loca = Some(loca);
    rejected("10", "a font whose loca entries decrease", t.build())
}

fn font_parser_numglyphs() -> CaseResult {
    let mut t = Ttf::base();
    t.num_glyphs = 6;
    rejected("10", "a font whose maxp counts 6 glyphs and whose loca holds 4", t.build())?;
    let mut t = Ttf::base();
    t.num_glyphs = 2;
    rejected("10", "a font whose maxp counts 2 glyphs and whose loca holds 4", t.build())
}

fn font_parser_cmap4_segments() -> CaseResult {
    let segs = [(0x41u16, 0x43u16, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)];
    let mut t = Ttf::base();
    t.cmap = cmap4(&segs, Some(3));
    rejected("10", "a format 4 cmap whose segCountX2 is odd", t.build())?;
    let mut t = Ttf::base();
    t.cmap = cmap4(&[(0x50, 0x52, 1 - 0x50, None), (0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)], None);
    rejected("10", "a format 4 cmap whose end codes are not sorted", t.build())?;
    let mut t = Ttf::base();
    t.cmap = cmap4(&[(0x41, 0x43, 1 - 0x41, None)], None);
    rejected("10", "a format 4 cmap whose last end code is not 0xFFFF", t.build())
}

fn font_parser_cmap4_odd_segments() -> CaseResult {
    // 38 one-codepoint segments from U+0020 every 3 codepoints, glyph 1 + i % 3; segment 10
    // maps through its glyph array; then the 0xFFFF segment: 39 segments.
    let mut segs: Vec<(u16, u16, i16, Option<Vec<u16>>)> = (0..38u16).map(|i| {
        let cp = 0x20 + 3 * i;
        let g = 1 + i % 3;
        if i == 10 { (cp, cp, 0, Some(vec![g])) } else { (cp, cp, g as i16 - cp as i16, None) }
    }).collect();
    segs.push((0xFFFF, 0xFFFF, 1, None));
    let mut t = Ttf::base();
    t.cmap = cmap4(&segs, None);
    let font = Font::parse(&t.build()).map_err(|e| format!("a valid 39-segment format 4 cmap was rejected: {e}")).cite("10")?;
    for cp in 0x20u32..0x20 + 200 {
        let want = if cp < 0x20 + 3 * 38 && (cp - 0x20) % 3 == 0 { 1 + ((cp - 0x20) / 3) % 3 } else { 0 } as u16;
        let got = font.glyph_index(char::from_u32(cp).unwrap());
        ensure(got == want, "10", format!("U+{cp:04X} maps to glyph {got}; the 39-segment table gives {want}"))?;
    }
    Ok(())
}

fn font_parser_cmap4_range_offset() -> CaseResult {
    let mut t = Ttf::base();
    let mut cmap = cmap4(&[(0x41, 0x43, 0, Some(vec![1, 2, 3])), (0xFFFF, 0xFFFF, 1, None)], None);
    // The first idRangeOffset is at 12 + 14 + 3 × 4 + 2; point it 4,000 bytes on.
    let at_ = 12 + 16 + 3 * 2 * 2;
    cmap[at_..at_ + 2].copy_from_slice(&4000u16.to_be_bytes());
    t.cmap = cmap;
    rejected("10", "a format 4 cmap whose idRangeOffset points outside the subtable", t.build())
}

fn font_parser_cmap12_groups() -> CaseResult {
    for (what, groups) in [
        ("overlapping", vec![(0x41u32, 0x45u32, 1u32), (0x44, 0x46, 2)]),
        ("unsorted", vec![(0x50, 0x51, 1), (0x41, 0x42, 2)]),
        ("inverted", vec![(0x45, 0x41, 1)]),
    ] {
        let mut t = Ttf::base();
        t.cmap = cmap12(&groups);
        rejected("10", &format!("a format 12 cmap with {what} groups"), t.build())?;
    }
    Ok(())
}

/// Glyph 2 replaced by `glyph`, with every other glyph intact.
fn with_glyph2(glyph: Vec<u8>) -> Vec<u8> {
    let mut t = Ttf::base();
    t.glyphs[2] = glyph;
    t.build()
}

fn font_parser_glyf_endpoints() -> CaseResult {
    let mut g = simple_glyph(&[&[(0, 0), (10, 10), (20, 0)], &[(30, 0), (40, 10), (50, 0)]]);
    // End points 2 and 5 become 5 and 2.
    g[10..14].copy_from_slice(&[0, 5, 0, 2]);
    glyph_rejected("10", "glyph 2 with end points 5, 2", with_glyph2(g), 2, &[1, 3])
}

fn font_parser_glyf_points() -> CaseResult {
    let g = simple_glyph(&[&[(0, 0), (10, 10), (20, 0), (30, 10), (40, 0), (50, 10)]]);
    glyph_rejected("10", "glyph 2 with 6 points where maxp allows 4", with_glyph2(g), 2, &[1, 3])?;
    // A compound is held to maxCompositePoints: three squares, 12 points, where maxPoints is
    // 16 and maxCompositePoints 8.
    let mut t = Ttf::base();
    t.glyphs[2] = compound_glyph(&[(1, 0, 0), (1, 100, 0), (1, 200, 0)]);
    t.max_points = 16;
    glyph_rejected("10", "glyph 2, a compound of 12 points where maxCompositePoints is 8", t.build(), 2, &[1, 3])
}

fn font_parser_glyf_flags() -> CaseResult {
    let mut g = Vec::new();
    be16(&mut g, 1);
    for b in [0i16, 0, 100, 100] { be16(&mut g, b as u16); }
    be16(&mut g, 3);
    be16(&mut g, 0);
    // Four points, but the first flag repeats 200 times.
    g.extend_from_slice(&[0x01 | 0x08, 200]);
    g.extend_from_slice(&[0; 16]);
    glyph_rejected("10", "glyph 2 whose flag repeat runs past its 4 points", with_glyph2(g), 2, &[1, 3])?;
    // A compound of the square with WE_HAVE_INSTRUCTIONS (0x0100) and byte arguments, so its
    // record is 16 bytes with no padding after it, followed by `tail`.
    let with_instructions = |tail: &[u8]| {
        let mut g = Vec::new();
        be16(&mut g, 0xFFFF);
        for b in [0i16, 0, 1000, 1000] { be16(&mut g, b as u16); }
        for x in [0x0002 | 0x0100, 1, 0] { be16(&mut g, x); }
        g.extend_from_slice(tail);
        with_glyph2(g)
    };
    glyph_rejected("10", "glyph 2, a compound declaring instructions that end at its last component", with_instructions(&[]), 2, &[1, 3])?;
    glyph_rejected("10", "glyph 2, a compound whose 50 instruction bytes run past its record", with_instructions(&[0, 50, 1, 2]), 2, &[1, 3])?;
    let file = with_instructions(&[0, 2, 1, 2]);
    step("10", isolated("a compound with two instruction bytes", 3_000, move || {
        let font = Font::parse(&file).map_err(|e| e.to_string())?;
        font.rasterize_glyph(2, 24.0).map(|_| ()).map_err(|e| format!("glyph 2, a compound with its two instruction bytes, was rejected: {e}"))
    }))
}

fn font_parser_compound_depth() -> CaseResult {
    // Glyphs 4 to 12 nest: glyph 4 holds the square, each later one the one before, so
    // glyph 11 is 8 deep and glyph 12 is 9 deep.
    let mut t = Ttf::base();
    for depth in 1..=9u16 {
        t.glyphs.push(compound_glyph(&[(if depth == 1 { 1 } else { 3 + depth - 1 }, 10, 0)]));
        t.long_metrics.push((700, 0));
    }
    t.num_glyphs = t.glyphs.len() as u16;
    t.num_h_metrics = t.long_metrics.len() as u16;
    let bytes = t.build();
    glyph_rejected("10", "a compound glyph nested 9 deep", bytes.clone(), 12, &[1, 4, 11])?;
    step("10", isolated("the 8-deep compound", 3_000, move || {
        let font = Font::parse(&bytes).map_err(|e| e.to_string())?;
        let b = font.rasterize_glyph(11, 24.0).map_err(|e| format!("glyph 11, nested 8 deep, was rejected: {e}"))?;
        if b.coverage.iter().any(|&c| c > 0) { Ok(()) } else { Err("glyph 11, nested 8 deep, rasterized to nothing; it is the square".into()) }
    }))
}

fn font_parser_compound_cycle() -> CaseResult {
    let mut t = Ttf::base();
    t.glyphs.push(compound_glyph(&[(4, 0, 0)]));
    t.glyphs.push(compound_glyph(&[(6, 0, 0)]));
    t.glyphs.push(compound_glyph(&[(5, 0, 0)]));
    t.long_metrics.extend([(700, 0); 3]);
    t.num_glyphs = 7;
    t.num_h_metrics = 7;
    let bytes = t.build();
    glyph_rejected("10", "glyph 4 that contains itself", bytes.clone(), 4, &[1, 3])?;
    glyph_rejected("10", "glyphs 5 and 6 that contain each other", bytes, 5, &[1, 3])
}

fn font_parser_compound_missing() -> CaseResult {
    glyph_rejected("10", "glyph 2, a compound of glyph 99 in a 4-glyph font", with_glyph2(compound_glyph(&[(99, 0, 0)])), 2, &[1, 3])
}

fn font_parser_hmtx_compressed() -> CaseResult {
    let file = read_file(MONO_PATH).cite("8.5")?;
    let (hmtx_at, hmtx_len) = ttf_table(&file, b"hmtx").ok_or_else(|| "no hmtx table".to_string()).cite("8.5")?;
    let hmtx = libfont::tables::hmtx::HmtxTable::new(&file[hmtx_at..hmtx_at + hmtx_len], 4);
    let font = Font::parse(&file).cite("8.5")?;
    for &(cp, gid, adv, lsb, ..) in MONO_METRICS.iter() {
        ensure(gid >= 4, "8.5", "the table's glyphs are all past the long metrics")?;
        let (got_adv, got_lsb, scaled) = (hmtx.advance_width(gid), hmtx.left_side_bearing(gid), font.advance_width(gid, 2048.0));
        ensure(got_adv == adv && scaled == adv as f32 && got_lsb == lsb, "10", format!(
            "U+{cp:04X} (glyph {gid}): advance {got_adv} (font {scaled}) and lsb {got_lsb}; the compressed hmtx gives {adv} and {lsb}"))?;
    }
    // A synthetic font with two long metrics for four glyphs.
    let mut t = Ttf::base();
    t.long_metrics = vec![(700, 100), (650, 50)];
    t.trailing_lsb = vec![-20, 33];
    t.num_h_metrics = 2;
    let font = Font::parse(&t.build()).map_err(|e| format!("a valid compressed hmtx was rejected: {e}")).cite("10")?;
    for (g, want) in [(0u16, 700.0f32), (1, 650.0), (2, 650.0), (3, 650.0)] {
        let got = font.advance_width(g, 1000.0);
        ensure(got == want, "10", format!("glyph {g}'s advance is {got}; with two long metrics it is {want}"))?;
    }
    Ok(())
}

fn font_parser_hmtx_truncated() -> CaseResult {
    let mut t = Ttf::base();
    let mut short = Vec::new();
    for &(a, l) in &t.long_metrics[..3] { be16(&mut short, a); be16(&mut short, l as u16); }
    t.hmtx = Some(short);
    rejected("10", "an hmtx of 12 bytes for 4 long metrics", t.build())?;
    for nhm in [0u16, 5] {
        let mut t = Ttf::base();
        t.num_h_metrics = nhm;
        rejected("10", &format!("an hhea numberOfHMetrics of {nhm} for 4 glyphs"), t.build())?;
    }
    Ok(())
}

fn font_parser_units_per_em() -> CaseResult {
    for upem in [0u16, 16_385, 65_535] {
        let mut t = Ttf::base();
        t.units_per_em = upem;
        rejected("10", &format!("a font with unitsPerEm {upem}"), t.build())?;
    }
    let mut t = Ttf::base();
    t.units_per_em = 16_384;
    Font::parse(&t.build()).map(|_| ()).map_err(|e| format!("unitsPerEm 16,384 is valid but was rejected: {e}")).cite("10")
}

fn font_parser_coords_extreme() -> CaseResult {
    use libfont::{FontError, GlyphLayout};
    let (lo, hi) = (i16::MIN, i16::MAX);
    let square = [(lo, lo, true), (lo, hi, true), (hi, hi, true), (hi, lo, true)];
    let swings: Vec<(i16, i16, bool)> = (0..130i16).flat_map(|i| [(i * 4, 0, true), (i * 4 + 2, if i % 2 == 1 { hi } else { lo }, false)]).collect();
    let cases: [(&str, Vec<u8>); 5] = [
        ("a square at the i16 limits with its true box", raw_glyph(&square, [lo, lo, hi, hi])),
        ("the same square under a 10-unit box", raw_glyph(&square, [0, 0, 10, 10])),
        ("a triangle with a vertex at each limit under a 10-unit box", raw_glyph(&[(lo, 0, true), (0, hi, true), (hi, lo, true)], [0, 0, 10, 10])),
        ("130 quadratics swinging between the limits under a 10-unit box", raw_glyph(&swings, [0, 0, 10, 10])),
        ("off-curve points alternating between opposite corners", raw_glyph(&[(lo, lo, false), (hi, hi, false), (lo, hi, false), (hi, lo, false)], [0, 0, 10, 10])),
    ];
    for (what, glyph) in cases {
        let mut t = Ttf::base();
        t.glyphs[2] = glyph;
        t.max_points = 260;
        let file = t.build();
        for size in [1.0f32, 24.0, 1024.0] {
            let file = file.clone();
            let name = format!("{what} at {size} px");
            step("7, 10", isolated(&name.clone(), 8_000, move || {
                let font = Font::parse(&file).map_err(|e| format!("{name}: the font was rejected ({e})"))?;
                let (bytes, points) = independent_glyph(&file, 2)?;
                libfont::work::reset();
                let outline = font.glyph_outline(2).map_err(|e| format!("{name}: no outline ({e})"))?;
                let steps = libfont::work::steps();
                if steps > 16 * bytes + 8 * points + 4_096 {
                    return Err(format!("{name}: its outline took {steps} steps; section 7 allows {}", 16 * bytes + 8 * points + 4_096));
                }
                let size26 = Font::size26(size).map_err(|e| e.to_string())?;
                libfont::work::reset();
                let layout = font.layout_glyph(&outline, 2, size26);
                let steps = libfont::work::steps();
                if steps > 1_028 * points {
                    return Err(format!("{name}: flattening {points} points took {steps} steps; section 7 allows {}", 1_028 * points));
                }
                let (segments, width, height) = match layout {
                    Err(FontError::GlyphTooComplex | FontError::GlyphTooExpensive) => return Ok(()),
                    Err(e) => return Err(format!("{name}: refused with {e}; the contract refuses such a glyph only with GlyphTooComplex or GlyphTooExpensive")),
                    Ok(GlyphLayout::Empty { .. }) => return Err(format!("{name}: laid out as empty")),
                    Ok(GlyphLayout::Outline { segments, width, height, .. }) => (segments, width, height),
                };
                libfont::work::reset();
                let raster = libfont::rasterizer::rasterize(&segments, width, height, 0, 0);
                let steps = libfont::work::steps();
                match raster {
                    Err(FontError::GlyphTooExpensive) if steps == 0 => Ok(()),
                    Err(FontError::GlyphTooExpensive) => Err(format!("{name}: refused with GlyphTooExpensive after {steps} raster steps; the refusal comes before any raster loop")),
                    Err(e) => Err(format!("{name}: rasterization refused with {e}")),
                    Ok(_) => {
                        let bound = raster_steps_bound(segments.len() as u64, height as u64, width as u64);
                        if steps > bound {
                            Err(format!("{name}: rasterizing {} segments into {width} x {height} took {steps} steps; section 7 allows {bound}", segments.len()))
                        } else {
                            Ok(())
                        }
                    }
                }
            }))?;
        }
    }
    Ok(())
}

fn font_parser_work_bound() -> CaseResult {
    use libfont::outline::LineSegment;
    // The independent square of section 7: four edges 1,022 px apart with a 1-pixel margin,
    // built here, in a 1,024 × 1,024 bitmap.
    let (a, b) = (1.0f32, 1023.0f32);
    let square = [
        LineSegment { x0: a, y0: a, x1: a, y1: b },
        LineSegment { x0: a, y0: b, x1: b, y1: b },
        LineSegment { x0: b, y0: b, x1: b, y1: a },
        LineSegment { x0: b, y0: a, x1: a, y1: a },
    ];
    ensure(raster_steps_bound(4, 1_024, 1_024) == 10_699_844, "7", "the suite's raster bound for the square is not the contract's 10,699,844")?;
    libfont::work::reset();
    let bitmap = libfont::rasterizer::rasterize(&square, 1_024, 1_024, 0, 0).cite("7")?;
    let steps = libfont::work::steps();
    value("square-steps", steps as i64, "steps", Some((8_388_608, 10_699_844)));
    ensure(bitmap.coverage[512 * 1_024 + 512] == 255 && bitmap.coverage[0] == 0, "7", "the square rasterized to the wrong coverage")?;
    ensure((8_388_608..=10_699_844).contains(&steps), "7", format!(
        "the 1,024 x 1,024 square took {steps} steps; section 7 requires at least 8,388,608 (8 x 1,024 x 1,024) and at most 10,699,844"))?;
    for entry in corpus_index().cite("10")? {
        let path = format!("{CORPUS_DIR}/{}", entry.file);
        let file = read_file(&path).cite("10")?;
        let name = entry.file.clone();
        step("7", isolated(&name.clone(), 8_000, move || {
            libfont::work::reset();
            let parsed = Font::parse(&file);
            let steps = libfont::work::steps();
            let bound = 16 * file.len() as u64 + 65_536;
            if steps > bound {
                return Err(format!("{name}: loading took {steps} steps; section 7 allows 16 x {} + 65,536 = {bound}", file.len()));
            }
            let Ok(font) = parsed else { return Ok(()) };
            let size = entry.glyph.map_or(24.0, |(_, s)| s);
            for g in 0..font.num_glyphs() {
                let what = format!("{name} glyph {g} at {size} px");
                let (bytes, points) = independent_glyph(&file, g)?;
                libfont::work::reset();
                match font.rasterize_glyph(g, size) {
                    Ok(_) => staged_within_bounds(&font, &file, g, size, &what)?,
                    Err(e) => {
                        let steps = libfont::work::steps();
                        let bound = 16 * bytes + 8 * points + 4_096 + 1_028 * points;
                        if steps > bound {
                            return Err(format!("{what}: refusing it ({e}) took {steps} steps; its outline and flattening bounds allow {bound}"));
                        }
                    }
                }
            }
            Ok(())
        }))?;
    }
    // 4,096 Unicode cmap records all naming one 4,096-group format 12 subtable.
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 4_096);
    for _ in 0..4_096 {
        for x in [3u16, 10] { be16(&mut cmap, x); }
        be32(&mut cmap, 4 + 8 * 4_096);
    }
    for x in [12u32 << 16, 16 + 12 * 4_096, 0, 4_096] { be32(&mut cmap, x); }
    for i in 0..4_096u32 {
        for x in [2 * i, 2 * i, 1] { be32(&mut cmap, x); }
    }
    let mut t = Ttf::base();
    t.cmap = cmap;
    let file = t.build();
    step("7", isolated("4,096 cmap records sharing one subtable", 8_000, move || {
        libfont::work::reset();
        let parsed = Font::parse(&file);
        let steps = libfont::work::steps();
        parsed.map_err(|e| format!("a font whose 4,096 cmap records share one valid subtable was rejected: {e}"))?;
        let bound = 16 * file.len() as u64 + 65_536;
        if steps > bound {
            return Err(format!("4,096 cmap records sharing one 4,096-group subtable: loading took {steps} steps; section 7 allows {bound}"));
        }
        Ok(())
    }))?;
    // A 10-byte glyph declaring 32,767 contours, and one contour of 65,535 points in 526
    // bytes (repeated flags; coordinates that repeat the previous one take no bytes).
    let mut cut = Vec::new();
    be16(&mut cut, 32_767);
    cut.extend_from_slice(&[0; 8]);
    let mut dense = Vec::new();
    be16(&mut dense, 1);
    dense.extend_from_slice(&[0; 8]);
    be16(&mut dense, 65_534);
    be16(&mut dense, 0);
    for k in 0..256 {
        dense.extend_from_slice(&[0x31 | 0x08, if k < 255 { 255 } else { 254 }]);
    }
    for (what, glyph) in [("a 10-byte glyph declaring 32,767 contours", cut), ("one contour of 65,535 points in 526 bytes", dense)] {
        let mut t = Ttf::base();
        t.glyphs[2] = glyph;
        t.max_points = 65_535;
        let file = t.build();
        step("7", isolated(what, 8_000, move || {
            let font = Font::parse(&file).map_err(|e| format!("{what}: the font was rejected ({e})"))?;
            let (bytes, points) = independent_glyph(&file, 2)?;
            libfont::work::reset();
            let outline = font.glyph_outline(2);
            let steps = libfont::work::steps();
            let bound = 16 * bytes + 8 * points + 4_096;
            if steps > bound {
                return Err(format!("{what}: its outline took {steps} steps ({}); section 7 allows 16 x {bytes} + 8 x {points} + 4,096 = {bound}",
                    outline.map_or_else(|e| e.to_string(), |o| format!("{} points", o.point_count()))));
            }
            Ok(())
        }))?;
    }
    Ok(())
}

fn font_parser_alloc_bound() -> CaseResult {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in corpus_index().cite("10")? {
        let path = format!("{CORPUS_DIR}/{}", entry.file);
        files.push((entry.file, read_file(&path).cite("10")?));
    }
    for path in [MONO_PATH, SANS_PATH] {
        files.push((path.to_string(), read_file(path).cite("8.5")?));
    }
    // A directory of 20,000 more records, and glyph 2 of 32,767 one-point contours.
    let mut t = Ttf::base();
    t.records = (0..20_000u32).map(|i| (*b"zz00", i % 64, 0)).collect();
    files.push(("a font with 20,007 table records".to_string(), t.build()));
    let mut many = Vec::new();
    be16(&mut many, 32_767);
    many.extend_from_slice(&[0; 8]);
    for i in 0..32_767u16 { be16(&mut many, i); }
    be16(&mut many, 0);
    for k in 0..128 {
        many.extend_from_slice(&[0x31 | 0x08, if k < 127 { 255 } else { 254 }]);
    }
    let mut t = Ttf::base();
    t.glyphs[2] = many;
    t.max_points = 32_767;
    files.push(("a font whose glyph 2 has 32,767 one-point contours".to_string(), t.build()));
    for (name, file) in files {
        let before = ALLOC_BYTES.load(Relaxed);
        let parsed = Font::parse(&file);
        let allocated = ALLOC_BYTES.load(Relaxed) - before;
        let bound = 2 * file.len() as u64 + 65_536;
        ensure(allocated <= bound, "7", format!("{name}: loading allocated {allocated} bytes; section 7 allows 2 x {} + 64 KiB = {bound}", file.len()))?;
        let Ok(font) = parsed else { continue };
        for g in 0..font.num_glyphs().min(400) {
            let (_, points) = independent_glyph(&file, g).cite("7")?;
            let before = ALLOC_BYTES.load(Relaxed);
            let _ = font.glyph_outline(g);
            let allocated = ALLOC_BYTES.load(Relaxed) - before;
            let bound = 64 * points + 65_536;
            ensure(allocated <= bound, "7", format!("{name} glyph {g}: its outline allocated {allocated} bytes; section 7 allows 64 x {points} + 64 KiB = {bound}"))?;
        }
    }
    Ok(())
}

fn font_parser_corpus() -> CaseResult {
    for entry in corpus_index().cite("10")? {
        let path = format!("{CORPUS_DIR}/{}", entry.file);
        let file = read_file(&path).cite("10")?;
        let name = entry.file.clone();
        step("10", isolated(&name.clone(), 3_000, move || {
            match (Font::parse(&file), entry.glyph) {
                (Err(e), None) if e.to_string() == entry.error => Ok(()),
                (Err(e), None) => Err(format!("{name}: Font::parse gave {e}; the index documents {}", entry.error)),
                (Ok(_), None) => Err(format!("{name} parsed; the index documents {}", entry.error)),
                (Err(e), Some(_)) => Err(format!("{name}: the font was rejected whole ({e}); the index confines {} to one glyph", entry.error)),
                (Ok(font), Some((g, size))) => match font.rasterize_glyph(g, size) {
                    Ok(b) => Err(format!("{name}: glyph {g} rasterized ({}x{}); the index documents {}", b.width, b.height, entry.error)),
                    Err(e) if e.to_string() != entry.error => Err(format!("{name}: glyph {g} gave {e}; the index documents {}", entry.error)),
                    Err(_) => font.rasterize_glyph(1, size).map(|_| ())
                        .map_err(|e| format!("{name}: glyph 1, which is valid, was refused too ({e}); the defect is glyph {g}'s alone")),
                },
            }
        }))?;
    }
    Ok(())
}
