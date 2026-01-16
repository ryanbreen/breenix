//! Font rendering types and utilities.
//!
//! Provides bitmap font support using the noto-sans-mono-bitmap crate.
//! This module abstracts over the underlying font library to provide
//! a clean API for text rendering in the graphics stack.

// This is a public API module - functions are intentionally available for external use
#![allow(dead_code)]

use noto_sans_mono_bitmap::{
    get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar,
};

/// Available font sizes.
/// Currently only Size16 is enabled in Cargo.toml features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontSize {
    /// 16 pixel height (default, currently enabled)
    #[default]
    Size16,
}

impl FontSize {
    /// Convert to the underlying RasterHeight.
    fn to_raster_height(self) -> RasterHeight {
        match self {
            FontSize::Size16 => RasterHeight::Size16,
        }
    }

    /// Get the pixel height for this font size.
    pub fn height(self) -> usize {
        match self {
            FontSize::Size16 => 16,
        }
    }
}

/// Font weight options.
/// Currently only Regular is enabled in Cargo.toml features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Weight {
    /// Regular weight (default, currently enabled)
    #[default]
    Regular,
}

impl Weight {
    /// Convert to the underlying FontWeight.
    fn to_font_weight(self) -> FontWeight {
        match self {
            Weight::Regular => FontWeight::Regular,
        }
    }
}

/// Font configuration combining size and weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Font {
    /// Font size
    pub size: FontSize,
    /// Font weight
    pub weight: Weight,
}

impl Font {
    /// Create a new font with the specified size and weight.
    pub const fn new(size: FontSize, weight: Weight) -> Self {
        Self { size, weight }
    }

    /// Get the default font (16px Regular).
    pub const fn default_font() -> Self {
        Self {
            size: FontSize::Size16,
            weight: Weight::Regular,
        }
    }

    /// Get metrics for this font configuration.
    pub fn metrics(&self) -> FontMetrics {
        let char_width = get_raster_width(
            self.weight.to_font_weight(),
            self.size.to_raster_height(),
        );
        FontMetrics {
            char_width,
            char_height: self.size.height(),
            line_spacing: 2,
            letter_spacing: 0,
        }
    }

    /// Get the glyph for a character, or None if not available.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        get_raster(c, self.weight.to_font_weight(), self.size.to_raster_height())
            .map(|rc| Glyph::from_rasterized(rc))
    }

    /// Get the replacement glyph ('?') for unknown characters.
    pub fn replacement_glyph(&self) -> Glyph {
        self.glyph('?').expect("Font should have '?' character")
    }

    /// Get the glyph for a character, using replacement if not found.
    pub fn glyph_or_replacement(&self, c: char) -> Glyph {
        self.glyph(c).unwrap_or_else(|| self.replacement_glyph())
    }
}

/// Metrics for a font configuration.
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    /// Width of each character in pixels
    pub char_width: usize,
    /// Height of each character in pixels
    pub char_height: usize,
    /// Additional vertical space between lines
    pub line_spacing: usize,
    /// Additional horizontal space between characters
    pub letter_spacing: usize,
}

impl FontMetrics {
    /// Get the total line height (char height + line spacing).
    pub fn line_height(&self) -> usize {
        self.char_height + self.line_spacing
    }

    /// Get the total character advance (char width + letter spacing).
    pub fn char_advance(&self) -> usize {
        self.char_width + self.letter_spacing
    }
}

/// Character glyph data - a wrapper around RasterizedChar.
pub struct Glyph {
    /// The rasterized character data
    rasterized: RasterizedChar,
}

impl Glyph {
    /// Create a Glyph from a RasterizedChar.
    fn from_rasterized(rc: RasterizedChar) -> Self {
        Self { rasterized: rc }
    }

    /// Get the width of this glyph in pixels.
    pub fn width(&self) -> usize {
        self.rasterized.width()
    }

    /// Get the height of this glyph in pixels.
    pub fn height(&self) -> usize {
        self.rasterized.height()
    }

    /// Get the raster data as rows of intensity values (0-255).
    /// Each row is a slice of bytes, one per pixel column.
    pub fn raster(&self) -> &[[u8; 8]] {
        // noto-sans-mono-bitmap returns fixed-width arrays
        self.rasterized.raster()
    }

    /// Iterate over the glyph pixels with coordinates and intensity.
    /// Yields (x, y, intensity) for each pixel.
    pub fn pixels(&self) -> impl Iterator<Item = (usize, usize, u8)> + '_ {
        let width = self.width();
        self.raster().iter().enumerate().flat_map(move |(y, row)| {
            row.iter()
                .take(width)
                .enumerate()
                .map(move |(x, &intensity)| (x, y, intensity))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_font_has_expected_metrics() {
        let font = Font::default_font();
        let metrics = font.metrics();
        assert_eq!(metrics.char_height, 16);
        assert!(metrics.char_width > 0);
    }

    #[test]
    fn can_get_glyph_for_ascii() {
        let font = Font::default_font();
        let glyph = font.glyph('A');
        assert!(glyph.is_some());
        let g = glyph.unwrap();
        assert!(g.width() > 0);
        assert_eq!(g.height(), 16);
    }

    #[test]
    fn replacement_glyph_exists() {
        let font = Font::default_font();
        let glyph = font.replacement_glyph();
        assert!(glyph.width() > 0);
    }

    #[test]
    fn glyph_pixels_iterator_yields_data() {
        let font = Font::default_font();
        let glyph = font.glyph('X').unwrap();
        let pixels: Vec<_> = glyph.pixels().collect();
        assert!(!pixels.is_empty());
        // X should have some non-zero intensity pixels
        assert!(pixels.iter().any(|(_, _, intensity)| *intensity > 0));
    }
}
