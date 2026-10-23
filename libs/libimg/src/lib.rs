#![no_std]
#[macro_use]
extern crate alloc;

pub mod bmp;
pub mod crc;
pub mod inflate;
pub mod png;
pub mod work;

/// Pixels decoded by the image library, for the GPU Image upload boundary.
/// The encoded image is validated before this value can be constructed.
pub struct Decoded(png::Image);

impl Decoded {
    pub fn png(bytes: &[u8]) -> Result<Self, png::PngError> {
        png::decode(bytes).map(Self)
    }
    pub fn width(&self) -> u32 { self.0.width }
    pub fn height(&self) -> u32 { self.0.height }
    pub fn pixels(&self) -> &[u8] { &self.0.pixels }
}
