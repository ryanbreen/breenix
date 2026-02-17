/// Axis-aligned rectangle for layout and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Returns true if the point (px, py) is inside this rectangle.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Returns a new rectangle shrunk by `padding` on all sides.
    pub fn inset(&self, padding: i32) -> Self {
        Self {
            x: self.x + padding,
            y: self.y + padding,
            w: self.w - padding * 2,
            h: self.h - padding * 2,
        }
    }

    /// Split off the top `size` pixels, returning (top_piece, remainder).
    pub fn split_top(&self, size: i32) -> (Self, Self) {
        (
            Self { x: self.x, y: self.y, w: self.w, h: size },
            Self { x: self.x, y: self.y + size, w: self.w, h: self.h - size },
        )
    }

    /// Split off the bottom `size` pixels, returning (bottom_piece, remainder).
    pub fn split_bottom(&self, size: i32) -> (Self, Self) {
        (
            Self { x: self.x, y: self.y + self.h - size, w: self.w, h: size },
            Self { x: self.x, y: self.y, w: self.w, h: self.h - size },
        )
    }

    /// Split off the left `size` pixels, returning (left_piece, remainder).
    pub fn split_left(&self, size: i32) -> (Self, Self) {
        (
            Self { x: self.x, y: self.y, w: size, h: self.h },
            Self { x: self.x + size, y: self.y, w: self.w - size, h: self.h },
        )
    }

    /// Split off the right `size` pixels, returning (right_piece, remainder).
    pub fn split_right(&self, size: i32) -> (Self, Self) {
        (
            Self { x: self.x + self.w - size, y: self.y, w: size, h: self.h },
            Self { x: self.x, y: self.y, w: self.w - size, h: self.h },
        )
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn center_x(&self) -> i32 {
        self.x + self.w / 2
    }

    pub fn center_y(&self) -> i32 {
        self.y + self.h / 2
    }
}
