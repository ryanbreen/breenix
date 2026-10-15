//! Pixel verdicts over canonical BGRX frames (BreenGL contract sections 2, 20 and 23).
//! No drawing, codecs, device access or floating point participates in a comparison.
#![no_std]

extern crate alloc;
use alloc::vec::Vec;
pub mod sha256;
pub use sha256::{sha256, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bgr,
    Rgb,
    Bgrx,
    Rgbx,
}
impl Format {
    pub const fn bpp(self) -> usize {
        match self {
            Self::Bgr | Self::Rgb => 3,
            Self::Bgrx | Self::Rgbx => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    TooLarge,
    Stride,
    SizeOverflow,
    Truncated,
    Geometry,
    Region,
    Mask,
    EmptyDomain,
    MaskCoverage,
    Tolerance,
    MissingReason,
    Predicate,
}

/// Validated, borrowed pixels; padding and X never contribute to a verdict.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    bytes: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: Format,
}
impl<'a> Frame<'a> {
    pub fn new(
        bytes: &'a [u8],
        width: usize,
        height: usize,
        stride: usize,
        format: Format,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::Empty);
        }
        if width > 16384 || height > 16384 {
            return Err(Error::TooLarge);
        }
        let row = width.checked_mul(format.bpp()).ok_or(Error::SizeOverflow)?;
        if stride < row {
            return Err(Error::Stride);
        }
        let size = stride
            .checked_mul(height)
            .filter(|&n| n <= i32::MAX as usize)
            .ok_or(Error::SizeOverflow)?;
        if bytes.len() < size {
            return Err(Error::Truncated);
        }
        Ok(Self {
            bytes,
            width,
            height,
            stride,
            format,
        })
    }
    /// B,G,R,FF. Coordinates outside the frame return None.
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let p = y * self.stride + x * self.format.bpp();
        let (a, b, c) = (self.bytes[p], self.bytes[p + 1], self.bytes[p + 2]);
        Some(match self.format {
            Format::Bgr | Format::Bgrx => [a, b, c, 255],
            _ => [c, b, a, 255],
        })
    }
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        self.each(|p| out.extend_from_slice(&p));
        out
    }
    /// Hash only the canonical row-major pixel bytes; no stride or geometry header.
    pub fn sha256(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        self.each(|p| hash.update(&p));
        hash.finish()
    }
    fn each(&self, mut f: impl FnMut([u8; 4])) {
        for y in 0..self.height {
            for x in 0..self.width {
                f(self.pixel(x, y).unwrap());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}
impl Rect {
    fn valid(self, width: usize, height: usize) -> bool {
        self.w != 0
            && self.h != 0
            && self.x.checked_add(self.w).is_some_and(|n| n <= width)
            && self.y.checked_add(self.h).is_some_and(|n| n <= height)
    }
    pub fn contains(self, x: usize, y: usize) -> bool {
        x >= self.x && y >= self.y && x - self.x < self.w && y - self.y < self.h
    }
    fn encloses(self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x - self.x <= self.w
            && other.y - self.y <= self.h
            && other.w <= self.w - (other.x - self.x)
            && other.h <= self.h - (other.y - self.y)
    }
}
#[derive(Clone, Copy)]
pub struct Mask<'a> {
    pub rect: Rect,
    pub reason: &'a str,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Tolerance {
    pub channel: u8,
    pub max_differing: usize,
}
#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Empty means the complete frame. Overlapping regions are compared once.
    pub regions: &'a [Rect],
    pub masks: &'a [Mask<'a>],
    pub tolerance: Tolerance,
    pub reason: &'a str,
}
impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            regions: &[],
            masks: &[],
            tolerance: Tolerance::default(),
            reason: "",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difference {
    pub x: usize,
    pub y: usize,
    pub expected: [u8; 4],
    pub actual: [u8; 4],
    pub delta: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub compared: usize,
    pub differing: usize,
    pub max_delta: u8,
    pub first: Option<Difference>,
    pub bound: usize,
}
impl Comparison {
    pub fn passed(&self) -> bool {
        self.differing <= self.bound
    }
}
fn delta(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap()
}
fn domain(options: &Options<'_>, x: usize, y: usize) -> bool {
    (options.regions.is_empty() || options.regions.iter().any(|r| r.contains(x, y)))
        && !options.masks.iter().any(|m| m.rect.contains(x, y))
}
fn validate(frame: Frame<'_>, options: &Options<'_>) -> Result<usize, Error> {
    if options.tolerance.channel > 8 {
        return Err(Error::Tolerance);
    }
    if (options.tolerance.channel != 0 || options.tolerance.max_differing != 0)
        && options.reason.trim().is_empty()
    {
        return Err(Error::MissingReason);
    }
    let full = [Rect {
        x: 0,
        y: 0,
        w: frame.width,
        h: frame.height,
    }];
    let regions = if options.regions.is_empty() {
        &full[..]
    } else {
        options.regions
    };
    for r in regions {
        if !r.valid(frame.width, frame.height) {
            return Err(Error::Region);
        }
    }
    for m in options.masks {
        if m.reason.trim().is_empty() {
            return Err(Error::MissingReason);
        }
        if !m.rect.valid(frame.width, frame.height) || !regions.iter().any(|r| r.encloses(m.rect)) {
            return Err(Error::Mask);
        }
    }
    for r in regions {
        let mut remaining = 0usize;
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                remaining += usize::from(!options.masks.iter().any(|m| m.rect.contains(x, y)));
            }
        }
        if remaining == 0 {
            return Err(Error::EmptyDomain);
        }
        if remaining * 4 < r.w * r.h * 3 {
            return Err(Error::MaskCoverage);
        }
    }
    let mut area = 0;
    for y in 0..frame.height {
        for x in 0..frame.width {
            area += usize::from(domain(options, x, y));
        }
    }
    if options.tolerance.max_differing > area / 1000 {
        return Err(Error::Tolerance);
    }
    Ok(area)
}

pub fn exact(expected: Frame<'_>, actual: Frame<'_>) -> Result<Comparison, Error> {
    compare(expected, actual, Options::default())
}
/// Invalid or vacuous checks return an error, never a passing comparison.
pub fn compare(
    expected: Frame<'_>,
    actual: Frame<'_>,
    options: Options<'_>,
) -> Result<Comparison, Error> {
    if expected.width != actual.width || expected.height != actual.height {
        return Err(Error::Geometry);
    }
    let area = validate(expected, &options)?;
    let mut out = Comparison {
        compared: area,
        differing: 0,
        max_delta: 0,
        first: None,
        bound: options.tolerance.max_differing,
    };
    for y in 0..expected.height {
        for x in 0..expected.width {
            if !domain(&options, x, y) {
                continue;
            }
            let (a, b) = (expected.pixel(x, y).unwrap(), actual.pixel(x, y).unwrap());
            let d = delta(a, b);
            out.max_delta = out.max_delta.max(d);
            if d > options.tolerance.channel {
                out.differing += 1;
                out.first.get_or_insert(Difference {
                    x,
                    y,
                    expected: a,
                    actual: b,
                    delta: d,
                });
            }
        }
    }
    Ok(out)
}

/// BGRX heat ramp: equal is black; nonzero d is (B=0,G=255-d,R=255,X=255).
pub fn heat_color(d: u8) -> [u8; 4] {
    if d == 0 {
        [0, 0, 0, 255]
    } else {
        [0, 255 - d, 255, 255]
    }
}
pub fn heatmap(expected: Frame<'_>, actual: Frame<'_>) -> Result<Vec<u8>, Error> {
    if expected.width != actual.width || expected.height != actual.height {
        return Err(Error::Geometry);
    }
    let mut out = Vec::with_capacity(expected.width * expected.height * 4);
    for y in 0..expected.height {
        for x in 0..expected.width {
            out.extend_from_slice(&heat_color(delta(
                expected.pixel(x, y).unwrap(),
                actual.pixel(x, y).unwrap(),
            )));
        }
    }
    Ok(out)
}

/// Integer box filter: floor-partition source bins, round each channel's mean half up.
/// Upscaling is refused so every destination pixel has a nonempty source bin.
pub fn thumbnail(frame: Frame<'_>, width: usize, height: usize) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 {
        return Err(Error::Empty);
    }
    if width > frame.width || height > frame.height {
        return Err(Error::Geometry);
    }
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let (x0, x1) = (x * frame.width / width, (x + 1) * frame.width / width);
            let (y0, y1) = (y * frame.height / height, (y + 1) * frame.height / height);
            let n = ((x1 - x0) * (y1 - y0)) as u64;
            let mut sum = [0u64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = frame.pixel(sx, sy).unwrap();
                    for c in 0..3 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            out.extend_from_slice(&[
                ((sum[0] + n / 2) / n) as u8,
                ((sum[1] + n / 2) / n) as u8,
                ((sum[2] + n / 2) / n) as u8,
                255,
            ]);
        }
    }
    Ok(out)
}
