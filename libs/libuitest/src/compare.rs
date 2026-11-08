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

/// Evaluate the validated structural definition over its post-mask domain.
pub fn structural(frame: Frame<'_>, check: &crate::schema::Check) -> Result<bool, Error> {
    let invalid = || Error::Invalid(libpixcheck::Error::Predicate);
    let definition = &check.definition;
    let kind = definition["predicate"].as_str().ok_or_else(invalid)?;
    let params = &definition["params"];
    let colour = if kind == "uniform" || kind == "outside" {
        let text = params["colour"].as_str().ok_or_else(invalid)?;
        let value = u32::from_str_radix(text.strip_prefix('#').ok_or_else(invalid)?, 16).map_err(|_| invalid())?;
        Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
    } else { None };
    if !matches!(kind, "uniform" | "outside" | "count") { return Err(invalid()); }
    let mut colours = Vec::new();
    let mut visited = 0u64;
    let mut matches = true;
    for y in 0..frame.height {
        for x in 0..frame.width {
            if !check.in_effective_domain(x as u32, y as u32) { continue; }
            visited += 1;
            let p = frame.pixel(x, y).expect("coordinates lie inside frame");
            let rgb = [p[2], p[1], p[0]];
            match kind {
                "uniform" => matches &= Some(rgb) == colour,
                "outside" => matches &= Some(rgb) != colour,
                "count" => colours.push(rgb),
                _ => unreachable!(),
            }
        }
    }
    if visited == 0 || visited != check.effective_area { return Err(invalid()); }
    if kind == "count" {
        let min = params["min"].as_u64().ok_or_else(invalid)?;
        let max = params.get("max").and_then(|v| v.as_u64());
        colours.sort_unstable();
        colours.dedup();
        matches = colours.len() as u64 >= min && max.is_none_or(|m| colours.len() as u64 <= m);
    }
    Ok(matches)
}
