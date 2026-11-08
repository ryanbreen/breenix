//! Generate the suite's coordinate-pattern PNG with the guest's formula code.
#[path = "../../../../libs/libuitest/src/formula.rs"]
mod formula;
#[path = "../../../../userspace/programs/src/uitest_suite/formula_pattern.rs"]
mod pattern;
#[path = "../png.rs"]
mod png;
use libgfx_ref::{color::Color, framebuf::{Format, FrameBuf}};

fn main() {
    let path = std::env::args().nth(1).expect("output PNG path");
    let pixels = formula::reference(4, 4, pattern::pixel);
    let mut frame = FrameBuf::new_filled(4, 4, Format::BGRX, Color::rgb(0, 0, 0)).unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let p = &pixels[(y * 4 + x) * 4..][..4];
            frame.put_pixel(x, y, Color::rgb(p[2], p[1], p[0]));
        }
    }
    std::fs::write(path, png::encode_rgb(&frame)).expect("write PNG");
}
