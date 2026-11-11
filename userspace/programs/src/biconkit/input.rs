use libicon::icon::{Icon, IconMouse};

pub fn grid_center(i: usize) -> (i32, i32) { ((80 + (i % 3) as i32 * 120).min(270), 75 + (i / 3) as i32 * 80) }
pub const HOVER_RADIUS: i32 = 30;

pub fn icon_mouse_for(x: i32, y: i32, down: bool, click: bool, release: bool, cx: i32, cy: i32, radius: i32) -> IconMouse {
    let dx = x as i64 - cx as i64; let dy = y as i64 - cy as i64;
    let hovering = dx * dx + dy * dy < (radius as i64) * (radius as i64);
    IconMouse { hovering, pressed: hovering && down, just_clicked: hovering && click, just_released: hovering && release,
        rel_x: if hovering && radius > 0 { dx as f32 / radius as f32 } else { 0.0 },
        rel_y: if hovering && radius > 0 { dy as f32 / radius as f32 } else { 0.0 } }
}

pub struct Input { pub selected: usize, pub x: i32, pub y: i32, pub down: bool, click: bool, release: bool }
impl Input {
    pub fn new() -> Self { Self { selected: 0, x: 0, y: 0, down: false, click: false, release: false } }
    pub fn move_to(&mut self, x: i32, y: i32, icons: &mut [Box<dyn Icon>]) { self.x=x; self.y=y; self.tick(1000, icons); }
    pub fn button(&mut self, down: bool, x: i32, y: i32, _icons: &mut [Box<dyn Icon>]) {
        self.x=x; self.y=y; self.click |= down && !self.down; self.release |= !down && self.down; self.down=down;
    }
    pub fn tick(&mut self, dt: u32, icons: &mut [Box<dyn Icon>]) {
        for (i, icon) in icons.iter_mut().enumerate() {
            let (cx,cy)=grid_center(i); let mouse=icon_mouse_for(self.x,self.y,self.down,self.click,self.release,cx,cy,HOVER_RADIUS);
            if mouse.just_released { self.selected=i; }
            icon.update(dt,mouse);
        }
        self.click=false; self.release=false;
    }
    pub fn navigate(&mut self, key: u8, count: usize) -> bool {
        let next = match key { b',' | b'<' => self.selected.saturating_sub(1), b'.' | b'>' if self.selected + 1 < count => self.selected+1, _ => self.selected };
        let changed=next != self.selected; self.selected=next; changed
    }
}
