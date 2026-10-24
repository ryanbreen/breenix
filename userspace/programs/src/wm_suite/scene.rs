//! Independent contract pixels, with certified glyphs and literal chrome geometry.
use super::kit::*;
use libgfx_ref::{
    color::Color,
    framebuf::{Format, FrameBuf},
};

pub fn background_region(s: &Screen, scale: i32, rect: (i32, i32, i32, i32)) -> CaseResult {
    for y in rect.1..rect.1 + rect.3 {
        for x in rect.0..rect.0 + rect.2 {
            if s.pixel(x,y)? != background(x,y,s.height as i32,scale) {
                return Err(format!("desktop background differs at {x},{y}").into());
            }
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
    let mut image = FrameBuf::new(width, 16 * scale as usize, Format::BGRX)
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
    for py in 0..16 * scale as usize {
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

/// The frozen empty desktop, including certified glyphs, both complete bars,
/// the full background and the arrow tip clipped at the last mode pixel.
pub fn empty_desktop(s: &Screen, d: &Desktop) -> CaseResult {
    use libgfx_ref::ttf_font::{self, CachedFont, Font};
    let bytes = std::fs::read("/usr/share/fonts/DejaVuSansMono.ttf").map_err(|e| e.to_string())?;
    let mut font = CachedFont::new(
        Font::parse(&bytes).map_err(|e| format!("reference font: {e:?}"))?,
        256,
    );
    let mut expected = FrameBuf::new(d.width as usize, d.height as usize, Format::BGRX)
        .map_err(|e| format!("reference empty desktop: {e:?}"))?;
    let color = |c: u32| Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    for y in 0..d.height as i32 {
        for x in 0..d.width as i32 {
            let c = if y < 28 * d.scale {
                0xff14161e
            } else if y >= d.height as i32 - 35 * d.scale {
                0xff191c26
            } else if y >= d.height as i32 - 36 * d.scale {
                0xff323746
            } else {
                background(x, y, d.height as i32, d.scale)
            };
            expected.put_pixel(x as usize, y as usize, color(c));
        }
    }
    // Text's raster is independent of bwm. Its contract fixes the wall value;
    // locate that literal raster in the top bar, then judge the complete mode.
    for text in ["Breenix", "00:00:00 UTC"] {
        let (x, y) = find_text(s, &mut font, text, d.scale, 28 * d.scale)?;
        if text == "Breenix" {
            check(
                x == 8 * d.scale,
                "desktop label violates the contract inset",
            )?;
        }
        ttf_font::draw_text_subpixel(
            &mut expected,
            &mut font,
            text,
            x,
            y,
            14.0 * d.scale as f32,
            Color::rgb(180, 185, 195),
        );
    }
    expected.put_pixel(
        (d.width - 1) as usize,
        (d.height - 1) as usize,
        Color::rgb(0, 0, 0),
    );
    for y in 0..d.height {
        for x in 0..d.width {
            let c = expected.get_pixel(x as usize, y as usize);
            let want = 0xff000000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            if s.pixel(x as i32,y as i32)? != want {
                return Err(format!("empty desktop differs at {x},{y}").into());
            }
        }
    }
    Ok(())
}

fn find_text(
    s: &Screen,
    font: &mut libgfx_ref::ttf_font::CachedFont,
    text: &str,
    scale: i32,
    bar_height: i32,
) -> Result<(i32, i32), String> {
    use libgfx_ref::ttf_font;
    let width = ttf_font::text_width(font, text, 14.0 * scale as f32).max(1) as usize;
    let height = 16 * scale as usize;
    let mut image = FrameBuf::new(width, height, Format::BGRX)
        .map_err(|e| format!("text search raster: {e:?}"))?;
    image.clear(Color::rgb(20, 22, 30));
    ttf_font::draw_text_subpixel(
        &mut image,
        font,
        text,
        0,
        0,
        14.0 * scale as f32,
        Color::rgb(180, 185, 195),
    );
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let c = image.get_pixel(x, y);
            pixels.push((
                x as i32,
                y as i32,
                0xff000000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32,
            ));
        }
    }
    let anchor = pixels
        .iter()
        .find(|p| p.2 != 0xff14161e)
        .ok_or("reference text has no ink")?;
    for y in 0..=bar_height - height as i32 {
        for x in 0..=s.width as i32 - width as i32 {
            if s.pixel(x + anchor.0, y + anchor.1)? != anchor.2 {
                continue;
            }
            if pixels
                .iter()
                .all(|p| s.pixel(x + p.0, y + p.1).ok() == Some(p.2))
            {
                return Ok((x, y));
            }
        }
    }
    Err(format!(
        "top bar lacks the certified pinned-font raster for {text}"
    ))
}

pub fn clock(s: &Screen, d: &Desktop, text: &str) -> CaseResult {
    use libgfx_ref::ttf_font::{CachedFont, Font};
    let data = std::fs::read("/usr/share/fonts/DejaVuSansMono.ttf").map_err(|e| e.to_string())?;
    let mut font = CachedFont::new(
        Font::parse(&data).map_err(|e| format!("clock reference font: {e:?}"))?,
        256,
    );
    let (x, _) = find_text(s, &mut font, text, d.scale, 28 * d.scale)?;
    check(
        x > d.width as i32 / 2,
        "clock text is outside the right half of the top bar",
    )
}
