//! Frame comparison for UI checks, using the shared pixel policy.
pub use libpixcheck::{Comparison, Difference, Format, Frame, Mask, Options, Rect, Tolerance};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    SizeMismatch { expected: (usize, usize), actual: (usize, usize) },
    Invalid(libpixcheck::Error),
}

#[derive(Debug)]
pub struct Verdict {
    pub comparison: Comparison,
    /// Index of the declared region nearest the first differing pixel; ties use declaration order.
    pub closest_region: Option<usize>,
}

impl Verdict {
    pub fn failure_class(&self) -> Option<crate::outcome::FailureClass> {
        if self.comparison.passed() { None } else { Some(crate::outcome::FailureClass::PixelMismatch) }
    }
}

pub fn compare(expected: Frame<'_>, actual: Frame<'_>, options: Options<'_>) -> Result<Verdict, Error> {
    if (expected.width, expected.height) != (actual.width, actual.height) {
        return Err(Error::SizeMismatch {
            expected: (expected.width, expected.height),
            actual: (actual.width, actual.height),
        });
    }
    let comparison = libpixcheck::compare(expected, actual, options).map_err(Error::Invalid)?;
    let closest_region = comparison.first.and_then(|pixel| {
        options.regions.iter().enumerate().min_by_key(|(_, region)| {
            let dx = if pixel.x < region.x { region.x - pixel.x } else { pixel.x.saturating_sub(region.x + region.w - 1) };
            let dy = if pixel.y < region.y { region.y - pixel.y } else { pixel.y.saturating_sub(region.y + region.h - 1) };
            dx + dy
        }).map(|(index, _)| index)
    });
    Ok(Verdict { comparison, closest_region })
}
