use super::*;

fn font() -> CachedFont {
    CachedFont::new(Font::parse(include_bytes!("../../../fonts/DejaVuSansMono.ttf")).unwrap(), 8)
}

fn coverage(font: &mut CachedFont, glyph: u16, size26: i32, lcd: bool, calls: &mut usize) -> Arc<AtlasGlyph> {
    font.lookup_or_rasterize(glyph, size26, lcd, |font| {
        *calls += 1;
        // A coverage token replaces GPU allocation; the lookup and insertion are production code.
        Ok(font.insert(glyph, size26, lcd, AtlasGlyph::empty(*calls), None))
    }).unwrap()
}

#[test]
fn repeated_lcd_glyph_rasterizes_coverage_once() {
    let mut font = font();
    let mut calls = 0;
    let first = coverage(&mut font, 36, 1024, true, &mut calls);
    for _ in 0..16 {
        let next = coverage(&mut font, 36, 1024, true, &mut calls);
        assert!(Arc::ptr_eq(&first, &next));
    }
    assert_eq!(calls, 1);
    let stats = font.cache_stats();
    assert_eq!((stats.hits, stats.misses, stats.entries), (16, 1, 1));
}

#[test]
fn lcd_coverage_is_distinct_from_grayscale_size_and_glyph() {
    let mut font = font();
    let mut calls = 0;
    let lcd = coverage(&mut font, 36, 1024, true, &mut calls);
    for (glyph, size, subpixel) in [(36, 1024, false), (36, 2048, true), (37, 1024, true)] {
        let other = coverage(&mut font, glyph, size, subpixel, &mut calls);
        assert!(!Arc::ptr_eq(&lcd, &other));
    }
    assert!(Arc::ptr_eq(&lcd, &coverage(&mut font, 36, 1024, true, &mut calls)));
    assert_eq!(calls, 4);
    assert_eq!(font.cache_stats().entries, 4);
}
