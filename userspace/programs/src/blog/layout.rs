use libbui::text::ScaledText;
use libgfx::ttf_font::CachedFont;

pub struct Layout {
    pub text: ScaledText,
    pub width: i32,
    pub content_y: i32,
    pub status_y: i32,
    pub status_h: i32,
    pub visible: usize,
}
impl Layout {
    pub fn new(font: Option<&CachedFont>, size: f32, scale: u32, width: i32, height: i32) -> Self {
        let text = ScaledText::new(font, size, scale);
        let content_y = text.px(20);
        let status_h = text.px(18).max(text.line_h + text.px(4));
        let status_y = (height - status_h).max(content_y);
        let visible = ((status_y - content_y) / text.line_h).max(0) as usize;
        Self { text, width, content_y, status_y, status_h, visible }
    }
    pub fn row_rect(&self, row: usize) -> [i32; 4] {
        [0, self.content_y + row as i32 * self.text.line_h, self.width, self.text.line_h]
    }
}
