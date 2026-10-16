//! `glyf`: glyph records (OpenType specification, "glyf" chapter). A defect inside one
//! record is an error for that glyph only.

use alloc::vec::Vec;

use crate::error::FontError;
use crate::reader::{i16_at, Reader};
use crate::work;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphPoint {
    pub x: i16,
    pub y: i16,
    pub on_curve: bool,
}

/// A simple glyph: its header bounding box and its points in font units, with the index of
/// each contour's last point (`endPtsOfContours`).
#[derive(Debug, Clone)]
pub struct SimpleGlyph {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub end_points: Vec<u16>,
    pub points: Vec<GlyphPoint>,
}

/// One component of a compound glyph.
#[derive(Debug, Clone, Copy)]
pub struct Component {
    pub glyph: u16,
    pub flags: u16,
    /// The x and y offsets (`ARGS_ARE_XY_VALUES`), or the parent and child point numbers.
    pub arg1: i32,
    pub arg2: i32,
    /// The 2×2 transform `[xx xy; yx yy]`: x' = xx·x + yx·y, y' = xy·x + yy·y.
    pub xx: f32,
    pub xy: f32,
    pub yx: f32,
    pub yy: f32,
}

/// A parsed glyph record.
#[derive(Debug, Clone)]
pub enum Glyph {
    Simple(SimpleGlyph),
    Compound { x_min: i16, y_min: i16, x_max: i16, y_max: i16, components: Vec<Component> },
}

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;

pub const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
pub const ARGS_ARE_XY_VALUES: u16 = 0x0002;
pub const WE_HAVE_A_SCALE: u16 = 0x0008;
pub const MORE_COMPONENTS: u16 = 0x0020;
pub const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
pub const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
pub const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
pub const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
pub const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

/// Components in one compound record, at most.
pub const MAX_COMPONENTS: usize = 256;

/// Parse one glyph record (`data` is exactly its bytes). A simple glyph may hold at most
/// `max_points` points; a component must name a glyph below `num_glyphs`.
pub fn parse(data: &[u8], max_points: usize, num_glyphs: u16) -> Result<Glyph, FontError> {
    let bad = FontError::BadGlyph;
    let contours = i16_at(data, 0).ok_or(bad)?;
    let b = |o| i16_at(data, o).ok_or(bad);
    let (x_min, y_min, x_max, y_max) = (b(2)?, b(4)?, b(6)?, b(8)?);
    if contours >= 0 {
        parse_simple(data, contours as usize, max_points)
            .map(|(end_points, points)| Glyph::Simple(SimpleGlyph { x_min, y_min, x_max, y_max, end_points, points }))
    } else {
        parse_components(data, num_glyphs).map(|components| Glyph::Compound { x_min, y_min, x_max, y_max, components })
    }
}

fn parse_simple(data: &[u8], n_contours: usize, max_points: usize) -> Result<(Vec<u16>, Vec<GlyphPoint>), FontError> {
    let bad = FontError::BadGlyph;
    if n_contours == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut r = Reader::at(data, 10);
    // At most 32,767 entries of 2 bytes, whatever the record holds.
    let mut ends: Vec<u16> = Vec::with_capacity(n_contours.min(data.len() / 2));
    for i in 0..n_contours {
        work::add(1);
        let e = r.u16().ok_or(bad)?;
        if i > 0 && e <= ends[i - 1] {
            return Err(bad);
        }
        ends.push(e);
    }
    let points = ends[n_contours - 1] as usize + 1;
    if points > max_points || points > 65_535 {
        return Err(FontError::TooManyPoints);
    }
    let instructions = r.u16().ok_or(bad)? as usize;
    r.skip(instructions).ok_or(bad)?;
    // Flags, with repeats; a run past the point count is malformed.
    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        work::add(1);
        let f = r.u8().ok_or(bad)?;
        flags.push(f);
        if f & REPEAT != 0 {
            let n = r.u8().ok_or(bad)? as usize;
            if flags.len() + n > points {
                return Err(bad);
            }
            work::add(n as u64);
            flags.extend(core::iter::repeat_n(f, n));
        }
    }
    // The x coordinates, then the y coordinates, each a run of deltas.
    let mut out: Vec<GlyphPoint> = Vec::with_capacity(points);
    let mut x = 0i16;
    for &f in &flags {
        work::add(1);
        x = delta(&mut r, f, X_SHORT, X_SAME_OR_POSITIVE, x).ok_or(bad)?;
        out.push(GlyphPoint { x, y: 0, on_curve: f & ON_CURVE != 0 });
    }
    let mut y = 0i16;
    for (p, &f) in out.iter_mut().zip(&flags) {
        work::add(1);
        y = delta(&mut r, f, Y_SHORT, Y_SAME_OR_POSITIVE, y).ok_or(bad)?;
        p.y = y;
    }
    Ok((ends, out))
}

/// The next coordinate after `v`: a one-byte delta whose sign `same` gives when `short` is
/// set, else `v` itself when `same` is set, else a two-byte delta.
fn delta(r: &mut Reader<'_>, f: u8, short: u8, same: u8, v: i16) -> Option<i16> {
    Some(if f & short != 0 {
        let d = r.u8()? as i16;
        if f & same != 0 { v.wrapping_add(d) } else { v.wrapping_sub(d) }
    } else if f & same == 0 {
        v.wrapping_add(r.i16()?)
    } else {
        v
    })
}

fn parse_components(data: &[u8], num_glyphs: u16) -> Result<Vec<Component>, FontError> {
    let bad = FontError::BadGlyph;
    let mut r = Reader::at(data, 10);
    let mut out = Vec::new();
    let mut instructions = false;
    loop {
        work::add(1);
        if out.len() == MAX_COMPONENTS {
            return Err(FontError::CompoundTooDeep);
        }
        let flags = r.u16().ok_or(bad)?;
        let glyph = r.u16().ok_or(bad)?;
        if glyph >= num_glyphs {
            return Err(FontError::BadComponent);
        }
        let xy = flags & ARGS_ARE_XY_VALUES != 0;
        let (arg1, arg2) = match (flags & ARG_1_AND_2_ARE_WORDS != 0, xy) {
            (true, true) => (r.i16().ok_or(bad)? as i32, r.i16().ok_or(bad)? as i32),
            (true, false) => (r.u16().ok_or(bad)? as i32, r.u16().ok_or(bad)? as i32),
            (false, true) => (r.i8().ok_or(bad)? as i32, r.i8().ok_or(bad)? as i32),
            (false, false) => (r.u8().ok_or(bad)? as i32, r.u8().ok_or(bad)? as i32),
        };
        let f2dot14 = |v: i16| v as f32 / 16_384.0;
        let (xx, xy_, yx, yy) = if flags & WE_HAVE_A_SCALE != 0 {
            let s = f2dot14(r.i16().ok_or(bad)?);
            (s, 0.0, 0.0, s)
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            (f2dot14(r.i16().ok_or(bad)?), 0.0, 0.0, f2dot14(r.i16().ok_or(bad)?))
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            (f2dot14(r.i16().ok_or(bad)?), f2dot14(r.i16().ok_or(bad)?), f2dot14(r.i16().ok_or(bad)?), f2dot14(r.i16().ok_or(bad)?))
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };
        out.push(Component { glyph, flags, arg1, arg2, xx, xy: xy_, yx, yy });
        instructions |= flags & WE_HAVE_INSTRUCTIONS != 0;
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    // Instructions for the compound follow its last component: a length, then the bytes.
    if instructions {
        let n = r.u16().ok_or(bad)? as usize;
        r.skip(n).ok_or(bad)?;
    }
    Ok(out)
}
