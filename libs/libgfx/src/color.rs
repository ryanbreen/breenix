//! Color types and constants.

/// An RGB color value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const RED: Self = Self::rgb(255, 50, 50);
    pub const GREEN: Self = Self::rgb(50, 255, 50);
    pub const BLUE: Self = Self::rgb(50, 50, 255);
    pub const YELLOW: Self = Self::rgb(255, 255, 50);
    pub const MAGENTA: Self = Self::rgb(255, 50, 255);
    pub const CYAN: Self = Self::rgb(50, 255, 255);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const ORANGE: Self = Self::rgb(255, 150, 50);
    pub const PURPLE: Self = Self::rgb(150, 50, 255);
    pub const GRAY: Self = Self::rgb(200, 200, 200);

    /// Blend this color (foreground) over another (background) using an 8-bit intensity.
    /// intensity=255 means fully foreground, intensity=0 means fully background.
    #[inline]
    pub fn blend(self, bg: Color, intensity: u8) -> Color {
        let a = intensity as u16;
        let inv = 255 - a;
        Color {
            r: ((self.r as u16 * a + bg.r as u16 * inv) / 255) as u8,
            g: ((self.g as u16 * a + bg.g as u16 * inv) / 255) as u8,
            b: ((self.b as u16 * a + bg.b as u16 * inv) / 255) as u8,
        }
    }
}
