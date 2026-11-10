//! Independent expected RGB pixels for the drawing scenarios.
const W: usize = 200;
const H: usize = 120;

pub struct Expected(pub Vec<u8>);
impl Expected {
    pub fn white() -> Self { Self(vec![255; W * H * 3]) }
    fn black(&mut self, x: usize, y: usize) {
        self.0[(y * W + x) * 3..(y * W + x + 1) * 3].fill(0);
    }
    pub fn horizontal(&mut self, x0: usize, x1: usize, y: usize) {
        for x in x0..=x1 { self.black(x, y); }
    }
    pub fn vertical(&mut self, x: usize, y0: usize, y1: usize) {
        for y in y0..=y1 { self.black(x, y); }
    }
    pub fn rectangle(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        self.horizontal(x0, x1, y0); self.horizontal(x0, x1, y1);
        self.vertical(x0, y0, y1); self.vertical(x1, y0, y1);
    }
    // Midpoint circle of radius 5: the first octant is (5,0), (5,1), (5,2), (4,3).
    pub fn circle5(&mut self, cx: usize, cy: usize) {
        for (x, y) in [(5, 0), (5, 1), (5, 2), (4, 3)] {
            for (dx, dy) in [(x,y), (y,x), (-x,y), (-y,x), (x,-y), (y,-x), (-x,-y), (-y,-x)] {
                self.black((cx as i32 + dx) as usize, (cy as i32 + dy) as usize);
            }
        }
    }
    pub fn interior(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        for y in y0 + 1..y1 { self.horizontal(x0 + 1, x1 - 1, y); }
    }
    pub fn digest(&self) -> String {
        libbreenix::crypto::sha256::sha256(&self.0).iter().map(|b| format!("{b:02x}")).collect()
    }
}
