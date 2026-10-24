//! Independent contract pixels, with certified bitmap glyphs and literal chrome geometry.
use super::kit::*;
use libgfx_ref::{
    bitmap_font as font,
    color::Color,
    framebuf::{Format, FrameBuf},
};

pub fn text(
    screen: &Screen,
    bytes: &[u8],
    x: i32,
    y: i32,
    fg: u32,
    bg: u32,
    scale: i32,
) -> CaseResult {
    let height = if scale == 2 {
        font::RasterHeight::Size32
    } else {
        font::RasterHeight::Size16
    };
    let m = font::metrics_at(height);
    let mut image = FrameBuf::new(bytes.len() * m.char_width, m.char_height, Format::BGRX)
        .map_err(|e| format!("reference text surface: {e:?}"))?;
    let color = |c: u32| Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    image.clear(color(bg));
    for (i, b) in bytes.iter().enumerate() {
        font::draw_char_at(
            &mut image,
            *b as char,
            i * m.char_width,
            0,
            color(fg),
            height,
        );
    }
    for py in 0..m.char_height {
        for px in 0..bytes.len() * m.char_width {
            let c = image.get_pixel(px, py);
            let expected = 0xff000000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            check(
                screen.pixel(x + px as i32, y + py as i32)? == expected,
                &format!(
                    "certified text differs at {},{}",
                    x + px as i32,
                    y + py as i32
                ),
            )?;
        }
    }
    Ok(())
}

pub fn background_region(s: &Screen, scale: i32, rect: (i32, i32, i32, i32)) -> CaseResult {
    for y in rect.1..rect.1 + rect.3 {
        for x in rect.0..rect.0 + rect.2 {
            check(
                s.pixel(x, y)? == background(x, y, s.height as i32, scale),
                &format!("desktop background differs at {x},{y}"),
            )?;
        }
    }
    Ok(())
}

pub fn hash(s: &Screen) -> u64 {
    s.pixels
        .chunks_exact(4)
        .flat_map(|b| [b[0], b[1], b[2], 255])
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
}

pub fn ttf_text(
    screen: &Screen,
    bytes: &[u8],
    x: i32,
    y: i32,
    fg: u32,
    bg: u32,
    scale: i32,
    path: &str,
) -> CaseResult {
    use libgfx_ref::ttf_font::{self, CachedFont, Font};
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut font = CachedFont::new(
        Font::parse(&data).map_err(|e| format!("reference pinned font: {e:?}"))?,
        256,
    );
    let width = ttf_font::text_width_bytes(&mut font, bytes, 14.0 * scale as f32).max(1) as usize;
    let mut image = FrameBuf::new(width, 32 * scale as usize, Format::BGRX)
        .map_err(|e| format!("reference font surface: {e:?}"))?;
    let color = |c: u32| Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    image.clear(color(bg));
    ttf_font::draw_text_subpixel(
        &mut image,
        &mut font,
        std::str::from_utf8(bytes).map_err(|e| e.to_string())?,
        0,
        0,
        14.0 * scale as f32,
        color(fg),
    );
    for py in 0..32 * scale as usize {
        for px in 0..width {
            let c = image.get_pixel(px, py);
            let expected = 0xff000000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            check(
                screen.pixel(x + px as i32, y + py as i32)? == expected,
                "configured title differs from certified TrueType renderer",
            )?;
        }
    }
    Ok(())
}
