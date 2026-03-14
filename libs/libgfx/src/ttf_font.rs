//! TrueType font rendering to FrameBuf using libfont.
//!
//! Provides the same compositing pattern as `bitmap_font.rs` but with
//! runtime-loaded TrueType fonts at any pixel size.

use libfont::CachedFont;
use libfont::rasterizer::GlyphBitmap;
use libfont::SubpixelBitmap;
use crate::color::Color;
use crate::framebuf::FrameBuf;

/// Draw a single character with anti-aliased alpha blending.
///
/// Returns the advance width in pixels (how far to move x for the next char).
pub fn draw_char(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    ch: char,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let glyph_index = font.glyph_index(ch);
    if glyph_index == 0 && ch != '\0' {
        // Try '?' as fallback
        let fallback = font.glyph_index('?');
        if fallback != 0 {
            return draw_glyph(fb, font, fallback, x, y, size, fg);
        }
    }
    draw_glyph(fb, font, glyph_index, x, y, size, fg)
}

fn draw_glyph(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    glyph_index: u16,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let advance = font.advance_width(glyph_index, size);

    let bitmap = match font.rasterize_glyph(glyph_index, size) {
        Ok(bmp) => bmp,
        Err(_) => return advance as i32,
    };

    if bitmap.height == 0 || bitmap.width == 0 {
        return advance as i32;
    }

    blit_coverage(fb, bitmap, x, y, fg);
    advance as i32
}

fn blit_coverage(
    fb: &mut FrameBuf,
    bitmap: &GlyphBitmap,
    x: i32,
    y: i32,
    fg: Color,
) {
    let bx = x + bitmap.x_offset;
    let by = y + bitmap.y_offset;

    for row in 0..bitmap.height {
        let py = by + row as i32;
        if py < 0 || py >= fb.height as i32 {
            continue;
        }
        for col in 0..bitmap.width {
            let px = bx + col as i32;
            if px < 0 || px >= fb.width as i32 {
                continue;
            }
            let intensity = bitmap.coverage[row * bitmap.width + col];
            if intensity == 0 {
                continue;
            }
            let ux = px as usize;
            let uy = py as usize;
            if intensity == 255 {
                fb.put_pixel(ux, uy, fg);
            } else {
                let bg = fb.get_pixel(ux, uy);
                fb.put_pixel(ux, uy, fg.blend(bg, intensity));
            }
        }
    }
}

/// Draw a text string, returning the total advance width in pixels.
pub fn draw_text(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let mut cursor_x = x;
    let mut prev_glyph: Option<u16> = None;

    for ch in text.chars() {
        let glyph_index = font.glyph_index(ch);

        // Apply kerning
        if let Some(prev) = prev_glyph {
            let kern = font.kern(prev, glyph_index, size);
            cursor_x += kern as i32;
        }

        let advance = draw_char(fb, font, ch, cursor_x, y, size, fg);
        cursor_x += advance;
        prev_glyph = Some(glyph_index);
    }

    cursor_x - x
}

/// Draw a single character with LCD subpixel rendering.
///
/// Returns the advance width in pixels.
pub fn draw_char_subpixel(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    ch: char,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let glyph_index = font.glyph_index(ch);
    if glyph_index == 0 && ch != '\0' {
        let fallback = font.glyph_index('?');
        if fallback != 0 {
            return draw_glyph_subpixel(fb, font, fallback, x, y, size, fg);
        }
    }
    draw_glyph_subpixel(fb, font, glyph_index, x, y, size, fg)
}

fn draw_glyph_subpixel(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    glyph_index: u16,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let advance = font.advance_width(glyph_index, size);

    let bitmap = match font.font().rasterize_glyph_subpixel(glyph_index, size) {
        Ok(bmp) => bmp,
        Err(_) => return advance as i32,
    };

    if bitmap.height == 0 || bitmap.width == 0 {
        return advance as i32;
    }

    blit_subpixel(fb, &bitmap, x, y, fg);
    advance as i32
}

fn blit_subpixel(
    fb: &mut FrameBuf,
    bitmap: &SubpixelBitmap,
    x: i32,
    y: i32,
    fg: Color,
) {
    let bx = x + bitmap.x_offset;
    let by = y + bitmap.y_offset;

    for row in 0..bitmap.height {
        let py = by + row as i32;
        if py < 0 || py >= fb.height as i32 {
            continue;
        }
        for col in 0..bitmap.width {
            let px = bx + col as i32;
            if px < 0 || px >= fb.width as i32 {
                continue;
            }
            let base = (row * bitmap.width + col) * 3;
            let r_cov = bitmap.coverage[base];
            let g_cov = bitmap.coverage[base + 1];
            let b_cov = bitmap.coverage[base + 2];
            if r_cov == 0 && g_cov == 0 && b_cov == 0 {
                continue;
            }
            let ux = px as usize;
            let uy = py as usize;
            let bg = fb.get_pixel(ux, uy);
            // Per-channel alpha blending
            let r = blend_channel(fg.r(), bg.r(), r_cov);
            let g = blend_channel(fg.g(), bg.g(), g_cov);
            let b = blend_channel(fg.b(), bg.b(), b_cov);
            fb.put_pixel(ux, uy, Color::rgb(r, g, b));
        }
    }
}

#[inline]
fn blend_channel(fg: u8, bg: u8, alpha: u8) -> u8 {
    if alpha == 255 { return fg; }
    let fg = fg as u16;
    let bg = bg as u16;
    let a = alpha as u16;
    ((fg * a + bg * (255 - a) + 128) / 255) as u8
}

/// Draw a text string with LCD subpixel rendering.
pub fn draw_text_subpixel(
    fb: &mut FrameBuf,
    font: &mut CachedFont,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    fg: Color,
) -> i32 {
    let mut cursor_x = x;
    let mut prev_glyph: Option<u16> = None;

    for ch in text.chars() {
        let glyph_index = font.glyph_index(ch);
        if let Some(prev) = prev_glyph {
            let kern = font.kern(prev, glyph_index, size);
            cursor_x += kern as i32;
        }
        let advance = draw_char_subpixel(fb, font, ch, cursor_x, y, size, fg);
        cursor_x += advance;
        prev_glyph = Some(glyph_index);
    }

    cursor_x - x
}

/// Measure the pixel width of a text string without drawing.
pub fn text_width(font: &mut CachedFont, text: &str, size: f32) -> i32 {
    let mut width = 0i32;
    let mut prev_glyph: Option<u16> = None;

    for ch in text.chars() {
        let glyph_index = font.glyph_index(ch);

        if let Some(prev) = prev_glyph {
            let kern = font.kern(prev, glyph_index, size);
            width += kern as i32;
        }

        let advance = font.advance_width(glyph_index, size);
        width += advance as i32;
        prev_glyph = Some(glyph_index);
    }

    width
}
