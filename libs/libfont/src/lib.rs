//! TrueType font parser and anti-aliased rasterizer (docs/gui/breengel.md sections 7, 8
//! and 10), written from the OpenType specification.
//!
//! `#![no_std]` + `extern crate alloc`. Zero external dependencies.
//!
//! `Font::parse` validates every table it reads, so a malformed font is refused whole with a
//! `FontError`; a defect confined to one glyph is reported when that glyph is read, and every
//! other glyph still works. Every loop counts its iterations on `libfont::work`.

#![no_std]
extern crate alloc;

pub mod cache;
pub mod error;
mod float;
pub mod outline;
pub mod rasterizer;
mod reader;
pub mod tables;
pub mod work;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Range;

use crate::cache::GlyphCache;
use crate::float::{ceil, floor};
use crate::outline::{flatten, Outline, OutlinePoint, MAX_DEPTH};
use crate::rasterizer::{rasterize, rasterize_subpixel};
use crate::tables::cmap::CmapTable;
use crate::tables::glyf::{self, Glyph, ARGS_ARE_XY_VALUES, SCALED_COMPONENT_OFFSET, UNSCALED_COMPONENT_OFFSET};
use crate::tables::head::HeadTable;
use crate::tables::hhea::HheaTable;
use crate::tables::hmtx::HmtxTable;
use crate::tables::kern::KernTable;
use crate::tables::loca::LocaTable;
use crate::tables::maxp::MaxpTable;
use crate::tables::TableDirectory;

pub use crate::cache::CacheStats;
pub use crate::error::FontError;
pub use crate::rasterizer::{GlyphBitmap, SubpixelBitmap};
pub use crate::rasterizer::GlyphBitmap as Bitmap;

/// Sizes accepted, in pixels per em.
pub const MIN_SIZE: f32 = 1.0;
pub const MAX_SIZE: f32 = 1024.0;

/// Line metrics at one size, in pixels: each is the 26.6 value
/// `round_half_away(hhea_value × size26 / unitsPerEm)` divided by 64.
#[derive(Debug, Clone, Copy)]
pub struct ScaledMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub line_height: f32,
}

/// One glyph's metrics in font units: hmtx's advance and left side bearing, and the
/// glyph record's bounding box (all zero for an empty glyph).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphMetrics {
    pub advance: u16,
    pub lsb: i16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

/// `round_half_away(n / d)` for `d > 0`, exactly.
pub fn round_half_away(n: i64, d: i64) -> i64 {
    if n >= 0 { (2 * n + d) / (2 * d) } else { -((-2 * n + d) / (2 * d)) }
}

/// A parsed TrueType font. Owns a copy of its file.
pub struct Font {
    data: Box<[u8]>,
    head: HeadTable,
    hhea: HheaTable,
    maxp: MaxpTable,
    cmap: CmapTable,
    hmtx: Range<usize>,
    loca: Range<usize>,
    glyf: Range<usize>,
    kern: Option<KernTable>,
}

impl Font {
    /// Parse and validate a TrueType font (section 10's font-scoped rules).
    pub fn parse(data: &[u8]) -> Result<Font, FontError> {
        let dir = TableDirectory::parse(data)?;
        let head = HeadTable::parse(&data[dir.require(b"head")?])?;
        let hhea = HheaTable::parse(&data[dir.require(b"hhea")?])?;
        let maxp = MaxpTable::parse(&data[dir.require(b"maxp")?])?;
        let cmap_range = dir.require(b"cmap")?;
        let hmtx = dir.require(b"hmtx")?;
        let loca = dir.require(b"loca")?;
        let glyf = dir.require(b"glyf")?;
        LocaTable::new(&data[loca.clone()], head.index_to_loc_format).validate(maxp.num_glyphs)?;
        HmtxTable::new(&data[hmtx.clone()], hhea.num_h_metrics).validate(maxp.num_glyphs)?;
        let cmap = CmapTable::parse(data, cmap_range)?;
        let kern = match dir.find(b"kern") {
            Some(range) => KernTable::parse(data, range)?,
            None => None,
        };
        Ok(Font { data: Box::from(data), head, hhea, maxp, cmap, hmtx, loca, glyf, kern })
    }

    pub fn units_per_em(&self) -> u16 {
        self.head.units_per_em
    }

    pub fn num_glyphs(&self) -> u16 {
        self.maxp.num_glyphs
    }

    fn hmtx_table(&self) -> HmtxTable<'_> {
        HmtxTable::new(&self.data[self.hmtx.clone()], self.hhea.num_h_metrics)
    }

    /// The glyph for a codepoint; 0 (`.notdef`) when the font does not map it.
    pub fn glyph_for(&self, cp: u32) -> u16 {
        let g = self.cmap.lookup(&self.data, cp);
        if g < self.maxp.num_glyphs { g } else { 0 }
    }

    pub fn glyph_index(&self, ch: char) -> u16 {
        self.glyph_for(ch as u32)
    }

    pub fn advance_units(&self, glyph: u16) -> u16 {
        self.hmtx_table().advance_width(glyph)
    }

    /// The kerning of a pair in font units, from the legacy `kern` table: the sum over its
    /// horizontal format 0 subtables, where an override subtable replaces the sum so far.
    pub fn kern_units(&self, left: u16, right: u16) -> i32 {
        self.kern.as_ref().map_or(0, |k| k.value(&self.data, left, right))
    }

    /// A glyph's advance in pixels at `pixel_size`.
    pub fn advance_width(&self, glyph: u16, pixel_size: f32) -> f32 {
        self.advance_units(glyph) as f32 * (pixel_size / self.head.units_per_em as f32)
    }

    /// A pair's kerning in pixels at `pixel_size`.
    pub fn kern(&self, left: u16, right: u16, pixel_size: f32) -> f32 {
        self.kern_units(left, right) as f32 * (pixel_size / self.head.units_per_em as f32)
    }

    /// `round_half_away(size × 64)`; `BadSize` outside 1 to 1,024.
    pub fn size26(size: f32) -> Result<i32, FontError> {
        if !(MIN_SIZE..=MAX_SIZE).contains(&size) {
            return Err(FontError::BadSize);
        }
        Ok(floor(size * 64.0 + 0.5) as i32)
    }

    /// `round_half_away(advance_units × size26 / unitsPerEm)`.
    pub fn advance26(&self, glyph: u16, size26: i32) -> i32 {
        round_half_away(self.advance_units(glyph) as i64 * size26 as i64, self.head.units_per_em as i64) as i32
    }

    /// `round_half_away(kern_units × size26 / unitsPerEm)`.
    pub fn kern26(&self, left: u16, right: u16, size26: i32) -> i32 {
        round_half_away(self.kern_units(left, right) as i64 * size26 as i64, self.head.units_per_em as i64) as i32
    }

    /// The hhea line metrics at `pixel_size`, through 26.6 (section 8.3). A size outside
    /// 1 to 1,024 scales directly.
    pub fn metrics(&self, pixel_size: f32) -> ScaledMetrics {
        let upem = self.head.units_per_em as i64;
        let v = |units: i16| match Font::size26(pixel_size) {
            Ok(s26) => round_half_away(units as i64 * s26 as i64, upem) as f32 / 64.0,
            Err(_) => units as f32 * (pixel_size / upem as f32),
        };
        let (ascender, descender, line_gap) = (v(self.hhea.ascender), v(self.hhea.descender), v(self.hhea.line_gap));
        ScaledMetrics { ascender, descender, line_gap, line_height: ascender - descender + line_gap }
    }

    /// The bytes of glyph `glyph`'s record; `None` for an empty glyph.
    fn record(&self, glyph: u16) -> Result<Option<&[u8]>, FontError> {
        if glyph >= self.maxp.num_glyphs {
            return Err(FontError::GlyphOutOfRange);
        }
        let loca = LocaTable::new(&self.data[self.loca.clone()], self.head.index_to_loc_format);
        let (start, end) = match (loca.entry(glyph as usize), loca.entry(glyph as usize + 1)) {
            (Some(s), Some(e)) => (s as usize, e as usize),
            _ => return Err(FontError::GlyphOutOfRange),
        };
        // A glyph's own loca entry places it: one starting past the glyf table is out of
        // range, even when the next entry equals it, and a record the next entry would run
        // past the table ends at the table's end (section 10 scopes a bad loca entry to its
        // own glyph).
        let glyf = &self.data[self.glyf.clone()];
        if start > glyf.len() || start > end {
            return Err(FontError::GlyphOutOfRange);
        }
        if start == end {
            return Ok(None);
        }
        if start == glyf.len() {
            return Err(FontError::GlyphOutOfRange);
        }
        Ok(Some(&glyf[start..end.min(glyf.len())]))
    }

    fn parse_glyph(&self, glyph: u16) -> Result<Option<Glyph>, FontError> {
        match self.record(glyph)? {
            None => Ok(None),
            Some(bytes) => glyf::parse(bytes, self.maxp.max_points as usize, self.maxp.num_glyphs).map(Some),
        }
    }

    /// The advance, left side bearing and record bounding box of a glyph, in font units.
    pub fn glyph_metrics(&self, glyph: u16) -> Result<GlyphMetrics, FontError> {
        let hmtx = self.hmtx_table();
        let (advance, lsb) = (hmtx.advance_width(glyph), hmtx.left_side_bearing(glyph));
        let rec = self.record(glyph)?;
        let b = |o: usize| rec.and_then(|r| r.get(o..o + 2)).map_or(0, |v| i16::from_be_bytes([v[0], v[1]]));
        Ok(GlyphMetrics { advance, lsb, x_min: b(2), y_min: b(4), x_max: b(6), y_max: b(8) })
    }

    /// A glyph's outline with every compound expanded (section 10's glyph-scoped rules:
    /// cycles, nesting deeper than 8, more than 256 components, bad component ids, and point
    /// counts above 65,535 or what `maxp` declares: `maxPoints` for a simple glyph,
    /// `maxCompositePoints` for a compound after expansion).
    pub fn glyph_outline(&self, glyph: u16) -> Result<Outline, FontError> {
        let m = self.glyph_metrics(glyph)?;
        let (x_min, y_min, x_max, y_max) = (m.x_min, m.y_min, m.x_max, m.y_max);
        let mut budget = Budget { points: 0, components: 0, limit: 0 };
        let mut out = Outline { x_min, y_min, x_max, y_max, points: Vec::new(), ends: Vec::new() };
        self.expand(glyph, IDENTITY, &mut Vec::new(), &mut budget, &mut out)?;
        Ok(out)
    }

    /// Append the contours of `glyph`, mapped by `t`, to `out`. `stack` holds the compounds
    /// being expanded, outermost first. Each point is stored once, in the outermost
    /// glyph's coordinates, whatever the nesting.
    fn expand(&self, glyph: u16, t: Affine, stack: &mut Vec<u16>, budget: &mut Budget, out: &mut Outline) -> Result<(), FontError> {
        work::add(1);
        let parsed = self.parse_glyph(glyph)?;
        if stack.is_empty() {
            // The outermost glyph sets the point limit maxp declares for its kind.
            let declared = match parsed {
                Some(Glyph::Compound { .. }) => self.maxp.max_composite_points,
                _ => self.maxp.max_points,
            };
            budget.limit = (declared as usize).min(65_535);
        }
        match parsed {
            None => Ok(()),
            Some(Glyph::Simple(s)) => {
                budget.add_points(s.points.len())?;
                let base = out.points.len() as u32;
                work::add(s.points.len() as u64 + s.end_points.len() as u64);
                out.points.extend(s.points.iter().map(|p| {
                    let (x, y) = t.apply(p.x as f32, p.y as f32);
                    OutlinePoint { x, y, on_curve: p.on_curve }
                }));
                out.ends.extend(s.end_points.iter().map(|&e| base + e as u32 + 1));
                Ok(())
            }
            Some(Glyph::Compound { components, .. }) => {
                stack.push(glyph);
                if stack.len() > MAX_DEPTH {
                    return Err(FontError::CompoundTooDeep);
                }
                let first = out.points.len();
                for c in components {
                    work::add(1);
                    budget.components += 1;
                    if budget.components > glyf::MAX_COMPONENTS {
                        return Err(FontError::CompoundTooDeep);
                    }
                    if stack.contains(&c.glyph) {
                        return Err(FontError::CompoundCycle);
                    }
                    let local = Affine([c.xx, c.xy, c.yx, c.yy, 0.0, 0.0]);
                    if c.flags & ARGS_ARE_XY_VALUES != 0 {
                        // The offset is in the compound's units, unless SCALED_COMPONENT_OFFSET
                        // alone puts it in the component's, where the component's transform
                        // applies to it too (both flags set is invalid: the default holds).
                        let (dx, dy) = (c.arg1 as f32, c.arg2 as f32);
                        let scaled = c.flags & (SCALED_COMPONENT_OFFSET | UNSCALED_COMPONENT_OFFSET) == SCALED_COMPONENT_OFFSET;
                        let (dx, dy) = if scaled { local.apply(dx, dy) } else { (dx, dy) };
                        let moved = Affine([c.xx, c.xy, c.yx, c.yy, dx, dy]);
                        self.expand(c.glyph, t.then(moved), stack, budget, out)?;
                    } else {
                        // Point matching: the component's point arg2 lands on this compound's
                        // point arg1 (both counted from the start of their own points).
                        let start = out.points.len();
                        self.expand(c.glyph, t.then(local), stack, budget, out)?;
                        let target = *out.points[first..start].get(c.arg1 as usize).ok_or(FontError::BadGlyph)?;
                        let own = *out.points[start..].get(c.arg2 as usize).ok_or(FontError::BadGlyph)?;
                        let (dx, dy) = (target.x - own.x, target.y - own.y);
                        work::add((out.points.len() - start) as u64);
                        for p in &mut out.points[start..] {
                            p.x += dx;
                            p.y += dy;
                        }
                    }
                }
                stack.pop();
                Ok(())
            }
        }
    }

    /// A glyph's bitmap placement at `size26` and the segments it flattens to: the bitmap
    /// is the header box scaled, plus a two-pixel margin, with the baseline at the rounded
    /// scaled ascender. A bitmap past `4s × 4s` is `GlyphTooExpensive` before flattening.
    pub fn layout_glyph(&self, outline: &Outline, glyph: u16, size26: i32) -> Result<GlyphLayout, FontError> {
        if !(64..=64 * 1024).contains(&size26) {
            return Err(FontError::BadSize);
        }
        let size = size26 as f32 / 64.0;
        let scale = size / self.head.units_per_em as f32;
        if outline.ends.is_empty() {
            let advance = self.advance_units(glyph) as f32 * scale;
            return Ok(GlyphLayout::Empty { advance: ceil(advance) as usize });
        }
        let b = self.placement([outline.x_min, outline.y_min, outline.x_max, outline.y_max], scale);
        if b.width as f32 > 4.0 * size || b.height as f32 > 4.0 * size {
            return Err(FontError::GlyphTooExpensive);
        }
        let (x_min, y_max) = (outline.x_min as f32 * scale, outline.y_max as f32 * scale);
        let segments = flatten(outline, scale, -floor(x_min), ceil(y_max))?;
        Ok(GlyphLayout::Outline { segments, width: b.width, height: b.height, x_offset: b.x_offset, y_offset: b.y_offset })
    }

    /// The bitmap rectangle of a glyph whose header box is `[x_min, y_min, x_max, y_max]`, at
    /// `scale` pixels per unit.
    fn placement(&self, [x_min, y_min, x_max, y_max]: [i16; 4], scale: f32) -> GlyphBox {
        let baseline = (self.hhea.ascender as f32 * scale + 0.5) as i32;
        let (x_min, y_min) = (x_min as f32 * scale, y_min as f32 * scale);
        let (x_max, y_max) = (x_max as f32 * scale, y_max as f32 * scale);
        GlyphBox {
            x_offset: floor(x_min) as i32,
            y_offset: baseline - ceil(y_max) as i32,
            width: ceil(x_max - x_min) as usize + 2,
            height: ceil(y_max - y_min) as usize + 2,
        }
    }

    /// Where a glyph's bitmap would lie at `size26`, from its record header alone (no
    /// outline, no loop): the rectangle `layout_glyph` gives a glyph with contours. `None`
    /// for a glyph without contours, which draws nothing.
    pub fn glyph_box(&self, glyph: u16, size26: i32) -> Result<Option<GlyphBox>, FontError> {
        if !(64..=64 * 1024).contains(&size26) {
            return Err(FontError::BadSize);
        }
        let Some(rec) = self.record(glyph)? else { return Ok(None) };
        let h = |o: usize| rec.get(o..o + 2).map(|v| i16::from_be_bytes([v[0], v[1]])).ok_or(FontError::BadGlyph);
        if h(0)? == 0 {
            return Ok(None);
        }
        let scale = size26 as f32 / 64.0 / self.head.units_per_em as f32;
        Ok(Some(self.placement([h(2)?, h(4)?, h(6)?, h(8)?], scale)))
    }

    /// Rasterize a glyph at `pixel_size` (section 8.3); `BadSize` outside 1 to 1,024.
    pub fn rasterize_glyph(&self, glyph: u16, pixel_size: f32) -> Result<GlyphBitmap, FontError> {
        self.rasterize_glyph26(glyph, Font::size26(pixel_size)?)
    }

    /// Rasterize a glyph at a 26.6 size.
    pub fn rasterize_glyph26(&self, glyph: u16, size26: i32) -> Result<GlyphBitmap, FontError> {
        if size26 < 64 || size26 > 64 * 1024 {
            return Err(FontError::BadSize);
        }
        let outline = self.glyph_outline(glyph)?;
        match self.layout_glyph(&outline, glyph, size26)? {
            GlyphLayout::Empty { advance } => Ok(GlyphBitmap { width: advance, height: 0, x_offset: 0, y_offset: 0, coverage: Vec::new() }),
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => rasterize(&segments, width, height, x_offset, y_offset),
        }
    }

    /// Rasterize a glyph for LCD subpixel text (three samples per pixel).
    pub fn rasterize_glyph_subpixel(&self, glyph: u16, pixel_size: f32) -> Result<SubpixelBitmap, FontError> {
        let size26 = Font::size26(pixel_size)?;
        let outline = self.glyph_outline(glyph)?;
        match self.layout_glyph(&outline, glyph, size26)? {
            GlyphLayout::Empty { advance } => Ok(SubpixelBitmap { width: advance, height: 0, x_offset: 0, y_offset: 0, coverage: Vec::new() }),
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => rasterize_subpixel(&segments, width, height, x_offset, y_offset),
        }
    }
}

/// A glyph bitmap's offset from the pen position and its size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphBox {
    pub x_offset: i32,
    pub y_offset: i32,
    pub width: usize,
    pub height: usize,
}

/// Where a glyph's bitmap goes and what it is drawn from (`Font::layout_glyph`).
#[derive(Debug, Clone)]
pub enum GlyphLayout {
    /// No outline: a zero-height bitmap as wide as the advance.
    Empty { advance: usize },
    Outline { segments: Vec<outline::LineSegment>, width: usize, height: usize, x_offset: i32, y_offset: i32 },
}

/// An affine map `[a, b, c, d, e, f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Clone, Copy)]
struct Affine([f32; 6]);

const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

impl Affine {
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// `self` after `inner`: a point is mapped by `inner`, then by `self`.
    fn then(&self, inner: Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, u, v] = inner.0;
        Affine([a * p + c * q, b * p + d * q, a * r + c * s, b * r + d * s, a * u + c * v + e, b * u + d * v + f])
    }
}

/// Points and components counted across one glyph's expansion, and the point limit for
/// the outermost glyph's kind.
struct Budget {
    points: usize,
    components: usize,
    limit: usize,
}

impl Budget {
    fn add_points(&mut self, n: usize) -> Result<(), FontError> {
        self.points += n;
        if self.points > self.limit {
            return Err(FontError::TooManyPoints);
        }
        Ok(())
    }
}

/// A font with a cache of rasterized glyphs (section 8.4).
pub struct CachedFont {
    font: Font,
    cache: GlyphCache,
    max_bytes: usize,
    /// A bitmap larger than the whole byte budget, returned without being cached.
    oversize: Option<GlyphBitmap>,
}

impl CachedFont {
    /// A cache of at most `capacity` glyphs.
    pub fn new(font: Font, capacity: usize) -> CachedFont {
        CachedFont { font, cache: GlyphCache::new(capacity, usize::MAX), max_bytes: usize::MAX, oversize: None }
    }

    /// The contract's cache: at most 256 glyphs and 4 MiB of bitmaps.
    pub fn with_default_cache(font: Font) -> CachedFont {
        CachedFont {
            font,
            cache: GlyphCache::new(cache::DEFAULT_ENTRIES, cache::DEFAULT_BYTES),
            max_bytes: cache::DEFAULT_BYTES,
            oversize: None,
        }
    }

    pub fn font(&self) -> &Font {
        &self.font
    }

    pub fn metrics(&self, pixel_size: f32) -> ScaledMetrics {
        self.font.metrics(pixel_size)
    }

    pub fn glyph_index(&self, ch: char) -> u16 {
        self.font.glyph_index(ch)
    }

    pub fn advance_width(&self, glyph: u16, pixel_size: f32) -> f32 {
        self.font.advance_width(glyph, pixel_size)
    }

    pub fn kern(&self, left: u16, right: u16, pixel_size: f32) -> f32 {
        self.font.kern(left, right, pixel_size)
    }

    /// The glyph at `pixel_size`, from the cache when it holds it; identical to a fresh
    /// rasterization either way.
    pub fn rasterize_glyph(&mut self, glyph: u16, pixel_size: f32) -> Result<&GlyphBitmap, FontError> {
        let size26 = Font::size26(pixel_size)?;
        if self.cache.get(glyph, size26).is_some() {
            return self.cache.get_quiet(glyph, size26).ok_or(FontError::GlyphTooExpensive);
        }
        let bitmap = self.font.rasterize_glyph26(glyph, size26)?;
        if bitmap.coverage.len() > self.max_bytes {
            return Ok(self.oversize.insert(bitmap));
        }
        Ok(self.cache.insert(glyph, size26, bitmap))
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Drop every cached glyph (a font or size change).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.oversize = None;
    }

    pub fn clear_cache(&mut self) {
        self.clear();
    }
}
