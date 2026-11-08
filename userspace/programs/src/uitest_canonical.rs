//! Canonical UI scene v1: integer, opaque, half-open rectangles only. The app
//! and host reference generator call this same scene without clocks, fonts,
//! process identity, random state or target-specific arithmetic.
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;

/// Emit RGB fills in painter order. The guest supplies GPU fill_rect; refgen
/// supplies the CPU oracle's fill_rect. Coordinates and colors are literal.
pub fn draw(mut fill: impl FnMut(i32,i32,i32,i32,[u8;3])) {
    fill(0,0,WIDTH as i32,HEIGHT as i32,[24,32,48]);
    fill(8,8,304,24,[40,100,220]);
    fill(12,40,296,120,[45,50,65]);
    fill(20,52,72,48,[224,96,64]);
    fill(104,52,72,48,[64,176,112]);
    fill(188,52,72,48,[64,128,224]);
    fill(20,116,240,12,[80,85,100]);
    fill(20,116,156,12,[220,180,64]);
    for row in 0..2 { for col in 0..18 {
        let color = if (row+col)%2 == 0 { [64,80,112] } else { [128,144,176] };
        fill(12+col*16,168+row*12,16,12,color);
    }}
}
