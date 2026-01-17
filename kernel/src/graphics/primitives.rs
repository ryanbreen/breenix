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

    /// Create a Color from pixel bytes based on pixel format (BGR or RGB).
    pub fn from_pixel_bytes(bytes: &[u8], bytes_per_pixel: usize, is_bgr: bool) -> Self {
        if bytes_per_pixel == 0 || bytes.is_empty() {
            return Color::BLACK;
        }

        let (r, g, b) = if is_bgr {
            let b = bytes[0];
            let g = if bytes_per_pixel > 1 && bytes.len() > 1 { bytes[1] } else { 0 };
            let r = if bytes_per_pixel > 2 && bytes.len() > 2 { bytes[2] } else { 0 };
            (r, g, b)
        } else {
            let r = bytes[0];
            let g = if bytes_per_pixel > 1 && bytes.len() > 1 { bytes[1] } else { 0 };
            let b = if bytes_per_pixel > 2 && bytes.len() > 2 { bytes[2] } else { 0 };
            (r, g, b)
        };

        Color::rgb(r, g, b)
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

    /// Get a single pixel color (must handle bounds checking).
    /// Returns None if coordinates are out of bounds.
    fn get_pixel(&self, x: i32, y: i32) -> Option<Color>;

    /// Get buffer for direct access (optional optimization).
    fn buffer_mut(&mut self) -> &mut [u8];

    /// Get buffer for read access.
    fn buffer(&self) -> &[u8];
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
    let stride_bytes = stride * bpp; // stride is in pixels, convert to bytes
    let is_bgr = canvas.is_bgr();
    let pixel = color.to_pixel_bytes(bpp, is_bgr);
    let buffer = canvas.buffer_mut();

    let row_start = (y as usize).saturating_mul(stride_bytes);
    if row_start >= buffer.len() {
        return;
    }
    let row_end = min(row_start + stride_bytes, buffer.len());
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
    let stride_bytes = stride * bpp; // stride is in pixels, convert to bytes
    let is_bgr = canvas.is_bgr();
    let pixel = color.to_pixel_bytes(bpp, is_bgr);
    let buffer = canvas.buffer_mut();

    let start_y = clipped.y as usize;
    let end_y = start_y.saturating_add(clipped.height as usize);
    let start_x = clipped.x as usize;
    let end_x = start_x.saturating_add(clipped.width as usize);

    for y in start_y..end_y {
        let row_start = y.saturating_mul(stride_bytes);
        if row_start >= buffer.len() {
            break;
        }
        let row_end = min(row_start + stride_bytes, buffer.len());
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

// ============================================================================
// Text Rendering
// ============================================================================

use super::font::{Font, Glyph};

/// Text rendering style configuration.
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    /// Foreground text color
    pub foreground: Color,
    /// Background color (None for transparent background)
    pub background: Option<Color>,
    /// Font to use for rendering
    pub font: Font,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            foreground: Color::WHITE,
            background: None,
            font: Font::default_font(),
        }
    }
}

impl TextStyle {
    /// Create a new text style with default settings (white on transparent).
    pub const fn new() -> Self {
        Self {
            foreground: Color::WHITE,
            background: None,
            font: Font::default_font(),
        }
    }

    /// Set the foreground color.
    pub const fn with_color(mut self, color: Color) -> Self {
        self.foreground = color;
        self
    }

    /// Set the background color.
    pub const fn with_background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set the font.
    pub const fn with_font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }
}

/// Blend two colors based on an intensity value (0-255).
/// intensity=0 returns bg, intensity=255 returns fg.
fn blend_colors(fg: Color, bg: Color, intensity: u8) -> Color {
    if intensity == 0 {
        return bg;
    }
    if intensity == 255 {
        return fg;
    }
    let alpha = intensity as u16;
    let inv_alpha = 255 - alpha;
    Color::rgb(
        ((fg.r as u16 * alpha + bg.r as u16 * inv_alpha) / 255) as u8,
        ((fg.g as u16 * alpha + bg.g as u16 * inv_alpha) / 255) as u8,
        ((fg.b as u16 * alpha + bg.b as u16 * inv_alpha) / 255) as u8,
    )
}

/// Draw a single character at the specified position.
///
/// Returns the width in pixels that the cursor should advance.
pub fn draw_char(canvas: &mut impl Canvas, x: i32, y: i32, c: char, style: &TextStyle) -> i32 {
    let glyph = style.font.glyph_or_replacement(c);
    let metrics = style.font.metrics();

    // If background is set, fill the character cell first
    if let Some(bg) = style.background {
        fill_rect(
            canvas,
            Rect {
                x,
                y,
                width: metrics.char_advance() as u32,
                height: metrics.char_height as u32,
            },
            bg,
        );
    }

    // Draw the glyph pixels
    draw_glyph(canvas, x, y, &glyph, style);

    metrics.char_advance() as i32
}

/// Draw a glyph at the specified position with the given style.
fn draw_glyph(canvas: &mut impl Canvas, x: i32, y: i32, glyph: &Glyph, style: &TextStyle) {
    for (gx, gy, intensity) in glyph.pixels() {
        if intensity == 0 {
            continue;
        }

        let px = x + gx as i32;
        let py = y + gy as i32;

        let color = if let Some(bg) = style.background {
            // Explicit background - blend foreground with specified background
            blend_colors(style.foreground, bg, intensity)
        } else {
            // No explicit background - blend with actual canvas pixel for proper anti-aliasing
            if let Some(existing) = canvas.get_pixel(px, py) {
                blend_colors(style.foreground, existing, intensity)
            } else {
                // Out of bounds, skip
                continue;
            }
        };

        canvas.set_pixel(px, py, color);
    }
}

/// Draw a text string at the specified position.
///
/// Handles newlines by moving to the next line.
/// Returns the final cursor position (x, y) after drawing.
pub fn draw_text(
    canvas: &mut impl Canvas,
    x: i32,
    y: i32,
    text: &str,
    style: &TextStyle,
) -> (i32, i32) {
    let metrics = style.font.metrics();
    let mut cx = x;
    let mut cy = y;

    for c in text.chars() {
        if c == '\n' {
            cx = x;
            cy += metrics.line_height() as i32;
            continue;
        }

        // Skip carriage return
        if c == '\r' {
            continue;
        }

        // Skip tab (or treat as spaces)
        if c == '\t' {
            cx += (metrics.char_advance() * 4) as i32;
            continue;
        }

        let advance = draw_char(canvas, cx, cy, c, style);
        cx += advance;
    }

    (cx, cy)
}

/// Measure the width of a string without drawing it.
pub fn text_width(text: &str, style: &TextStyle) -> i32 {
    let metrics = style.font.metrics();
    let mut max_width = 0i32;
    let mut current_width = 0i32;

    for c in text.chars() {
        if c == '\n' {
            max_width = max_width.max(current_width);
            current_width = 0;
            continue;
        }
        if c == '\r' {
            continue;
        }
        if c == '\t' {
            current_width += (metrics.char_advance() * 4) as i32;
            continue;
        }
        current_width += metrics.char_advance() as i32;
    }

    max_width.max(current_width)
}

/// Get the line height for a given style.
pub fn text_line_height(style: &TextStyle) -> i32 {
    style.font.metrics().line_height() as i32
}

/// Get the height needed for multi-line text.
pub fn text_height(text: &str, style: &TextStyle) -> i32 {
    let metrics = style.font.metrics();
    let line_count = text.lines().count().max(1);
    (line_count * metrics.line_height()) as i32
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

        fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
            if x < 0 || y < 0 {
                return None;
            }
            let x = x as usize;
            let y = y as usize;
            if x >= self.width || y >= self.height {
                return None;
            }
            let offset = y * self.stride + x * self.bpp;
            if offset + self.bpp > self.buffer.len() {
                return None;
            }
            Some(Color::from_pixel_bytes(&self.buffer[offset..offset + self.bpp], self.bpp, self.is_bgr))
        }

        fn buffer_mut(&mut self) -> &mut [u8] {
            &mut self.buffer
        }

        fn buffer(&self) -> &[u8] {
            &self.buffer
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

    // ========================================================================
    // Text Rendering Tests
    // ========================================================================

    #[test]
    fn text_style_default_values() {
        let style = TextStyle::default();
        assert_eq!(style.foreground, Color::WHITE);
        assert!(style.background.is_none());
    }

    #[test]
    fn text_style_builder_methods() {
        let style = TextStyle::new()
            .with_color(Color::RED)
            .with_background(Color::BLUE);
        assert_eq!(style.foreground, Color::RED);
        assert_eq!(style.background, Some(Color::BLUE));
    }

    #[test]
    fn blend_colors_extremes() {
        let fg = Color::WHITE;
        let bg = Color::BLACK;

        // intensity=0 should return background
        let result = blend_colors(fg, bg, 0);
        assert_eq!(result, bg);

        // intensity=255 should return foreground
        let result = blend_colors(fg, bg, 255);
        assert_eq!(result, fg);
    }

    #[test]
    fn blend_colors_midpoint() {
        let fg = Color::WHITE;
        let bg = Color::BLACK;
        let result = blend_colors(fg, bg, 128);
        // Should be approximately half (128/255 * 255 ≈ 128)
        assert!(result.r > 100 && result.r < 140);
        assert!(result.g > 100 && result.g < 140);
        assert!(result.b > 100 && result.b < 140);
    }

    #[test]
    fn text_width_single_line() {
        let style = TextStyle::default();
        let width = text_width("ABC", &style);
        let metrics = style.font.metrics();
        assert_eq!(width, (3 * metrics.char_advance()) as i32);
    }

    #[test]
    fn text_width_empty_string() {
        let style = TextStyle::default();
        assert_eq!(text_width("", &style), 0);
    }

    #[test]
    fn text_width_multiline_returns_max() {
        let style = TextStyle::default();
        let metrics = style.font.metrics();
        // "ABCD" is wider than "AB"
        let width = text_width("AB\nABCD", &style);
        assert_eq!(width, (4 * metrics.char_advance()) as i32);
    }

    #[test]
    fn text_height_single_line() {
        let style = TextStyle::default();
        let height = text_height("Hello", &style);
        let metrics = style.font.metrics();
        assert_eq!(height, metrics.line_height() as i32);
    }

    #[test]
    fn text_height_multiline() {
        let style = TextStyle::default();
        let height = text_height("Line1\nLine2\nLine3", &style);
        let metrics = style.font.metrics();
        assert_eq!(height, (3 * metrics.line_height()) as i32);
    }

    #[test]
    fn text_line_height_matches_metrics() {
        let style = TextStyle::default();
        let metrics = style.font.metrics();
        assert_eq!(text_line_height(&style), metrics.line_height() as i32);
    }

    #[test]
    fn draw_char_modifies_canvas() {
        let mut canvas = TestCanvas::new(100, 100, 4, false);
        let style = TextStyle::new().with_color(Color::WHITE);

        // Canvas should start all black
        assert!(canvas.buffer.iter().all(|&b| b == 0));

        // Draw a character
        let advance = draw_char(&mut canvas, 10, 10, 'X', &style);

        // Advance should be positive
        assert!(advance > 0);

        // Some pixels should have been modified (non-zero)
        assert!(canvas.buffer.iter().any(|&b| b != 0));
    }

    #[test]
    fn draw_text_positions_correctly() {
        let mut canvas = TestCanvas::new(200, 100, 4, false);
        let style = TextStyle::new().with_color(Color::WHITE);
        let metrics = style.font.metrics();

        let (final_x, final_y) = draw_text(&mut canvas, 0, 0, "AB", &style);

        // Final X should be 2 character widths
        assert_eq!(final_x, (2 * metrics.char_advance()) as i32);
        // Final Y should be unchanged (no newlines)
        assert_eq!(final_y, 0);
    }

    #[test]
    fn draw_text_handles_newlines() {
        let mut canvas = TestCanvas::new(200, 100, 4, false);
        let style = TextStyle::new().with_color(Color::WHITE);
        let metrics = style.font.metrics();

        let (final_x, final_y) = draw_text(&mut canvas, 10, 10, "A\nB", &style);

        // After newline, X resets to starting X
        assert_eq!(final_x, 10 + metrics.char_advance() as i32);
        // Y advances by one line
        assert_eq!(final_y, 10 + metrics.line_height() as i32);
    }

    #[test]
    fn draw_text_with_background_fills_cells() {
        let mut canvas = TestCanvas::new(100, 100, 4, false);
        let style = TextStyle::new()
            .with_color(Color::WHITE)
            .with_background(Color::BLUE);

        draw_text(&mut canvas, 0, 0, "A", &style);

        // Check that at least some pixels have blue component
        // (background should have been drawn)
        let has_blue = canvas.buffer.chunks(4).any(|pixel| pixel[2] > 0);
        assert!(has_blue);
    }

    #[test]
    fn draw_char_returns_advance_width() {
        let mut canvas = TestCanvas::new(100, 100, 4, false);
        let style = TextStyle::default();
        let metrics = style.font.metrics();

        let advance = draw_char(&mut canvas, 0, 0, 'M', &style);
        assert_eq!(advance, metrics.char_advance() as i32);
    }
}
