//! Why a font or one of its glyphs was refused (docs/gui/breengel.md section 10).

use core::fmt;

/// A parser or rasterizer refusal. Font-scoped errors come from `Font::parse`; glyph-scoped
/// ones from the calls that read one glyph, and leave every other glyph usable.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// A GPU raster operation failed with this negated errno.
    Device(i64),
    /// A read past the end of the file or of a table.
    Truncated,
    /// A table record whose offset or length runs past the file, or a table whose fixed
    /// fields are out of their range.
    BadTable([u8; 4]),
    /// A required table is absent.
    MissingTable([u8; 4]),
    /// `unitsPerEm` of 0 or above 16,384.
    BadUnitsPerEm,
    /// A glyph whose `loca` entry lies beyond the `glyf` table, or a glyph id past the font.
    GlyphOutOfRange,
    /// `loca` entries that decrease.
    BadLoca,
    /// A `loca` length other than `numGlyphs + 1` entries.
    GlyphCountMismatch,
    /// A malformed format 4 or format 12 `cmap` subtable.
    BadCmap,
    /// Contour end points that do not increase, or a flag or coordinate run past the glyph.
    BadGlyph,
    /// More points than `maxp` declares, or above 65,535.
    TooManyPoints,
    /// Compound nesting deeper than 8, or more than 256 components.
    CompoundTooDeep,
    /// A compound that contains itself, directly or through another.
    CompoundCycle,
    /// A component index at or above `numGlyphs`.
    BadComponent,
    /// A short `hmtx`, or `numberOfHMetrics` of 0 or above `numGlyphs`.
    BadHmtx,
    /// Past the flattening bounds of section 7.
    GlyphTooComplex,
    /// Past the rasterization bounds of section 7.
    GlyphTooExpensive,
    /// A size below 1 or above 1,024 pixels per em.
    BadSize,
}

fn tag(f: &mut fmt::Formatter<'_>, name: &str, t: &[u8; 4]) -> fmt::Result {
    f.write_str(name)?;
    f.write_str("(")?;
    for &b in t {
        let c = if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' };
        fmt::Write::write_char(f, c)?;
    }
    f.write_str(")")
}

/// The variant's name, with the table tag where it has one: `BadTable(glyf)`. The corpus
/// index (`fonts-corpus/index.txt`) names errors this way.
impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::Device(e) => write!(f, "GPU glyph error {e}"),
            FontError::BadTable(t) => tag(f, "BadTable", t),
            FontError::MissingTable(t) => tag(f, "MissingTable", t),
            other => f.write_str(match other {
                FontError::Truncated => "Truncated",
                FontError::BadUnitsPerEm => "BadUnitsPerEm",
                FontError::GlyphOutOfRange => "GlyphOutOfRange",
                FontError::BadLoca => "BadLoca",
                FontError::GlyphCountMismatch => "GlyphCountMismatch",
                FontError::BadCmap => "BadCmap",
                FontError::BadGlyph => "BadGlyph",
                FontError::TooManyPoints => "TooManyPoints",
                FontError::CompoundTooDeep => "CompoundTooDeep",
                FontError::CompoundCycle => "CompoundCycle",
                FontError::BadComponent => "BadComponent",
                FontError::BadHmtx => "BadHmtx",
                FontError::GlyphTooComplex => "GlyphTooComplex",
                FontError::GlyphTooExpensive => "GlyphTooExpensive",
                _ => "BadSize",
            }),
        }
    }
}

impl fmt::Debug for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
