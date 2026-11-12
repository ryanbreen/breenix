use libvt::TermEmu;

#[derive(Clone, Copy)]
pub struct Geometry { pub x: i32, pub y: i32, pub w: usize, pub h: usize }

#[derive(Default)]
pub struct Selection {
    start: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
    press_pixel: Option<(i32, i32)>,
    moved: bool,
}

impl Selection {
    fn cell(x: i32, y: i32, g: Geometry, size: (usize, usize)) -> (usize, usize) {
        let col = (i64::from(x) - i64::from(g.x)).max(0) as usize / g.w.max(1);
        let row = (i64::from(y) - i64::from(g.y)).max(0) as usize / g.h.max(1);
        (col.min(size.0.saturating_sub(1)), row.min(size.1.saturating_sub(1)))
    }
    pub fn clear(&mut self) { *self = Self::default(); }
    pub fn press(&mut self, x: i32, y: i32, g: Geometry, size: (usize, usize)) {
        self.clear();
        self.start = Some(Self::cell(x, y, g, size));
        self.press_pixel = Some((x, y));
    }
    pub fn update(&mut self, x: i32, y: i32, g: Geometry, size: (usize, usize)) {
        if let Some(origin) = self.press_pixel {
            self.moved |= origin != (x, y);
            if self.moved { self.end = Some(Self::cell(x, y, g, size)); }
        }
    }
    pub fn release(&mut self, x: i32, y: i32, g: Geometry, size: (usize, usize)) {
        if self.press_pixel.is_none() { return; }
        self.update(x, y, g, size);
        self.press_pixel = None;
        if !self.moved { self.clear(); }
    }
    pub fn bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        let (a, b) = (self.start?, self.end?);
        Some(if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) })
    }
    pub fn contains(&self, x: usize, y: usize) -> bool {
        self.bounds().is_some_and(|(a, b)| (a.1, a.0) <= (y, x) && (y, x) <= (b.1, b.0))
    }
    pub fn text(&self, vt: &TermEmu) -> Option<String> {
        let (a, b) = self.bounds()?;
        let (cols, rows) = vt.size();
        if b.1 >= rows || b.0 >= cols { return None; }
        let mut lines = Vec::new();
        for y in a.1..=b.1 {
            let start = if y == a.1 { a.0 } else { 0 };
            let end = if y == b.1 { b.0 + 1 } else { cols };
            let line: String = (start..end).map(|x| vt.view_cell(x, y).ch).collect();
            lines.push(line.trim_end_matches(' ').to_owned());
        }
        Some(lines.join("\n"))
    }
}
