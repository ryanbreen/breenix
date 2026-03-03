//! WasmCanvas — implements the breenix-core Canvas trait backed by an RGBA pixel buffer.

use breenix_core::graphics::primitives::{Canvas, Color};

/// A software canvas that stores RGBA pixels in a flat Vec<u8>.
/// Intended to be shared with JavaScript via ImageData.
pub struct WasmCanvas {
    width: usize,
    height: usize,
    /// 4 bytes per pixel: R, G, B, A
    buffer: Vec<u8>,
}

impl WasmCanvas {
    /// Create a new WasmCanvas with the given dimensions.
    /// All pixels are initialized to opaque black.
    pub fn new(width: usize, height: usize) -> Self {
        let len = width * height * 4;
        let mut buffer = vec![0u8; len];
        // Set alpha to 0xFF for every pixel
        for pixel in buffer.chunks_exact_mut(4) {
            pixel[3] = 0xFF;
        }
        Self { width, height, buffer }
    }

    /// Raw pointer to the pixel buffer (for zero-copy ImageData construction).
    pub fn buffer_ptr(&self) -> *const u8 {
        self.buffer.as_ptr()
    }

    /// Length of the pixel buffer in bytes.
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }
}

impl Canvas for WasmCanvas {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn bytes_per_pixel(&self) -> usize {
        4
    }

    fn stride(&self) -> usize {
        self.width
    }

    fn is_bgr(&self) -> bool {
        false // Browser ImageData is RGBA
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.width + x) * 4;
        if offset + 4 > self.buffer.len() {
            return;
        }
        self.buffer[offset] = color.r;
        self.buffer[offset + 1] = color.g;
        self.buffer[offset + 2] = color.b;
        self.buffer[offset + 3] = 0xFF;
    }

    fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 {
            return None;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y * self.width + x) * 4;
        if offset + 4 > self.buffer.len() {
            return None;
        }
        Some(Color::rgb(
            self.buffer[offset],
            self.buffer[offset + 1],
            self.buffer[offset + 2],
        ))
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }

    fn buffer(&self) -> &[u8] {
        &self.buffer
    }
}
