use libbui::text::ScaledText;
use libgfx::ttf_font::CachedFont;

pub struct Layout {
    pub text: ScaledText,
    pub width: i32,
    pub height: i32,
    pub row_h: i32,
    pub category_h: i32,
    pub list_y: i32,
    pub footer_h: i32,
    pub bar_y: i32,
    pub bar_h: i32,
}
impl Layout {
    pub fn new(font: Option<&CachedFont>, size: f32, scale: u32, width: i32, height: i32) -> Self {
        let mut text = ScaledText::new(font, size, scale);
        // The self-check's fallback is the existing 5x7 face, with a 6x8 cell.
        if font.is_none() { text.cell_w = text.px(6); text.line_h = text.px(8); }
        let row_h = text.px(18).max(text.line_h + text.px(2));
        let category_h = text.px(12).max(text.line_h + text.px(2));
        let bar_y = text.px(10) + 2 * text.line_h + text.px(8);
        let bar_h = text.px(16).max(text.line_h + text.px(8));
        let list_y = bar_y + bar_h + text.px(10);
        let footer_h = text.px(24).max(text.line_h + text.px(8));
        Self { text, width, height, row_h, category_h, list_y, footer_h, bar_y, bar_h }
    }
    pub fn visible_h(&self) -> i32 { (self.height - self.list_y - self.footer_h).max(0) }
    pub fn scroll_rect(&self) -> [i32; 4] {
        [self.width - self.text.px(12), self.list_y, self.text.px(12), self.visible_h()]
    }
}
