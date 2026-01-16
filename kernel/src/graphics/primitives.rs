//! 2D drawing primitives for framebuffer-style canvases.
//!
//! This module provides a public API for 2D graphics operations.
//! The functions are intended to be called by shell commands, applications,
//! or other kernel components that need graphics capabilities.

// This is a public API module - functions are intentionally available for external use
#![allow(dead_code)]

use core::cmp::{max, min};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);

    /// Convert to pixel bytes based on pixel format (BGR or RGB).
    pub fn to_pixel_bytes(&self, bytes_per_pixel: usize, is_bgr: bool) -> [u8; 4] {
        let mut out = [0u8; 4];
        if bytes_per_pixel == 0 {
            return out;
        }

        if is_bgr {
            out[0] = self.b;
            if bytes_per_pixel > 1 {
                out[1] = self.g;
            }
            if bytes_per_pixel > 2 {
                out[2] = self.r;
            }
        } else {
            out[0] = self.r;
            if bytes_per_pixel > 1 {
                out[1] = self.g;
            }
            if bytes_per_pixel > 2 {
                out[2] = self.b;
            }
        }

        if bytes_per_pixel > 3 {
            out[3] = 0xFF;
        }

        out
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub trait Canvas {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn bytes_per_pixel(&self) -> usize;
    fn stride(&self) -> usize;
    fn is_bgr(&self) -> bool;

    /// Set a single pixel (must handle bounds checking).
    fn set_pixel(&mut self, x: i32, y: i32, color: Color);

    /// Get buffer for direct access (optional optimization).
    fn buffer_mut(&mut self) -> &mut [u8];
}

fn to_i32_clamped(value: u32) -> i32 {
    if value > i32::MAX as u32 {
        i32::MAX
    } else {
        value as i32
    }
}

fn clip_rect_to_bounds(rect: Rect, width: usize, height: usize) -> Option<Rect> {
    let canvas_w = width as i32;
    let canvas_h = height as i32;
    if canvas_w <= 0 || canvas_h <= 0 {
        return None;
    }

    let rect_w = to_i32_clamped(rect.width);
    let rect_h = to_i32_clamped(rect.height);
    if rect_w <= 0 || rect_h <= 0 {
        return None;
    }

    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = x0.saturating_add(rect_w);
    let y1 = y0.saturating_add(rect_h);

    let cx0 = max(0, min(canvas_w, x0));
    let cy0 = max(0, min(canvas_h, y0));
    let cx1 = max(0, min(canvas_w, x1));
    let cy1 = max(0, min(canvas_h, y1));

    if cx0 >= cx1 || cy0 >= cy1 {
        return None;
    }

    Some(Rect {
        x: cx0,
        y: cy0,
        width: (cx1 - cx0) as u32,
        height: (cy1 - cy0) as u32,
    })
}

/// Draw a horizontal line (optimized - single memset per row).
pub fn draw_hline(canvas: &mut impl Canvas, x1: i32, x2: i32, y: i32, color: Color) {
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;
    let bpp = canvas.bytes_per_pixel();
    if width <= 0 || height <= 0 || bpp == 0 {
        return;
    }
    if y < 0 || y >= height {
        return;
    }

    let mut start = min(x1, x2);
    let mut end = max(x1, x2);
    if end < 0 || start >= width {
        return;
    }
    start = max(start, 0);
    end = min(end, width - 1);
    if start > end {
        return;
    }

    let stride = canvas.stride();
    let is_bgr = canvas.is_bgr();
    let pixel = color.to_pixel_bytes(bpp, is_bgr);
    let buffer = canvas.buffer_mut();

    let row_start = (y as usize).saturating_mul(stride);
    if row_start >= buffer.len() {
        return;
    }
    let row_end = min(row_start + stride, buffer.len());
    let row_slice = &mut buffer[row_start..row_end];

    let start_byte = (start as usize).saturating_mul(bpp);
    if start_byte >= row_slice.len() {
        return;
    }
    let end_byte = min((end as usize + 1).saturating_mul(bpp), row_slice.len());

    for chunk in row_slice[start_byte..end_byte].chunks_exact_mut(bpp) {
        chunk.copy_from_slice(&pixel[..bpp]);
    }
}

/// Draw a vertical line.
pub fn draw_vline(canvas: &mut impl Canvas, x: i32, y1: i32, y2: i32, color: Color) {
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;
    if width <= 0 || height <= 0 {
        return;
    }
    if x < 0 || x >= width {
        return;
    }

    let mut start = min(y1, y2);
    let mut end = max(y1, y2);
    if end < 0 || start >= height {
        return;
    }
    start = max(start, 0);
    end = min(end, height - 1);
    if start > end {
        return;
    }

    for y in start..=end {
        canvas.set_pixel(x, y, color);
    }
}

/// Draw a line using Bresenham's algorithm.
pub fn draw_line(canvas: &mut impl Canvas, x1: i32, y1: i32, x2: i32, y2: i32, color: Color) {
    let mut x0 = x1;
    let mut y0 = y1;
    let x1 = x2;
    let y1 = y2;

    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        canvas.set_pixel(x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Draw rectangle outline.
pub fn draw_rect(canvas: &mut impl Canvas, rect: Rect, color: Color) {
    let rect_w = to_i32_clamped(rect.width);
    let rect_h = to_i32_clamped(rect.height);
    if rect_w <= 0 || rect_h <= 0 {
        return;
    }

    let x2 = rect.x.saturating_add(rect_w - 1);
    let y2 = rect.y.saturating_add(rect_h - 1);

    draw_hline(canvas, rect.x, x2, rect.y, color);
    draw_hline(canvas, rect.x, x2, y2, color);
    draw_vline(canvas, rect.x, rect.y, y2, color);
    draw_vline(canvas, x2, rect.y, y2, color);
}

/// Draw filled rectangle (optimized - fill row by row).
pub fn fill_rect(canvas: &mut impl Canvas, rect: Rect, color: Color) {
    let Some(clipped) = clip_rect_to_bounds(rect, canvas.width(), canvas.height()) else {
        return;
    };

    let bpp = canvas.bytes_per_pixel();
    if bpp == 0 {
        return;
    }
    let stride = canvas.stride();
    let is_bgr = canvas.is_bgr();
    let pixel = color.to_pixel_bytes(bpp, is_bgr);
    let buffer = canvas.buffer_mut();

    let start_y = clipped.y as usize;
    let end_y = start_y.saturating_add(clipped.height as usize);
    let start_x = clipped.x as usize;
    let end_x = start_x.saturating_add(clipped.width as usize);

    for y in start_y..end_y {
        let row_start = y.saturating_mul(stride);
        if row_start >= buffer.len() {
            break;
        }
        let row_end = min(row_start + stride, buffer.len());
        let row_slice = &mut buffer[row_start..row_end];

        let start_byte = start_x.saturating_mul(bpp);
        if start_byte >= row_slice.len() {
            continue;
        }
        let end_byte = min(end_x.saturating_mul(bpp), row_slice.len());

        for chunk in row_slice[start_byte..end_byte].chunks_exact_mut(bpp) {
            chunk.copy_from_slice(&pixel[..bpp]);
        }
    }
}

/// Draw circle outline using midpoint circle algorithm.
pub fn draw_circle(canvas: &mut impl Canvas, cx: i32, cy: i32, radius: u32, color: Color) {
    let mut x = to_i32_clamped(radius);
    if x <= 0 {
        canvas.set_pixel(cx, cy, color);
        return;
    }
    let mut y = 0;
    let mut err = 1 - x;

    while x >= y {
        canvas.set_pixel(cx + x, cy + y, color);
        canvas.set_pixel(cx - x, cy + y, color);
        canvas.set_pixel(cx + x, cy - y, color);
        canvas.set_pixel(cx - x, cy - y, color);
        canvas.set_pixel(cx + y, cy + x, color);
        canvas.set_pixel(cx - y, cy + x, color);
        canvas.set_pixel(cx + y, cy - x, color);
        canvas.set_pixel(cx - y, cy - x, color);

        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Draw filled circle.
pub fn fill_circle(canvas: &mut impl Canvas, cx: i32, cy: i32, radius: u32, color: Color) {
    let mut x = to_i32_clamped(radius);
    if x <= 0 {
        canvas.set_pixel(cx, cy, color);
        return;
    }
    let mut y = 0;
    let mut err = 1 - x;

    while x >= y {
        draw_hline(canvas, cx - x, cx + x, cy + y, color);
        draw_hline(canvas, cx - x, cx + x, cy - y, color);
        draw_hline(canvas, cx - y, cx + y, cy + x, color);
        draw_hline(canvas, cx - y, cx + y, cy - x, color);

        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    struct TestCanvas {
        width: usize,
        height: usize,
        bpp: usize,
        stride: usize,
        is_bgr: bool,
        buffer: Vec<u8>,
    }

    impl TestCanvas {
        fn new(width: usize, height: usize, bpp: usize, is_bgr: bool) -> Self {
            let stride = width.saturating_mul(bpp);
            let buffer_len = stride.saturating_mul(height);
            Self {
                width,
                height,
                bpp,
                stride,
                is_bgr,
                buffer: vec![0u8; buffer_len],
            }
        }

        fn pixel_at(&self, x: usize, y: usize) -> &[u8] {
            let offset = y * self.stride + x * self.bpp;
            &self.buffer[offset..offset + self.bpp]
        }
    }

    impl Canvas for TestCanvas {
        fn width(&self) -> usize {
            self.width
        }

        fn height(&self) -> usize {
            self.height
        }

        fn bytes_per_pixel(&self) -> usize {
            self.bpp
        }

        fn stride(&self) -> usize {
            self.stride
        }

        fn is_bgr(&self) -> bool {
            self.is_bgr
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
            let offset = y * self.stride + x * self.bpp;
            if offset + self.bpp > self.buffer.len() {
                return;
            }
            let pixel = color.to_pixel_bytes(self.bpp, self.is_bgr);
            self.buffer[offset..offset + self.bpp].copy_from_slice(&pixel[..self.bpp]);
        }

        fn buffer_mut(&mut self) -> &mut [u8] {
            &mut self.buffer
        }
    }

    #[test]
    fn color_to_pixel_bytes_rgb() {
        let color = Color::rgb(1, 2, 3);
        assert_eq!(color.to_pixel_bytes(3, false), [1, 2, 3, 0]);
        assert_eq!(color.to_pixel_bytes(4, false), [1, 2, 3, 0xFF]);
    }

    #[test]
    fn color_to_pixel_bytes_bgr() {
        let color = Color::rgb(1, 2, 3);
        assert_eq!(color.to_pixel_bytes(3, true), [3, 2, 1, 0]);
        assert_eq!(color.to_pixel_bytes(4, true), [3, 2, 1, 0xFF]);
    }

    #[test]
    fn bounds_checking_ignores_out_of_range_pixels() {
        let mut canvas = TestCanvas::new(2, 2, 3, false);
        draw_line(&mut canvas, -2, -2, -1, -1, Color::WHITE);
        assert!(canvas.buffer.iter().all(|byte| *byte == 0));

        draw_line(&mut canvas, -1, 0, 1, 0, Color::RED);
        assert_eq!(canvas.pixel_at(0, 0), &[255, 0, 0]);
        assert_eq!(canvas.pixel_at(1, 0), &[255, 0, 0]);
        assert!(canvas.pixel_at(0, 1).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn clip_rect_intersects_with_canvas_bounds() {
        let rect = Rect {
            x: -5,
            y: -5,
            width: 8,
            height: 8,
        };
        let clipped = clip_rect_to_bounds(rect, 10, 10).expect("rect should clip");
        assert_eq!(clipped.x, 0);
        assert_eq!(clipped.y, 0);
        assert_eq!(clipped.width, 3);
        assert_eq!(clipped.height, 3);
    }
}
