//! Device rectangles derived from Gus Kit's logical content layout.
use libbui::Rect;

#[derive(Clone, Copy)]
pub struct Layout {
    pub scale: i32,
    pub tools: [Rect; 7],
    pub tool_text: [(i32, i32); 7],
    pub sizes: [Rect; 3],
    pub size_text: [(i32, i32); 3],
    pub palette: Rect,
    pub hue: Rect,
    pub sv: Rect,
    pub recent_label: Rect,
    pub recent_text: (i32, i32),
    pub recent: [Rect; 8],
    pub status: Rect,
    pub picker: Rect,
    pub session: Rect,
    pub session_title: Rect,
    pub address_label: Rect,
    pub address: Rect,
    pub name_label: Rect,
    pub name: Rect,
    pub session_status: Rect,
    pub confirm: Rect,
    pub confirm_label: Rect,
    pub canvas_y: i32,
}
impl Layout {
    pub fn new(scale: u32) -> Self {
        let scale = scale as i32;
        let r = |x, y, w, h| Rect::new(x * scale, y * scale, w * scale, h * scale);
        Self {
            scale,
            tools: core::array::from_fn(|i| r(2 + i as i32 * 38, 23, 36, 16)),
            tool_text: core::array::from_fn(|i| ((4 + i as i32 * 38) * scale, 25 * scale)),
            sizes: core::array::from_fn(|i| r(2 + i as i32 * 22, 41, 20, 16)),
            size_text: core::array::from_fn(|i| ((5 + i as i32 * 22) * scale, 42 * scale)),
            palette: r(0, 58, 780, 118), hue: r(0, 58, 780, 18), sv: r(0, 76, 100, 100),
            recent_label: r(108, 76, 144, 14), recent_text: (108 * scale, 78 * scale),
            recent: core::array::from_fn(|i| r(108 + i as i32 * 18, 90, 16, 16)),
            status: r(110, 150, 690, 26), picker: r(150, 100, 500, 400),
            session: r(220, 200, 360, 180), session_title: r(230, 210, 340, 20),
            address_label: r(230, 230, 340, 20), address: r(270, 250, 300, 24),
            name_label: r(230, 280, 340, 20), name: r(270, 300, 300, 24),
            session_status: r(230, 340, 340, 30),
            confirm: r(180, 220, 440, 64), confirm_label: r(190, 242, 420, 20),
            canvas_y: 176 * scale,
        }
    }
    pub fn logical(&self, r: Rect) -> Rect {
        Rect::new(r.x / self.scale, r.y / self.scale, r.w / self.scale, r.h / self.scale)
    }
    pub fn canvas(&self, width: usize, height: usize) -> Rect {
        Rect::new(0, self.canvas_y, width as i32, height as i32)
    }
    pub fn point(&self, x: i32, y: i32, width: usize, height: usize) -> Option<(i32, i32)> {
        let r = self.canvas(width, height);
        r.contains(x, y).then_some((x - r.x, y - r.y))
    }
    pub fn hue_at(&self, x: i32) -> u16 {
        super::node::hue_at(((x - self.hue.x) / self.scale) as usize, (self.hue.w / self.scale) as usize)
    }
    pub fn sv_at(&self, x: i32, y: i32) -> (u8, u8) {
        super::node::sv_at(((x - self.sv.x) / self.scale) as usize,
            ((y - self.sv.y) / self.scale) as usize, (self.sv.w / self.scale) as usize)
    }
}
