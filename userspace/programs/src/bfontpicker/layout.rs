use libbui::text::ScaledText;
use libgfx::ttf_font::CachedFont;

pub struct Layout {
    pub text: ScaledText,
    pub width: i32,
    pub list_top: i32,
    pub row_h: i32,
    pub list_bottom: i32,
    pub preview_y: i32,
    pub preview_h: i32,
    pub status_y: i32,
}
impl Layout {
    pub fn new(font: Option<&CachedFont>, size: f32, scale: u32, width: i32, height: i32) -> Self {
        // Labels use the shared Noto raster; previews use the selected TTF.
        let text = ScaledText::new(None, 16.0, scale);
        let status_y = (height - text.px(20).max(text.line_h + text.px(2))).max(0);
        let preview = ScaledText::new(font, size, scale);
        let preview_h = text.px(100).max(text.px(32) + 2 * preview.line_h)
            .min((status_y - text.px(30)).max(0));
        let preview_y = status_y - preview_h;
        Self { text, width, list_top: text.px(30), row_h: text.line_h + text.px(4),
            list_bottom: preview_y - text.px(4), preview_y, preview_h, status_y }
    }
    pub fn visible(&self) -> usize { ((self.list_bottom - self.list_top) / self.row_h).max(0) as usize }
    pub fn row_rect(&self, row: usize) -> [i32; 4] {
        [0, self.list_top + row as i32 * self.row_h, self.width, self.row_h]
    }
    pub fn hit_row(&self, x: i32, y: i32, scroll: usize) -> Option<usize> {
        if x < 0 || x >= self.width || y < self.list_top { return None; }
        let row = ((y - self.list_top) / self.row_h) as usize;
        (row < self.visible()).then_some(scroll + row)
    }
}
