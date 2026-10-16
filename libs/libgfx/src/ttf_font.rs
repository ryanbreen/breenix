//! TrueType text on a FrameBuf through libfont (docs/gui/breengel.md section 8.3).
//!
//! Layout is one codepoint at a time in 26.6 fixed point: the pen starts at 0, glyph i is
//! drawn with its origin at `x + floor((pen_i + 32) / 64)`, and
//! `pen_{i+1} = pen_i + advance26(g_i) + kern26(g_i, g_{i+1})`. A codepoint the font does not
//! map draws glyph 0 (`.notdef`). Each glyph pixel is the coverage blend of the foreground
//! over the pixel already there, and each drawn glyph marks its bitmap rectangle.

use libfont::{CachedFont, Font, GlyphBitmap, SubpixelBitmap};

use crate::alpha::{blend, div255};
use crate::color::Color;
use crate::framebuf::{Box64, FrameBuf};
use crate::work;

/// U+FFFD REPLACEMENT CHARACTER.
pub const REPLACEMENT: u32 = 0xFFFD;

/// Decode UTF-8, giving one U+FFFD for each maximal invalid subpart (Unicode §3.9,
/// "U+FFFD Substitution of Maximal Subparts", over the well-formed sequences of its
/// Table 3-7).
pub fn decode_utf8(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let mut i = 0;
    core::iter::from_fn(move || {
        if i >= bytes.len() {
            return None;
        }
        work::add(1);
        let b0 = bytes[i];
        // (sequence length, the range of the second byte) for each valid lead byte.
        let (len, lo, hi) = match b0 {
            0x00..=0x7F => {
                i += 1;
                return Some(b0 as u32);
            }
            0xC2..=0xDF => (2, 0x80, 0xBF),
            0xE0 => (3, 0xA0, 0xBF),
            0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
            0xED => (3, 0x80, 0x9F),
            0xF0 => (4, 0x90, 0xBF),
            0xF1..=0xF3 => (4, 0x80, 0xBF),
            0xF4 => (4, 0x80, 0x8F),
            _ => {
                i += 1;
                return Some(REPLACEMENT);
            }
        };
        let mut cp = (b0 as u32) & (0x7F >> len);
        for k in 1..len {
            let (lo, hi) = if k == 1 { (lo, hi) } else { (0x80, 0xBF) };
            match bytes.get(i + k) {
                Some(&b) if (lo..=hi).contains(&b) => cp = (cp << 6) | (b & 0x3F) as u32,
                _ => {
                    // The maximal subpart is the lead and the k − 1 bytes that fit.
                    i += k;
                    return Some(REPLACEMENT);
                }
            }
        }
        i += len;
        Some(cp)
    })
}

/// Blend a coverage bitmap at (x, y) + its offsets, inside the surface and clip.
fn blit_coverage(fb: &mut FrameBuf, bitmap: &GlyphBitmap, x: i64, y: i64, fg: Color) {
    let (bx, by) = (x + bitmap.x_offset as i64, y + bitmap.y_offset as i64);
    let rect = Box64 { x0: bx, y0: by, x1: bx + bitmap.width as i64, y1: by + bitmap.height as i64 };
    let v = rect.intersect(&fb.clip_box());
    if !v.is_empty() {
        work::add(v.height() as u64 * (1 + v.width() as u64));
        for py in v.y0..v.y1 {
            let row = ((py - by) as usize) * bitmap.width;
            for px in v.x0..v.x1 {
                let a = bitmap.coverage[row + (px - bx) as usize];
                if a == 0 {
                    continue;
                }
                let c = if a == 255 { fg } else { blend(fg, fb.get_pixel(px as usize, py as usize), a) };
                let p = fb.opaque(c);
                fb.write(px as usize, py as usize, p);
            }
        }
    }
    fb.mark(rect);
}

/// Blend an LCD bitmap at (x, y): each channel by its own coverage.
fn blit_subpixel(fb: &mut FrameBuf, bitmap: &SubpixelBitmap, x: i64, y: i64, fg: Color) {
    let (bx, by) = (x + bitmap.x_offset as i64, y + bitmap.y_offset as i64);
    let rect = Box64 { x0: bx, y0: by, x1: bx + bitmap.width as i64, y1: by + bitmap.height as i64 };
    let v = rect.intersect(&fb.clip_box());
    if !v.is_empty() {
        work::add(v.height() as u64 * (1 + v.width() as u64));
        let ch = |f: u8, b: u8, a: u8| div255(f as u32 * a as u32 + b as u32 * (255 - a as u32));
        for py in v.y0..v.y1 {
            let row = ((py - by) as usize) * bitmap.width * 3;
            for px in v.x0..v.x1 {
                let o = row + (px - bx) as usize * 3;
                let (r, g, b) = (bitmap.coverage[o], bitmap.coverage[o + 1], bitmap.coverage[o + 2]);
                if r == 0 && g == 0 && b == 0 {
                    continue;
                }
                let bg = fb.get_pixel(px as usize, py as usize);
                let p = fb.opaque(Color::rgb(ch(fg.r, bg.r, r), ch(fg.g, bg.g, g), ch(fg.b, bg.b, b)));
                fb.write(px as usize, py as usize, p);
            }
        }
    }
    fb.mark(rect);
}

/// `floor((pen + 32) / 64)`: a 26.6 pen position rounded once to whole pixels.
fn pixels(pen: i64) -> i64 {
    (pen + 32).div_euclid(64)
}

/// Lay out `glyphs` from pen 0 at `size`, calling `draw(font, glyph, x)` with each glyph's
/// pixel origin offset; returns the width, `floor((pen_end + 32) / 64)`. A size outside
/// 1 to 1,024 lays out nothing.
fn layout(font: &mut CachedFont, glyphs: impl Iterator<Item = u16>, size: f32,
    mut draw: impl FnMut(&mut CachedFont, u16, i64)) -> i32 {
    let Ok(size26) = Font::size26(size) else { return 0 };
    let (mut pen, mut prev) = (0i64, None);
    for g in glyphs {
        work::add(1);
        if let Some(p) = prev {
            pen += font.font().kern26(p, g, size26) as i64;
        }
        draw(font, g, pixels(pen));
        pen += font.font().advance26(g, size26) as i64;
        prev = Some(g);
    }
    pixels(pen) as i32
}

fn glyphs_of<'a>(font: &'a CachedFont, cps: impl Iterator<Item = u32> + 'a) -> impl Iterator<Item = u16> + 'a {
    cps.map(move |cp| font.font().glyph_for(cp))
}

fn draw_glyphs(fb: &mut FrameBuf, font: &mut CachedFont, glyphs: alloc::vec::Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    layout(font, glyphs.into_iter(), size, |font, g, dx| {
        if let Ok(bitmap) = font.rasterize_glyph(g, size) {
            blit_coverage(fb, bitmap, x as i64 + dx, y as i64, fg);
        }
    })
}

/// Draw one character (glyph 0 when unmapped); returns its advance in whole pixels.
pub fn draw_char(fb: &mut FrameBuf, font: &mut CachedFont, ch: char, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let g = font.glyph_index(ch);
    draw_glyphs(fb, font, alloc::vec![g], x, y, size, fg)
}

/// Draw `text` with its pen starting at (x, y); returns the width section 8.3 gives.
pub fn draw_text(fb: &mut FrameBuf, font: &mut CachedFont, text: &str, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    draw_glyphs(fb, font, glyphs, x, y, size, fg)
}

/// Draw UTF-8 `bytes`, one U+FFFD for each maximal invalid subpart; never panics.
pub fn draw_text_bytes(fb: &mut FrameBuf, font: &mut CachedFont, bytes: &[u8], x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, decode_utf8(bytes)).collect();
    draw_glyphs(fb, font, glyphs, x, y, size, fg)
}

/// The width `draw_text` returns, without drawing.
pub fn text_width(font: &mut CachedFont, text: &str, size: f32) -> i32 {
    let glyphs: alloc::vec::Vec<u16> = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

/// The width `draw_text_bytes` returns, without drawing.
pub fn text_width_bytes(font: &mut CachedFont, bytes: &[u8], size: f32) -> i32 {
    let glyphs: alloc::vec::Vec<u16> = glyphs_of(font, decode_utf8(bytes)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

fn draw_glyphs_subpixel(fb: &mut FrameBuf, font: &mut CachedFont, glyphs: alloc::vec::Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    layout(font, glyphs.into_iter(), size, |font, g, dx| {
        if let Ok(bitmap) = font.font().rasterize_glyph_subpixel(g, size) {
            blit_subpixel(fb, &bitmap, x as i64 + dx, y as i64, fg);
        }
    })
}

/// Draw one character with LCD subpixel rendering; returns its advance in whole pixels.
pub fn draw_char_subpixel(fb: &mut FrameBuf, font: &mut CachedFont, ch: char, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let g = font.glyph_index(ch);
    draw_glyphs_subpixel(fb, font, alloc::vec![g], x, y, size, fg)
}

/// Draw `text` with LCD subpixel rendering; returns its width.
pub fn draw_text_subpixel(fb: &mut FrameBuf, font: &mut CachedFont, text: &str, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    draw_glyphs_subpixel(fb, font, glyphs, x, y, size, fg)
}
