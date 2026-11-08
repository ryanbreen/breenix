//! Build an expected canonical BGRX frame from a suite-authored pixel formula.
extern crate alloc;
use alloc::vec::Vec;

pub fn reference(width: usize, height: usize, pixel: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    bytes
}
