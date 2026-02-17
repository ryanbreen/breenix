use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;

use crate::rect::Rect;
use crate::theme::Theme;

/// Measure the pixel width of a text string using the theme's font.
pub fn text_width(text: &[u8], theme: &Theme) -> i32 {
    if theme.use_bitmap_font {
        bitmap_font::text_width(text) as i32
    } else {
        font::text_width(text, 1) as i32
    }
}

/// Return the line height of the theme's font.
pub fn text_height(theme: &Theme) -> i32 {
    if theme.use_bitmap_font {
        bitmap_font::metrics().char_height as i32
    } else {
        7 // 5x7 bitmap font glyph height at scale=1
    }
}

/// Draw text at the given position using the theme's font.
pub fn draw_text(fb: &mut FrameBuf, text: &[u8], x: i32, y: i32, color: Color, theme: &Theme) {
    if x < 0 || y < 0 {
        return;
    }
    if theme.use_bitmap_font {
        bitmap_font::draw_text(fb, text, x as usize, y as usize, color);
    } else {
        font::draw_text(fb, text, x as usize, y as usize, color, 1);
    }
}

/// Draw text centered within a rectangle using the theme's font.
pub fn draw_text_centered(
    fb: &mut FrameBuf,
    text: &[u8],
    rect: &Rect,
    color: Color,
    theme: &Theme,
) {
    let tw = text_width(text, theme);
    let th = text_height(theme);
    let x = rect.x + (rect.w - tw) / 2;
    let y = rect.y + (rect.h - th) / 2;
    draw_text(fb, text, x, y, color, theme);
}
