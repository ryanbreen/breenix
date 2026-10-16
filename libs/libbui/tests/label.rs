//! §14.3 and the 2026-10-15 fixture ruling: test clipping both through and above dot rows.
use libbui::widget::label::Label;
use libbui::{Rect, Theme};
use libgfx::{color::Color, framebuf::FrameBuf};
#[test]
fn narrow_ellipsis_keeps_ink_only_when_dot_rows_are_inside() {
    let paper = Color::rgb(13, 27, 41);
    for height in [7, 3] {
        let mut pixels = vec![0u8; 80 * 40 * 4];
        let mut fb = unsafe { FrameBuf::from_raw(pixels.as_mut_ptr(), 80, 40, 320, 4, true) };
        fb.clear(paper);
        Label::new(Rect::new(10, 8, 12, height), b"ABCDEFGHIJKLM").draw(&mut fb, &Theme::dark());
        let mut ink = 0;
        for y in 0..40 {
            for x in 0..80 {
                let dot = height == 7 && [13, 14].contains(&y) && [11, 12, 17, 18].contains(&x);
                let want = if dot { Color::WHITE } else { paper };
                assert_eq!(fb.get_pixel(x, y), want, "12x{height}: pixel ({x},{y})");
                if fb.get_pixel(x, y) != paper {
                    ink += 1;
                }
            }
        }
        assert_eq!(ink, if height == 7 { 8 } else { 0 });
    }
}
