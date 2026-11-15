use libbui::text::ScaledText;
use libgfx::ttf_font::CachedFont;
use super::input;

pub struct Layout { pub text: ScaledText, pub width: i32, pub height: i32 }
impl Layout {
    pub fn new(_font: Option<&CachedFont>, _size: f32, scale: u32, width: i32, height: i32) -> Self {
        let mut text = ScaledText::new(None, 7.0, scale);
        text.cell_w = text.px(6);
        text.line_h = text.px(8);
        Self { text, width, height }
    }
    pub fn logical_point(&self, x: i32, y: i32) -> (i32, i32) {
        (x.div_euclid(self.text.scale as i32), y.div_euclid(self.text.scale as i32))
    }
    pub fn grid_center(&self, i: usize) -> (i32, i32) {
        let (x,y) = input::grid_center(i); (self.text.px(x), self.text.px(y))
    }
    pub fn radius(&self) -> i32 { self.text.px(input::HOVER_RADIUS) }
    pub fn icon_rect(&self, i: usize) -> [i32; 4] {
        let (x,y) = self.grid_center(i); let r = self.radius(); [x-r,y-r,2*r,2*r]
    }
    pub fn preview(&self) -> (i32, i32) {
        (self.width / 2, (self.height - self.text.px(155)).max(0))
    }
}
