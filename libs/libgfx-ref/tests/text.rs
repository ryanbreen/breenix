//! Host checks of libgfx's TrueType text against docs/gui/breengel.md sections 7 and 8.3.
//! The guest suite (/sbin/suite-breengel) gives the verdict.

use libgfx_ref::ttf_font::{CachedFont, Font};
use libgfx_ref::color::Color;
use libgfx_ref::framebuf::{Format, FrameBuf};
use libgfx_ref::ttf_font;

static MONO: &[u8] = include_bytes!("../../../fonts/DejaVuSansMono.ttf");
const WHITE: Color = Color::rgb(255, 255, 255);

fn font() -> CachedFont {
    CachedFont::with_default_cache(Font::parse(MONO).unwrap())
}

#[test]
fn text_wholly_outside_the_surface_rasterizes_no_glyph() {
    let line = "W".repeat(1_000);
    for (x, y) in [(i32::MAX, 10), (i32::MIN, 10), (10, i32::MAX), (10, -1_000), (2_000, 5)] {
        let mut f = font();
        let mut fb = FrameBuf::new(64, 32, Format::BGRX).unwrap();
        let width = ttf_font::draw_text(&mut fb, &mut f, &line, x, y, 16.0, WHITE);
        let stats = f.cache_stats();
        assert_eq!((stats.misses, stats.entries), (0, 0), "text at ({x}, {y}) rasterized a glyph");
        assert_eq!(width, ttf_font::text_width(&mut f, &line, 16.0));
        assert!(fb.take_dirty().is_none(), "text at ({x}, {y}) marked damage");
    }
}

#[test]
fn text_partly_outside_draws_exactly_its_visible_part() {
    let mut whole = FrameBuf::new(160, 32, Format::BGRX).unwrap();
    ttf_font::draw_text(&mut whole, &mut font(), "AVg|Wy", 40, 4, 16.0, WHITE);
    for dx in [-70, -45, -13, 0, 90, 130] {
        let mut part = FrameBuf::new(160, 32, Format::BGRX).unwrap();
        ttf_font::draw_text(&mut part, &mut font(), "AVg|Wy", 40 + dx, 4, 16.0, WHITE);
        for y in 0..32 {
            for x in 0..160i32 {
                let src = x - dx;
                if (0..160).contains(&src) {
                    assert_eq!(part.get_pixel(x as usize, y), whole.get_pixel(src as usize, y), "dx {dx}: pixel ({x}, {y})");
                }
            }
        }
    }
}
