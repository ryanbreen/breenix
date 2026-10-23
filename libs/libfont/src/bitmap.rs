//! Readback values; no CPU drawing implementation.
use alloc::vec::Vec;
/// A rasterized glyph: one coverage byte per pixel, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}

/// An LCD glyph: three coverage bytes (R, G, B) per pixel, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubpixelBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}
