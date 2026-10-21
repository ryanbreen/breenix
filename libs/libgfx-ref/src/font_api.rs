//! CPU glyph rasterization for independent suite expectations.
use alloc::vec::Vec;
use core::ops::Deref;
use libfont::{FontError, GlyphLayout, ScaledMetrics};
use crate::cache::{self, GlyphCache, CacheStats};
use crate::rasterizer::{rasterize, rasterize_subpixel, GlyphBitmap, SubpixelBitmap};

pub struct Font(libfont::Font);
impl Deref for Font { type Target = libfont::Font; fn deref(&self) -> &Self::Target { &self.0 } }
impl Font {
    pub fn parse(bytes: &[u8]) -> Result<Self, FontError> { libfont::Font::parse(bytes).map(Self) }
    pub fn size26(size: f32) -> Result<i32, FontError> { libfont::Font::size26(size) }
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
/// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.oversize = None;
    }

    pub fn clear_cache(&mut self) {
        self.clear();
    }
}
