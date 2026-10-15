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

/// A simple glyph: its header bounding box and contours of points in font units.
#[derive(Debug, Clone)]
pub struct SimpleGlyph {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub contours: Vec<Vec<GlyphPoint>>,
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
        parse_simple(data, contours as usize, max_points).map(|contours| Glyph::Simple(SimpleGlyph { x_min, y_min, x_max, y_max, contours }))
    } else {
        parse_components(data, num_glyphs).map(|components| Glyph::Compound { x_min, y_min, x_max, y_max, components })
    }
}

fn parse_simple(data: &[u8], n_contours: usize, max_points: usize) -> Result<Vec<Vec<GlyphPoint>>, FontError> {
    let bad = FontError::BadGlyph;
    if n_contours == 0 {
        return Ok(Vec::new());
    }
    let mut r = Reader::at(data, 10);
    let mut ends = Vec::with_capacity(n_contours.min(data.len() / 2));
    work::add(n_contours as u64);
    for i in 0..n_contours {
        let e = r.u16().ok_or(bad)?;
        if i > 0 && e as usize <= ends[i - 1] {
            return Err(bad);
        }
        ends.push(e as usize);
    }
    let points = ends[n_contours - 1] + 1;
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
    let mut coords = |short: u8, same: u8| -> Result<Vec<i16>, FontError> {
        let mut v = 0i16;
        let mut out = Vec::with_capacity(points);
        work::add(points as u64);
        for &f in &flags {
            if f & short != 0 {
                let d = r.u8().ok_or(bad)? as i16;
                v = if f & same != 0 { v.wrapping_add(d) } else { v.wrapping_sub(d) };
            } else if f & same == 0 {
                v = v.wrapping_add(r.i16().ok_or(bad)?);
            }
            out.push(v);
        }
        Ok(out)
    };
    let xs = coords(X_SHORT, X_SAME_OR_POSITIVE)?;
    let ys = coords(Y_SHORT, Y_SAME_OR_POSITIVE)?;
    let mut out = Vec::with_capacity(n_contours);
    let mut start = 0;
    work::add(points as u64);
    for &end in &ends {
        out.push((start..=end).map(|i| GlyphPoint { x: xs[i], y: ys[i], on_curve: flags[i] & ON_CURVE != 0 }).collect());
        start = end + 1;
    }
    Ok(out)
}

fn parse_components(data: &[u8], num_glyphs: u16) -> Result<Vec<Component>, FontError> {
    let bad = FontError::BadGlyph;
    let mut r = Reader::at(data, 10);
    let mut out = Vec::new();
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
        if flags & MORE_COMPONENTS == 0 {
            return Ok(out);
        }
    }
}
