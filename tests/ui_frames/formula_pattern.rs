//! Formula authored in the suite: a small coordinate-derived RGB pattern.
pub fn pixel(x: usize, y: usize) -> [u8; 4] {
    [x as u8 * 10, y as u8 * 20, 30, 255]
}
