//! Failure details shared by UI outcomes, checks and input delivery.
use std::fmt::Debug;

/// Canonical BGRX input is shown as RGB; the unused alpha byte is excluded.
pub fn pixel_details(x: usize, y: usize, got: [u8; 4], want: [u8; 4]) -> String {
    format!(
        "first={x},{y} got=#{:02x}{:02x}{:02x} want=#{:02x}{:02x}{:02x}",
        got[2], got[1], got[0], want[2], want[1], want[0]
    )
}
pub fn pixel(check: &str, x: usize, y: usize, got: [u8; 4], want: [u8; 4]) -> String {
    format!(
        "pixel-mismatch check={check:?} {}",
        pixel_details(x, y, got, want)
    )
}
pub fn tree(
    check: &str,
    node: &str,
    field: &str,
    expected: &impl Debug,
    reported: &impl Debug,
) -> String {
    format!("tree-mismatch check={check:?} node={node:?} field={field} expected={expected:?} reported={reported:?}")
}
pub fn input(check: &str, serial: u64, disposition: &str, detail: &str) -> String {
    format!("input check={check:?} serial={serial} disposition={disposition} {detail}")
}
