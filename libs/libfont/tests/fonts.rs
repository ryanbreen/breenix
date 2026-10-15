//! Host checks of libfont against the pinned fonts and docs/gui/breengel.md sections 7, 8.5
//! and 10. The guest suite (/sbin/suite-breengel) gives the verdict; these are the
//! development loop.

use libfont::outline::LineSegment;
use libfont::rasterizer::{rasterize, raster_bound};
use libfont::{work, CachedFont, Font, FontError};

static MONO: &[u8] = include_bytes!("../../../fonts/DejaVuSansMono.ttf");
static SANS: &[u8] = include_bytes!("../../../fonts/DejaVuSans.ttf");

#[test]
fn pinned_fonts_parse_with_the_independent_values() {
    let mono = Font::parse(MONO).unwrap();
    let sans = Font::parse(SANS).unwrap();
    assert_eq!((mono.units_per_em(), mono.num_glyphs()), (2048, 3377));
    assert_eq!((sans.units_per_em(), sans.num_glyphs()), (2048, 6253));
    for cp in 0x20..=0x7E {
        assert_eq!(mono.glyph_for(cp), (cp - 0x1D) as u16, "U+{cp:04X}");
    }
    for (cp, g) in [(0x0100, 194), (0x2013, 1822), (0x2014, 1823), (0x2018, 1827), (0x2019, 1828), (0x201C, 1831), (0x201D, 1832), (0xE000, 0), (0x10FFFF, 0)] {
        assert_eq!(mono.glyph_for(cp), g, "U+{cp:04X}");
    }
    let m = mono.glyph_metrics(0x21 - 0x1D).unwrap();
    assert_eq!((m.advance, m.lsb, m.x_min, m.y_min, m.x_max, m.y_max), (1233, 516, 516, 0, 719, 1493));
    let s26 = Font::size26(16.0).unwrap();
    assert_eq!(mono.advance26(mono.glyph_for('H' as u32), s26), 617);
    let s26 = Font::size26(32.0).unwrap();
    let (a, v) = (sans.glyph_for('A' as u32), sans.glyph_for('V' as u32));
    assert_eq!((a, v), (36, 57));
    assert_eq!((sans.advance26(a, s26), sans.advance26(v, s26), sans.kern26(a, v, s26)), (1401, 1401, -131));
    for (size, asc, desc) in [(8.0, 475, -121), (12.0, 713, -181), (16.0, 951, -242), (24.0, 1426, -362), (48.0, 2852, -725)] {
        for f in [&mono, &sans] {
            let m = f.metrics(size);
            assert_eq!(((m.ascender * 64.0) as i32, (m.descender * 64.0) as i32, (m.line_gap * 64.0) as i32), (asc, desc, 0));
        }
    }
}

#[test]
fn every_glyph_of_the_pinned_fonts_rasterizes() {
    for (name, data) in [("mono", MONO), ("sans", SANS)] {
        let font = Font::parse(data).unwrap();
        for g in 0..font.num_glyphs() {
            for size in [6.0, 16.0, 72.0] {
                if let Err(e) = font.rasterize_glyph(g, size) {
                    panic!("{name} glyph {g} at {size} px: {e}");
                }
            }
        }
    }
}

#[test]
fn sizes_outside_1_to_1024_are_refused() {
    let font = Font::parse(MONO).unwrap();
    let h = font.glyph_for('H' as u32);
    for size in [0.0, 0.99, -3.0, 1024.01, 4096.0, f32::NAN] {
        assert_eq!(font.rasterize_glyph(h, size).err(), Some(FontError::BadSize), "{size}");
    }
    for size in [1.0, 1024.0] {
        assert!(font.rasterize_glyph(h, size).is_ok(), "{size}");
    }
}

#[test]
fn ascii_at_every_tested_size_stays_within_the_bounds() {
    for data in [MONO, SANS] {
        let font = Font::parse(data).unwrap();
        for size in [6.0f32, 12.0, 24.0, 48.0, 72.0] {
            for cp in 0x21..0x7F {
                let g = font.glyph_for(cp);
                let outline = font.glyph_outline(g).unwrap();
                let p = outline.points() as u64;
                work::reset();
                let scale = size / 2048.0;
                let segs = libfont::outline::flatten(&outline, scale, 0.0, 0.0).unwrap();
                assert!(work::steps() <= 1028 * p, "flatten U+{cp:04X} at {size}: {} steps for {p} points", work::steps());
                let bitmap = font.rasterize_glyph(g, size).unwrap();
                work::reset();
                let _ = font.rasterize_glyph(g, size).unwrap();
                let total = work::steps();
                let bound = raster_bound(segs.len(), bitmap.height, bitmap.width) as u64 + 16 * 4096 + 1028 * p;
                assert!(total <= bound, "U+{cp:04X} at {size}: {total} > {bound}");
            }
        }
    }
}

#[test]
fn the_independent_square_counts_its_span_fill() {
    let (x0, y0, x1, y1) = (1.0f32, 1.0, 1023.0, 1023.0);
    let segs = [
        LineSegment { x0, y0, x1: x0, y1 },
        LineSegment { x0, y0: y1, x1, y1 },
        LineSegment { x0: x1, y0: y1, x1, y1: y0 },
        LineSegment { x0: x1, y0, x1: x0, y1: y0 },
    ];
    work::reset();
    let b = rasterize(&segs, 1024, 1024, 0, 0).unwrap();
    let steps = work::steps();
    assert_eq!(raster_bound(4, 1024, 1024), 10_699_844);
    assert!((8_388_608..=10_699_844).contains(&steps), "{steps} steps");
    assert_eq!(b.coverage[512 * 1024 + 512], 255);
    assert_eq!(b.coverage[0], 0);
}

#[test]
fn the_cache_reports_hits_and_keeps_its_budgets() {
    let mut font = CachedFont::with_default_cache(Font::parse(MONO).unwrap());
    let q = font.glyph_index('Q');
    let fresh = font.font().rasterize_glyph(q, 20.0).unwrap();
    let first = font.rasterize_glyph(q, 20.0).unwrap().clone();
    let second = font.rasterize_glyph(q, 20.0).unwrap().clone();
    assert_eq!((first.clone(), second), (fresh.clone(), fresh));
    let s = font.cache_stats();
    assert_eq!((s.hits, s.misses, s.entries), (1, 1, 1));
    for g in 0..400u16 {
        for size in [9.0, 31.0, 64.0, 100.0, 150.0] {
            font.rasterize_glyph(g, size).unwrap();
            let s = font.cache_stats();
            assert!(s.entries <= 256 && s.bytes <= 4 << 20, "{s:?}");
        }
    }
    font.clear();
    assert_eq!(font.cache_stats().entries, 0);
}
