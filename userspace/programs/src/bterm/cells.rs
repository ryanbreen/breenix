//! Terminal cell dimensions in device pixels.
use libgfx::{bitmap_font, ttf_font::CachedFont};

/// Round the TTF cell in logical units before scaling, so the grid keeps the
/// same logical spacing at either UI scale. Bitmap cells use their raster's metrics.
pub fn cell_dims(font: Option<&mut CachedFont>, logical_size: f32, ui_scale: u32) -> (usize, usize) {
    let (_, raster) = libbui::text::scaled_font(logical_size, ui_scale);
    let fallback = || {
        let metrics = bitmap_font::metrics_at(raster);
        (metrics.char_width, metrics.line_height())
    };
    let Some(font) = font else { return fallback(); };
    let metrics = font.metrics(logical_size);
    let advance = font.advance_width(font.glyph_index('M'), logical_size);
    if !advance.is_finite() || !metrics.ascender.is_finite() || !metrics.descender.is_finite()
        || advance <= 0.0 || advance > 200.0 || metrics.ascender <= 0.0 || metrics.ascender > 200.0 {
        return fallback();
    }
    let w = (advance + 0.5) as i32;
    let h = (metrics.ascender + 0.99) as i32 + ((-metrics.descender) + 0.99) as i32;
    (w.max(1) as usize * ui_scale as usize, h.max(1) as usize * ui_scale as usize)
}
