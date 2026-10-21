pub mod bmp;
pub mod crc;
pub mod deflate;
pub mod inflate;
pub mod png;
pub mod work;

/// Decoder-owned pixels. Consumers can inspect these bytes but cannot construct or
/// mutate a Decoded value from CPU-drawn pixels (gpu.md section 2.4).
pub struct Decoded { image: png::Image }
impl Decoded {
    pub fn png(bytes: &[u8]) -> Result<Self, png::PngError> { png::decode(bytes).map(|image| Self { image }) }
    pub fn width(&self) -> u32 { self.image.width }
    pub fn height(&self) -> u32 { self.image.height }
    pub fn pixels(&self) -> &[u8] { &self.image.pixels }
}
