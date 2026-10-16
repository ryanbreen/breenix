//! Alpha arithmetic (docs/gui/breengel.md sections 1 and 2).

use crate::color::Color;

/// The exact round-to-nearest of `x / 255` for `0 ≤ x ≤ 65025`.
#[inline]
pub fn div255(x: u32) -> u8 {
    ((x + 128 + ((x + 128) >> 8)) >> 8) as u8
}

/// A straight colour channel `c` with alpha `a`, premultiplied: `div255(c × a)`.
#[inline]
pub fn premultiply(c: u8, a: u8) -> u8 {
    div255(c as u32 * a as u32)
}

/// Source-over on premultiplied `[b, g, r, a]` pixels:
/// `out = s + div255(d × (255 − s_a))` for every channel, alpha included.
#[inline]
pub fn over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let inv = 255 - src[3] as u32;
    let ch = |s: u8, d: u8| (s as u32 + div255(d as u32 * inv) as u32) as u8;
    [ch(src[0], dst[0]), ch(src[1], dst[1]), ch(src[2], dst[2]), ch(src[3], dst[3])]
}

/// The coverage blend of an opaque foreground `f` over a background `b` with coverage `a`:
/// `div255(f × a + b × (255 − a))` per channel.
#[inline]
pub fn blend(f: Color, b: Color, a: u8) -> Color {
    let (a, inv) = (a as u32, 255 - a as u32);
    let ch = |f: u8, b: u8| div255(f as u32 * a + b as u32 * inv);
    Color::rgb(ch(f.r, b.r), ch(f.g, b.g), ch(f.b, b.b))
}
