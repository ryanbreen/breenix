#![no_std]
//! Original geometric Breenix splash art. Coordinates are design units; a pixel
//! uses sixteen sample centres at (1, 3, 5, 7)/8 along both axes. Covered samples
//! take the topmost shape's colour. Channels resolve as (sum + 8) / 16.

pub mod geometry;
pub mod oracle;
mod raster;

pub use raster::render;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

pub const BACKGROUND: Color = Color(15, 20, 35);
pub const INDIGO: Color = Color(52, 92, 190);
pub const TEAL: Color = Color(34, 150, 160);
pub const WHITE: Color = Color(236, 241, 250);
pub const WORDMARK: Color = Color(205, 215, 235);
pub const GREEN: Color = Color(70, 200, 120);
pub const AMBER: Color = Color(230, 170, 65);
pub const TRACK: Color = Color(40, 50, 75);
pub const WIDTH: u32 = 176;
pub const HEIGHT: u32 = 136;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Milestone {
    Pending,
    Running,
    Done,
}

/// Stage text and boot-mode text are drawn by the console's text renderer.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub cursor_visible: bool,
    pub completed: u16,
    pub total: u16,
    pub milestones: [Milestone; 6],
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            cursor_visible: true,
            completed: 0,
            total: 0,
            milestones: [Milestone::Pending; 6],
        }
    }
}

/// One cycle per second while booting; hold the cursor lit after hand-over.
pub const fn cursor_visible(elapsed_ms: u64, booting: bool) -> bool {
    !booting || elapsed_ms % 1000 < 500
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    Rgba,
    Bgra,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb | Self::Bgr => 3,
            Self::Rgba | Self::Bgra => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ZeroScale,
    InvalidSurface,
}

/// A checked caller-owned framebuffer. Stride is in bytes. Four-byte formats
/// write an opaque alpha byte; padding and pixels outside the art are untouched.
pub struct Surface<'a> {
    pub(crate) pixels: &'a mut [u8],
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    pub(crate) format: PixelFormat,
}

impl<'a> Surface<'a> {
    pub fn new(
        pixels: &'a mut [u8],
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
    ) -> Result<Self, Error> {
        let row = width
            .checked_mul(format.bytes_per_pixel())
            .ok_or(Error::InvalidSurface)?;
        let required = if height == 0 || width == 0 {
            0
        } else {
            (height - 1)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(row))
                .ok_or(Error::InvalidSurface)?
        };
        if stride < row || required > pixels.len() {
            return Err(Error::InvalidSurface);
        }
        Ok(Self {
            pixels,
            width,
            height,
            stride,
            format,
        })
    }
}

/// The badge gradient spans the first through last pixel rows, including the
/// endpoint colours. Both drawing and the oracle use this colour specification.
pub fn badge_color(row: i32, scale: u16) -> Color {
    let denominator = (64 * i64::from(scale) - 1).max(1);
    let t = i64::from(row).clamp(0, denominator);
    let lerp = |a, b| {
        ((i64::from(a) * (denominator - t) + i64::from(b) * t + denominator / 2) / denominator)
            as u8
    };
    Color(
        lerp(INDIGO.0, TEAL.0),
        lerp(INDIGO.1, TEAL.1),
        lerp(INDIGO.2, TEAL.2),
    )
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
