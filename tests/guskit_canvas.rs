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

#[test]
fn history_undo_redo_and_branch() {
    let mut h = canvas::History::new(Canvas::new(2, 1));
    let mut a = Canvas::new(2, 1);
    a.pencil(&[(0, 0)], K);
    h.push(a);
    let mut b = Canvas::new(2, 1);
    b.fill((0, 0), K);
    h.push(b);
    assert!(h.undo());
    assert_eq!(h.canvas().pixels(), &[1, 2, 3, 255, 255, 255]);
    assert!(h.redo());
    assert_eq!(h.canvas().pixels(), &[1, 2, 3, 1, 2, 3]);
    assert!(h.undo());
    h.push(Canvas::new(1, 1));
    assert_eq!(h.redo_depth(), 0);
    assert!(!h.redo());
}

#[test]
fn history_folds_oldest_into_base() {
    let mut h = canvas::History::new(Canvas::new(1, 1));
    for n in 0..33 {
        let mut c = Canvas::new(1, 1);
        c.fill((0, 0), [n, 0, 0]);
        h.push(c);
    }
    assert_eq!(h.undo_depth(), 32);
    for _ in 0..32 { assert!(h.undo()); }
    assert_eq!(h.canvas().pixels(), &[0, 0, 0]);
    assert!(!h.undo());
    for _ in 0..32 { assert!(h.redo()); }
    assert_eq!(h.canvas().pixels(), &[32, 0, 0]);
}

#[test]
fn preview_does_not_change_the_committed_canvas() {
    let base = Canvas::new(3, 2);
    let preview = base.preview(&[(0, 0), (2, 0)], |image, points| image.pencil(points, [0, 0, 0]));
    assert_eq!(base.pixels(), &[255; 18]);
    assert_eq!(preview.pixels(), &[0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255]);
}

#[test]
fn canvas_render_runs_preserve_pixels_and_coalesce_adjacent_colors() {
    let mut image = Canvas::new(4, 2);
    image.line((1, 0), (2, 0), [0, 0, 0]);
    let mut runs = Vec::new();
    image.render_runs(|x, y, width, color| runs.push((x, y, width, color)));
    assert_eq!(runs, vec![(0,0,1,[255;3]), (1,0,2,[0;3]), (3,0,1,[255;3]), (0,1,4,[255;3])]);
}

#[test]
fn grow_preserves_pixels_and_pads_white() {
    let mut h = canvas::History::new(Canvas::new(2, 2));
    let mut image = h.canvas().clone();
    image.pencil(&[(0, 0), (1, 1)], K);
    h.push(image);
    h.grow(4, 3);
    assert_eq!(h.canvas().pixels(), &[1,2,3,255,255,255,255,255,255,255,255,255, 255,255,255,1,2,3,255,255,255,255,255,255, 255,255,255,255,255,255,255,255,255,255,255,255]);
}

#[test]
fn shrink_then_grow_never_discards_pixels() {
    let mut image = Canvas::new(3, 2);
    image.pencil(&[(2, 1)], K);
    let mut h = canvas::History::new(image);
    h.grow(1, 1);
    h.grow(4, 3);
    assert_eq!(&h.canvas().pixels()[(4 + 2)*3..(4 + 3)*3], &K);
    assert_eq!(h.canvas().pixels().len(), 4 * 3 * 3);
}

#[test]
fn undo_and_redo_after_grow_preserve_all_states() {
    let mut h = canvas::History::new(Canvas::new(2, 2));
    let mut first = h.canvas().clone();
    first.pencil(&[(1, 1)], K);
    h.push(first.clone());
    let mut second = first;
    second.pencil(&[(0, 0)], K);
    h.push(second);
    assert!(h.undo());
    h.grow(3, 3);
    assert_eq!((h.undo_depth(), h.redo_depth()), (1, 1));
    assert!(h.undo());
    assert_eq!(h.canvas().pixels(), &[255; 27]);
    assert!(h.redo());
    assert_eq!(&h.canvas().pixels()[12..15], &K);
    assert!(h.redo());
    assert_eq!(&h.canvas().pixels()[..3], &K);
    assert_eq!(&h.canvas().pixels()[12..15], &K);
}
