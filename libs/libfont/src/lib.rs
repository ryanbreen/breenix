//! TrueType font parser and anti-aliased rasterizer.
//!
//! `#![no_std]` + `extern crate alloc`. Zero external dependencies.
//!
//! Parses .ttf files, extracts glyph outlines, and rasterizes them into
//! coverage bitmaps suitable for alpha-blended text rendering.

#![no_std]
extern crate alloc;

pub mod reader;
pub mod tables;
pub mod outline;
pub mod rasterizer;
pub mod cache;
mod float;

use alloc::string::String;
use crate::tables::TableDirectory;
use crate::tables::head::HeadTable;
use crate::tables::hhea::HheaTable;
use crate::tables::hmtx::HmtxTable;
use crate::tables::maxp;
use crate::tables::cmap::CmapTable;
use crate::tables::loca::LocaTable;
use crate::tables::kern::KernTable;
use crate::tables::glyf;
use crate::outline::flatten_glyph;
use crate::rasterizer::{rasterize, GlyphBitmap};
use crate::cache::GlyphCache;
use crate::float::{floor, ceil};

pub use crate::rasterizer::GlyphBitmap as Bitmap;

/// Scaled font metrics in pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct ScaledMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub line_height: f32,
}

/// Parsed TrueType font. Borrows the .ttf byte slice.
pub struct Font<'a> {
    head: HeadTable,
    hhea: HheaTable,
    cmap: CmapTable<'a>,
    hmtx_data: &'a [u8],
    loca_data: &'a [u8],
    glyf_data: &'a [u8],
    kern: Option<KernTable<'a>>,
    num_h_metrics: u16,
    index_to_loc_format: i16,
}

impl<'a> Font<'a> {
    /// Parse a TrueType font from raw .ttf data.
    pub fn parse(data: &'a [u8]) -> Result<Self, String> {
        let dir = TableDirectory::parse(data)?;

        let head_data = dir.table_data(data, b"head")
            .ok_or_else(|| String::from("missing head table"))?;
        let head = HeadTable::parse(head_data)?;

        let hhea_data = dir.table_data(data, b"hhea")
            .ok_or_else(|| String::from("missing hhea table"))?;
        let hhea = HheaTable::parse(hhea_data)?;

        let maxp_data = dir.table_data(data, b"maxp")
            .ok_or_else(|| String::from("missing maxp table"))?;
        let _ = maxp::MaxpTable::parse(maxp_data)?;

        let cmap_data = dir.table_data(data, b"cmap")
            .ok_or_else(|| String::from("missing cmap table"))?;
        let cmap = CmapTable::parse(cmap_data)?;

        let hmtx_data = dir.table_data(data, b"hmtx")
            .ok_or_else(|| String::from("missing hmtx table"))?;

        let loca_data = dir.table_data(data, b"loca")
            .ok_or_else(|| String::from("missing loca table"))?;

        let glyf_data = dir.table_data(data, b"glyf")
            .ok_or_else(|| String::from("missing glyf table"))?;

        let kern = dir.table_data(data, b"kern").and_then(KernTable::parse);

        Ok(Self {
            head,
            hhea,
            cmap,
            hmtx_data,
            loca_data,
            glyf_data,
            kern,
            num_h_metrics: hhea.num_h_metrics,
            index_to_loc_format: head.index_to_loc_format,
        })
    }

    /// Get scaled metrics for a given pixel size.
    pub fn metrics(&self, pixel_size: f32) -> ScaledMetrics {
        let scale = pixel_size / self.head.units_per_em as f32;
        let ascender = self.hhea.ascender as f32 * scale;
        let descender = self.hhea.descender as f32 * scale;
        let line_gap = self.hhea.line_gap as f32 * scale;
        ScaledMetrics {
            ascender,
            descender,
            line_gap,
            line_height: ascender - descender + line_gap,
        }
    }

    /// Map a Unicode codepoint to a glyph index.
    pub fn glyph_index(&self, ch: char) -> u16 {
        self.cmap.glyph_index(ch as u32)
    }

    /// Get the advance width of a glyph in pixels.
    pub fn advance_width(&self, glyph_index: u16, pixel_size: f32) -> f32 {
        let hmtx = HmtxTable::new(self.hmtx_data, self.num_h_metrics);
        let scale = pixel_size / self.head.units_per_em as f32;
        hmtx.advance_width(glyph_index) as f32 * scale
    }

    /// Get the kerning value between two glyphs in pixels.
    pub fn kern(&self, left: u16, right: u16, pixel_size: f32) -> f32 {
        match &self.kern {
            Some(kern) => {
                let scale = pixel_size / self.head.units_per_em as f32;
                kern.kern_value(left, right) as f32 * scale
            }
            None => 0.0,
        }
    }

    /// Rasterize a single glyph at the given pixel size.
    pub fn rasterize_glyph(
        &self,
        glyph_index: u16,
        pixel_size: f32,
    ) -> Result<GlyphBitmap, String> {
        let scale = pixel_size / self.head.units_per_em as f32;
        let loca = LocaTable::new(self.loca_data, self.index_to_loc_format);
        let hmtx = HmtxTable::new(self.hmtx_data, self.num_h_metrics);

        // Get glyph offset — None means empty glyph (e.g., space)
        let glyph_offset = match loca.glyph_offset(glyph_index) {
            Some(off) => off,
            None => {
                // Empty glyph — return a zero-size bitmap
                let advance = hmtx.advance_width(glyph_index) as f32 * scale;
                return Ok(GlyphBitmap {
                    width: ceil(advance) as usize,
                    height: 0,
                    x_offset: 0,
                    y_offset: 0,
                    coverage: alloc::vec::Vec::new(),
                });
            }
        };

        let simple_glyph = self.resolve_glyph(glyph_offset)?;

        let ascender = self.hhea.ascender as f32 * scale;
        // Fixed baseline position in cell — same for ALL glyphs at this size.
        // Using round ensures the baseline doesn't shift per-glyph.
        let baseline = (ascender + 0.5) as i32;

        // Calculate bitmap bounds from glyph bounds
        let x_min = simple_glyph.x_min as f32 * scale;
        let y_min = simple_glyph.y_min as f32 * scale;
        let x_max = simple_glyph.x_max as f32 * scale;
        let y_max = simple_glyph.y_max as f32 * scale;

        let bmp_x_offset = floor(x_min) as i32;
        // Position bitmap so its internal baseline (at row ceil(y_max))
        // aligns with the fixed cell baseline. This guarantees all glyphs
        // share the same baseline regardless of their individual y_max.
        let bmp_y_offset = baseline - ceil(y_max) as i32;

        let bmp_width = ceil(x_max - x_min) as usize + 2; // +2 for safety margin
        let bmp_height = ceil(y_max - y_min) as usize + 2;

        if bmp_width == 0 || bmp_height == 0 {
            return Ok(GlyphBitmap {
                width: 0,
                height: 0,
                x_offset: 0,
                y_offset: 0,
                coverage: alloc::vec::Vec::new(),
            });
        }

        // Flatten the glyph outline into line segments
        let x_off = -floor(x_min);
        let y_off = ceil(y_max); // top of bitmap in font coords (y-flipped)
        let segments = flatten_glyph(&simple_glyph, scale, x_off, y_off);

        Ok(rasterize(&segments, bmp_width, bmp_height, bmp_x_offset, bmp_y_offset))
    }

    fn resolve_glyph(
        &self,
        offset: u32,
    ) -> Result<glyf::SimpleGlyph, String> {
        // First try simple parse
        if let Some(glyph) = glyf::parse_glyph(self.glyf_data, offset)? {
            if !glyph.contours.is_empty() {
                return Ok(glyph);
            }
            // Might be compound — try compound resolution
            let glyf_data = self.glyf_data;
            let index_to_loc_format = self.index_to_loc_format;
            let loca_data = self.loca_data;

            let result = glyf::resolve_compound(self.glyf_data, offset, &|comp_idx| {
                let comp_loca = LocaTable::new(loca_data, index_to_loc_format);
                let comp_off = comp_loca.glyph_offset(comp_idx)?;
                glyf::parse_glyph(glyf_data, comp_off).ok().flatten()
            })?;
            match result {
                Some(g) if !g.contours.is_empty() => Ok(g),
                _ => Ok(glyph),
            }
        } else {
            Err(String::from("failed to parse glyph"))
        }
    }
}

/// Diagnostic info for a glyph (for debugging rasterization issues).
pub struct GlyphDebugInfo {
    pub loca_offset: Option<u32>,
    pub num_contours: i16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub total_points: usize,
    pub units_per_em: u16,
}

/// Detailed rasterization diagnostics — shows all intermediate values.
pub struct RasterDebugInfo {
    pub pixel_size: f32,
    pub units_per_em: u16,
    pub scale: f32,
    pub glyph_x_min: i16,
    pub glyph_y_min: i16,
    pub glyph_x_max: i16,
    pub glyph_y_max: i16,
    pub x_min_scaled: f32,
    pub y_min_scaled: f32,
    pub x_max_scaled: f32,
    pub y_max_scaled: f32,
    pub bmp_width: usize,
    pub bmp_height: usize,
    pub bmp_x_offset: i32,
    pub bmp_y_offset: i32,
    pub baseline: i32,
    pub num_contours: usize,
    pub num_points: usize,
    pub num_segments: usize,
    pub nonzero_coverage: usize,
}

impl<'a> Font<'a> {
    /// Full rasterization diagnostic — returns all intermediate values.
    pub fn debug_rasterize(&self, glyph_index: u16, pixel_size: f32) -> Result<RasterDebugInfo, String> {
        let scale = pixel_size / self.head.units_per_em as f32;
        let loca = LocaTable::new(self.loca_data, self.index_to_loc_format);

        let glyph_offset = loca.glyph_offset(glyph_index)
            .ok_or_else(|| String::from("no glyph offset"))?;

        let simple_glyph = self.resolve_glyph(glyph_offset)?;

        let ascender = self.hhea.ascender as f32 * scale;
        let baseline = (ascender + 0.5) as i32;

        let x_min_s = simple_glyph.x_min as f32 * scale;
        let y_min_s = simple_glyph.y_min as f32 * scale;
        let x_max_s = simple_glyph.x_max as f32 * scale;
        let y_max_s = simple_glyph.y_max as f32 * scale;

        let bmp_x_offset = floor(x_min_s) as i32;
        let bmp_y_offset = baseline - ceil(y_max_s) as i32;
        let bmp_width = ceil(x_max_s - x_min_s) as usize + 2;
        let bmp_height = ceil(y_max_s - y_min_s) as usize + 2;

        let x_off = -floor(x_min_s);
        let y_off = ceil(y_max_s);
        let segments = flatten_glyph(&simple_glyph, scale, x_off, y_off);

        let num_contours = simple_glyph.contours.len();
        let num_points: usize = simple_glyph.contours.iter().map(|c| c.len()).sum();

        let bitmap = if bmp_width > 0 && bmp_height > 0 && !segments.is_empty() {
            rasterize(&segments, bmp_width, bmp_height, bmp_x_offset, bmp_y_offset)
        } else {
            rasterizer::GlyphBitmap {
                width: bmp_width, height: bmp_height,
                x_offset: bmp_x_offset, y_offset: bmp_y_offset,
                coverage: alloc::vec![0; bmp_width * bmp_height],
            }
        };
        let nonzero_coverage = bitmap.coverage.iter().filter(|&&v| v > 0).count();

        Ok(RasterDebugInfo {
            pixel_size,
            units_per_em: self.head.units_per_em,
            scale,
            glyph_x_min: simple_glyph.x_min,
            glyph_y_min: simple_glyph.y_min,
            glyph_x_max: simple_glyph.x_max,
            glyph_y_max: simple_glyph.y_max,
            x_min_scaled: x_min_s,
            y_min_scaled: y_min_s,
            x_max_scaled: x_max_s,
            y_max_scaled: y_max_s,
            bmp_width,
            bmp_height,
            bmp_x_offset,
            bmp_y_offset,
            baseline,
            num_contours,
            num_points,
            num_segments: segments.len(),
            nonzero_coverage,
        })
    }

    /// Get raw glyph diagnostic info without rasterizing.
    pub fn debug_glyph(&self, glyph_index: u16) -> GlyphDebugInfo {
        let loca = LocaTable::new(self.loca_data, self.index_to_loc_format);
        let loca_offset = loca.glyph_offset(glyph_index);

        let (num_contours, x_min, y_min, x_max, y_max, total_points) = match loca_offset {
            Some(off) => {
                let off = off as usize;
                if off + 10 <= self.glyf_data.len() {
                    let mut r = reader::Reader::at(self.glyf_data, off);
                    let nc = r.read_i16().unwrap_or(0);
                    let xn = r.read_i16().unwrap_or(0);
                    let yn = r.read_i16().unwrap_or(0);
                    let xx = r.read_i16().unwrap_or(0);
                    let yx = r.read_i16().unwrap_or(0);
                    // Try to count points by parsing
                    let pts = match self.resolve_glyph(off as u32) {
                        Ok(g) => g.contours.iter().map(|c| c.len()).sum(),
                        Err(_) => 0,
                    };
                    (nc, xn, yn, xx, yx, pts)
                } else {
                    (0, 0, 0, 0, 0, 0)
                }
            }
            None => (0, 0, 0, 0, 0, 0),
        };

        GlyphDebugInfo {
            loca_offset,
            num_contours,
            x_min,
            y_min,
            x_max,
            y_max,
            total_points,
            units_per_em: self.head.units_per_em,
        }
    }
}

/// Font wrapper with built-in glyph bitmap cache.
pub struct CachedFont<'a> {
    font: Font<'a>,
    cache: GlyphCache,
}

impl<'a> CachedFont<'a> {
    pub fn new(font: Font<'a>, max_cache_entries: usize) -> Self {
        Self {
            font,
            cache: GlyphCache::new(max_cache_entries),
        }
    }

    pub fn metrics(&self, pixel_size: f32) -> ScaledMetrics {
        self.font.metrics(pixel_size)
    }

    pub fn glyph_index(&self, ch: char) -> u16 {
        self.font.glyph_index(ch)
    }

    pub fn advance_width(&self, glyph_index: u16, pixel_size: f32) -> f32 {
        self.font.advance_width(glyph_index, pixel_size)
    }

    pub fn kern(&self, left: u16, right: u16, pixel_size: f32) -> f32 {
        self.font.kern(left, right, pixel_size)
    }

    pub fn rasterize_glyph(
        &mut self,
        glyph_index: u16,
        pixel_size: f32,
    ) -> Result<&GlyphBitmap, String> {
        if self.cache.get(glyph_index, pixel_size).is_none() {
            let bitmap = self.font.rasterize_glyph(glyph_index, pixel_size)?;
            self.cache.insert(glyph_index, pixel_size, bitmap);
        }
        self.cache.get(glyph_index, pixel_size)
            .ok_or_else(|| alloc::string::String::from("cache lookup failed after insert"))
    }

    pub fn font(&self) -> &Font<'a> {
        &self.font
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FONT_DATA: &[u8] = include_bytes!("../../../fonts/DejaVuSansMono.ttf");

    #[test]
    fn parse_font() {
        Font::parse(FONT_DATA).expect("failed to parse DejaVuSansMono.ttf");
    }

    #[test]
    fn cmap_lookup() {
        let font = Font::parse(FONT_DATA).unwrap();
        let a_idx = font.glyph_index('A');
        assert!(a_idx > 0, "glyph index for 'A' should be nonzero, got {}", a_idx);
        let space_idx = font.glyph_index(' ');
        assert!(space_idx > 0, "glyph index for space should be nonzero, got {}", space_idx);
    }

    #[test]
    fn rasterize_a_16px() {
        let font = Font::parse(FONT_DATA).unwrap();
        let glyph_idx = font.glyph_index('A');
        let bitmap = font.rasterize_glyph(glyph_idx, 16.0).expect("rasterize 'A' failed");
        assert!(bitmap.width > 0, "bitmap width should be > 0");
        assert!(bitmap.height > 0, "bitmap height should be > 0");
        let nonzero_count = bitmap.coverage.iter().filter(|&&c| c > 0).count();
        assert!(nonzero_count > 0, "bitmap should have nonzero coverage pixels");
    }

    #[test]
    fn rasterize_space() {
        let font = Font::parse(FONT_DATA).unwrap();
        let glyph_idx = font.glyph_index(' ');
        let bitmap = font.rasterize_glyph(glyph_idx, 16.0).expect("rasterize space failed");
        assert_eq!(bitmap.height, 0, "space glyph should have zero height");
    }

    #[test]
    fn metrics_16px() {
        let font = Font::parse(FONT_DATA).unwrap();
        let m = font.metrics(16.0);
        assert!(m.ascender > 0.0, "ascender should be > 0, got {}", m.ascender);
        assert!(m.line_height > 0.0, "line_height should be > 0, got {}", m.line_height);
        assert!(m.descender < 0.0, "descender should be negative, got {}", m.descender);
    }

    #[test]
    fn advance_width_m_16px() {
        let font = Font::parse(FONT_DATA).unwrap();
        let glyph_idx = font.glyph_index('M');
        let advance = font.advance_width(glyph_idx, 16.0);
        assert!(advance >= 6.0 && advance <= 14.0,
            "advance width of 'M' at 16px should be reasonable (6-14px), got {}", advance);
    }

    #[test]
    fn cached_font_basic() {
        let font = Font::parse(FONT_DATA).unwrap();
        let mut cached = CachedFont::new(font, 128);
        let glyph_idx = cached.glyph_index('A');
        // First call populates cache
        let bmp1 = cached.rasterize_glyph(glyph_idx, 16.0).unwrap();
        let w1 = bmp1.width;
        let h1 = bmp1.height;
        // Second call hits cache
        let bmp2 = cached.rasterize_glyph(glyph_idx, 16.0).unwrap();
        assert_eq!(bmp2.width, w1);
        assert_eq!(bmp2.height, h1);
    }
}
