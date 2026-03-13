//! no_std-compatible float utilities.

/// Floor: largest integer <= x, returned as f32.
#[inline]
pub fn floor(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) > x { (i - 1) as f32 } else { i as f32 }
}

/// Ceiling: smallest integer >= x, returned as f32.
#[inline]
pub fn ceil(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) < x { (i + 1) as f32 } else { i as f32 }
}

