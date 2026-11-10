#[path = "../userspace/programs/src/guskit/canvas.rs"]
mod canvas;

use canvas::{Canvas, Radius};
const K: [u8; 3] = [1, 2, 3];
const W: [u8; 3] = [255; 3];
fn check(c: &Canvas, rows: [&str; 8]) {
    let expected: Vec<u8> = rows.iter().flat_map(|r| r.bytes()).flat_map(|p| if p == b'#' { K } else { W }).collect();
    assert_eq!(c.pixels(), expected);
}
#[test]
fn pencil_polyline_and_clipping() {
    let mut c = Canvas::new(8, 8);
    c.pencil(&[(-1, 0), (2, 0), (2, 2)], K);
    check(&c, ["###.....", "..#.....", "..#.....", "........", "........", "........", "........", "........"]);
}
#[test]
fn brush_discs_follow_the_line() {
    let mut c = Canvas::new(8, 8);
    c.brush(&[(1, 1), (2, 1)], Radius::Small, K);
    check(&c, ["####....", "#####...", "####....", ".##.....", "........", "........", "........", "........"]);
}
#[test]
fn brush_sizes_and_eraser_background() {
    let mut c = Canvas::new(8, 8);
    c.brush(&[(0, 0)], Radius::Large, K);
    c.erase(&[(0, 0)], Radius::Medium);
    check(&c, ["......##", ".....###", ".....###", ".....###", "....####", ".#######", "########", "########"]);
}
#[test]
fn line_bresenham() {
    let mut c = Canvas::new(8, 8);
    c.line((0, 0), (5, 2), K);
    check(&c, ["##......", "..##....", "....##..", "........", "........", "........", "........", "........"]);
}
#[test]
fn rectangle_is_an_outline() {
    let mut c = Canvas::new(8, 8);
    c.rectangle((5, 4), (2, 1), K);
    check(&c, ["........", "..####..", "..#..#..", "..#..#..", "..####..", "........", "........", "........"]);
}
#[test]
fn circle_is_an_outline_with_integer_radius() {
    let mut c = Canvas::new(8, 8);
    c.circle((3, 3), (5, 4), K);
    check(&c, ["........", "..###...", ".#...#..", ".#...#..", ".#...#..", "..###...", "........", "........"]);
}
#[test]
fn fill_is_four_connected() {
    let mut c = Canvas::new(8, 8);
    c.rectangle((1, 1), (6, 6), K);
    c.fill((0, 0), K);
    check(&c, ["########", "########", "##....##", "##....##", "##....##", "##....##", "########", "########"]);
}
#[test]
fn digest_covers_row_major_rgb_bytes() {
    let mut c = Canvas::new(8, 8);
    c.pencil(&[(0, 0)], K);
    let mut bytes = [255u8; 192];
    bytes[..3].copy_from_slice(&K);
    assert_eq!(c.pixels(), bytes);
    assert_eq!(c.digest(), libpixcheck::sha256(&bytes));
}
