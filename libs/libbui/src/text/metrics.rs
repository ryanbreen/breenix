use super::scaled_font;
use libgfx::{bitmap_font, ttf_font::CachedFont};

/// Text and spacing in device pixels, rounded in logical units first.
#[derive(Clone, Copy)]
pub struct ScaledText {
    pub size: f32,
    pub raster: bitmap_font::RasterHeight,
    pub cell_w: i32,
    pub line_h: i32,
    pub scale: u32,
}
impl ScaledText {
    pub fn new(font: Option<&CachedFont>, logical_size: f32, scale: u32) -> Self {
        let (size, raster) = scaled_font(logical_size, scale);
        let (cell_w, line_h) = if let Some(font) = font {
            let m = font.metrics(logical_size);
            let w = (font.advance_width(font.glyph_index('M'), logical_size) + 0.5) as i32;
            let h = (m.ascender + 0.99) as i32 + (-m.descender + 0.99) as i32;
            (w.max(1) * scale as i32, h.max(1) * scale as i32)
        } else {
            let m = bitmap_font::metrics_at(raster);
            (m.char_width as i32, m.line_height() as i32)
        };
        Self { size, raster, cell_w, line_h, scale }
    }
    pub fn px(self, logical: i32) -> i32 { logical * self.scale as i32 }
}
