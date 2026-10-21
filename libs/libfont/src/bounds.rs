//! Admission bound shared by geometry preparation and the GPU text path.
/// Section 7's step bound for rasterizing `e` segments into `h` rows of `c` sampled
/// columns: `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64`.
pub fn raster_bound(e: usize, h: usize, c: usize) -> u128 {
    let (e, h, c) = (e as u128, h as u128, c as u128);
    let log = (128 - (e + 1).leading_zeros() as u128) - if (e + 1).is_power_of_two() { 1 } else { 0 };
    e + h * (10 * c + 8 * e * (3 + log) + 17) + 64
}

/// Glyphs whose bound exceeds this are refused before any raster loop.
pub const MAX_RASTER_STEPS: u128 = 1 << 28;

