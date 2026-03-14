//! `glyf` table: glyph outlines — simple glyphs and compound glyphs.

use alloc::string::String;
use alloc::vec::Vec;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy)]
pub struct GlyphPoint {
    pub x: i16,
    pub y: i16,
    pub on_curve: bool,
}

#[derive(Debug, Clone)]
pub struct SimpleGlyph {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub contours: Vec<Vec<GlyphPoint>>,
}

#[derive(Debug, Clone, Copy)]
pub struct CompoundComponent {
    pub glyph_index: u16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub scale_xx: f32,
    pub scale_xy: f32,
    pub scale_yx: f32,
    pub scale_yy: f32,
}

// Flag bits for simple glyph points
const ON_CURVE_POINT: u8 = 0x01;
const X_SHORT_VECTOR: u8 = 0x02;
const Y_SHORT_VECTOR: u8 = 0x04;
const REPEAT_FLAG: u8 = 0x08;
const X_IS_SAME_OR_POSITIVE_X_SHORT: u8 = 0x10;
const Y_IS_SAME_OR_POSITIVE_Y_SHORT: u8 = 0x20;

// Compound glyph flags
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

pub fn parse_glyph(glyf_data: &[u8], offset: u32) -> Result<Option<SimpleGlyph>, String> {
    let off = offset as usize;
    if off + 10 > glyf_data.len() {
        return Err(String::from("glyph offset out of bounds"));
    }
    let mut r = Reader::at(glyf_data, off);
    let num_contours = r.read_i16()?;
    let x_min = r.read_i16()?;
    let y_min = r.read_i16()?;
    let x_max = r.read_i16()?;
    let y_max = r.read_i16()?;

    if num_contours >= 0 {
        parse_simple_glyph(&mut r, num_contours as u16, x_min, y_min, x_max, y_max)
            .map(Some)
    } else {
        parse_compound_glyph(glyf_data, &mut r, x_min, y_min, x_max, y_max).map(Some)
    }
}

fn parse_simple_glyph(
    r: &mut Reader,
    num_contours: u16,
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
) -> Result<SimpleGlyph, String> {
    if num_contours == 0 {
        return Ok(SimpleGlyph {
            x_min, y_min, x_max, y_max,
            contours: Vec::new(),
        });
    }

    // Read end points of contours
    let mut end_points = Vec::with_capacity(num_contours as usize);
    for _ in 0..num_contours {
        end_points.push(r.read_u16()?);
    }

    let num_points = *end_points.last().unwrap() as usize + 1;

    // Skip instructions
    let instruction_length = r.read_u16()? as usize;
    r.skip(instruction_length)?;

    // Read flags
    let mut flags = Vec::with_capacity(num_points);
    while flags.len() < num_points {
        let flag = r.read_u8()?;
        flags.push(flag);
        if flag & REPEAT_FLAG != 0 {
            let count = r.read_u8()? as usize;
            for _ in 0..count {
                flags.push(flag);
            }
        }
    }

    // Read x coordinates
    let mut x_coords = Vec::with_capacity(num_points);
    let mut x: i16 = 0;
    for &flag in &flags[..num_points] {
        if flag & X_SHORT_VECTOR != 0 {
            let dx = r.read_u8()? as i16;
            if flag & X_IS_SAME_OR_POSITIVE_X_SHORT != 0 {
                x += dx;
            } else {
                x -= dx;
            }
        } else if flag & X_IS_SAME_OR_POSITIVE_X_SHORT == 0 {
            x += r.read_i16()?;
        }
        // else: x is same as previous
        x_coords.push(x);
    }

    // Read y coordinates
    let mut y_coords = Vec::with_capacity(num_points);
    let mut y: i16 = 0;
    for &flag in &flags[..num_points] {
        if flag & Y_SHORT_VECTOR != 0 {
            let dy = r.read_u8()? as i16;
            if flag & Y_IS_SAME_OR_POSITIVE_Y_SHORT != 0 {
                y += dy;
            } else {
                y -= dy;
            }
        } else if flag & Y_IS_SAME_OR_POSITIVE_Y_SHORT == 0 {
            y += r.read_i16()?;
        }
        y_coords.push(y);
    }

    // Build contours
    let mut contours = Vec::with_capacity(num_contours as usize);
    let mut start = 0usize;
    for &end in &end_points {
        let end = end as usize;
        if end >= num_points {
            return Err(String::from("contour end point out of bounds"));
        }
        let mut contour = Vec::with_capacity(end - start + 1);
        for i in start..=end {
            contour.push(GlyphPoint {
                x: x_coords[i],
                y: y_coords[i],
                on_curve: flags[i] & ON_CURVE_POINT != 0,
            });
        }
        contours.push(contour);
        start = end + 1;
    }

    Ok(SimpleGlyph {
        x_min, y_min, x_max, y_max,
        contours,
    })
}

fn parse_compound_glyph(
    _glyf_data: &[u8],
    r: &mut Reader,
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
) -> Result<SimpleGlyph, String> {
    let mut components = Vec::new();

    loop {
        let flags = r.read_u16()?;
        let glyph_index = r.read_u16()?;

        let (x_offset, y_offset) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            if flags & ARGS_ARE_XY_VALUES != 0 {
                (r.read_i16()?, r.read_i16()?)
            } else {
                let _ = r.read_u16()?;
                let _ = r.read_u16()?;
                (0i16, 0i16)
            }
        } else if flags & ARGS_ARE_XY_VALUES != 0 {
            (r.read_i8()? as i16, r.read_i8()? as i16)
        } else {
            let _ = r.read_u8()?;
            let _ = r.read_u8()?;
            (0i16, 0i16)
        };

        let (scale_xx, scale_xy, scale_yx, scale_yy) = if flags & WE_HAVE_A_SCALE != 0 {
            let s = r.read_i16()? as f32 / 16384.0;
            (s, 0.0, 0.0, s)
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            let sx = r.read_i16()? as f32 / 16384.0;
            let sy = r.read_i16()? as f32 / 16384.0;
            (sx, 0.0, 0.0, sy)
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            let xx = r.read_i16()? as f32 / 16384.0;
            let xy = r.read_i16()? as f32 / 16384.0;
            let yx = r.read_i16()? as f32 / 16384.0;
            let yy = r.read_i16()? as f32 / 16384.0;
            (xx, xy, yx, yy)
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };

        components.push(CompoundComponent {
            glyph_index,
            x_offset,
            y_offset,
            scale_xx,
            scale_xy,
            scale_yx,
            scale_yy,
        });

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }

    // Components will be resolved by Font::resolve_glyph via resolve_compound().
    // Return empty contours here; the higher-level code handles recursion.
    let all_contours = Vec::new();
    let _ = &components;

    Ok(SimpleGlyph {
        x_min, y_min, x_max, y_max,
        contours: all_contours,
    })
}

/// Resolve a compound glyph by recursively looking up components.
/// `lookup_fn` should return the parsed simple glyph for a given glyph index.
pub fn resolve_compound(
    glyf_data: &[u8],
    offset: u32,
    lookup_fn: &dyn Fn(u16) -> Option<SimpleGlyph>,
) -> Result<Option<SimpleGlyph>, String> {
    let off = offset as usize;
    if off + 10 > glyf_data.len() {
        return Err(String::from("compound glyph offset out of bounds"));
    }
    let mut r = Reader::at(glyf_data, off);
    let num_contours = r.read_i16()?;
    let x_min = r.read_i16()?;
    let y_min = r.read_i16()?;
    let x_max = r.read_i16()?;
    let y_max = r.read_i16()?;

    if num_contours >= 0 {
        // Not compound — parse as simple
        return parse_simple_glyph(&mut r, num_contours as u16, x_min, y_min, x_max, y_max)
            .map(Some);
    }

    let mut all_contours = Vec::new();

    loop {
        let flags = r.read_u16()?;
        let glyph_index = r.read_u16()?;

        let (x_offset, y_offset) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            if flags & ARGS_ARE_XY_VALUES != 0 {
                (r.read_i16()?, r.read_i16()?)
            } else {
                let _ = r.read_u16()?;
                let _ = r.read_u16()?;
                (0i16, 0i16)
            }
        } else if flags & ARGS_ARE_XY_VALUES != 0 {
            (r.read_i8()? as i16, r.read_i8()? as i16)
        } else {
            let _ = r.read_u8()?;
            let _ = r.read_u8()?;
            (0i16, 0i16)
        };

        let (scale_xx, scale_xy, scale_yx, scale_yy) = if flags & WE_HAVE_A_SCALE != 0 {
            let s = r.read_i16()? as f32 / 16384.0;
            (s, 0.0, 0.0, s)
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            let sx = r.read_i16()? as f32 / 16384.0;
            let sy = r.read_i16()? as f32 / 16384.0;
            (sx, 0.0, 0.0, sy)
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            let xx = r.read_i16()? as f32 / 16384.0;
            let xy = r.read_i16()? as f32 / 16384.0;
            let yx = r.read_i16()? as f32 / 16384.0;
            let yy = r.read_i16()? as f32 / 16384.0;
            (xx, xy, yx, yy)
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };

        if let Some(component_glyph) = lookup_fn(glyph_index) {
            for contour in &component_glyph.contours {
                let transformed: Vec<GlyphPoint> = contour.iter().map(|p| {
                    let tx = (p.x as f32 * scale_xx + p.y as f32 * scale_yx) + x_offset as f32;
                    let ty = (p.x as f32 * scale_xy + p.y as f32 * scale_yy) + y_offset as f32;
                    GlyphPoint {
                        x: tx as i16,
                        y: ty as i16,
                        on_curve: p.on_curve,
                    }
                }).collect();
                all_contours.push(transformed);
            }
        }

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }

    Ok(Some(SimpleGlyph {
        x_min, y_min, x_max, y_max,
        contours: all_contours,
    }))
}
