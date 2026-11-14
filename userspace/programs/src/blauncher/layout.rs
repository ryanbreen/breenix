use libbui::text::ScaledText;
use libgfx::ttf_font::CachedFont;

pub struct Layout {
    pub text: ScaledText,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub search_x: i32,
    pub search_y: i32,
    pub search_w: i32,
    pub search_h: i32,
    pub list_y: i32,
    pub item_h: i32,
    pub visible_count: usize,
}
impl Layout {
    pub fn new(font: Option<&CachedFont>, size: f32, scale: u32, width: i32, _height: i32) -> Self {
        let text = ScaledText::new(font, size, scale);
        let w = text.px(500).min(width);
        let x = (width - w) / 2;
        let padding = text.px(16);
        let search_h = text.px(44).max(text.line_h + text.px(8));
        let item_h = text.px(36).max(2 * text.line_h + text.px(4));
        Self { text, x, y: 0, w, h: 0, search_x: x + padding, search_y: padding,
            search_w: w - 2 * padding, search_h, list_y: padding + search_h + text.px(8),
            item_h, visible_count: 0 }
    }
    pub fn panel(mut self, count: usize) -> Self {
        self.visible_count = count.min(12);
        self.h = self.list_y + self.visible_count.max(1) as i32 * self.item_h + self.text.px(40);
        self
    }
    pub fn row_rect(&self, row: usize) -> [i32; 4] {
        [self.x + self.text.px(8), self.list_y + row as i32 * self.item_h,
            self.w - self.text.px(16), self.item_h]
    }
    pub fn hit_row(&self, x: i32, y: i32) -> Option<usize> {
        let r = self.row_rect(0);
        if x < r[0] || x >= r[0] + r[2] || y < r[1] { return None; }
        let row = ((y - r[1]) / self.item_h) as usize;
        (row < self.visible_count).then_some(row)
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}
