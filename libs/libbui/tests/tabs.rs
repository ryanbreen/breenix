use libbui::{widget::tab_bar::TabBar, InputState, Rect, Theme, WidgetEvent};
use libgfx_ref::{color::Color, framebuf::{FrameBuf, Format}};
#[test]
fn partially_visible_tabs_are_clipped_and_use_the_drawn_font_width() {
    for bitmap in [false, true] {
        let mut theme = Theme::dark();
        theme.use_bitmap_font = bitmap;
        let paper = Color::rgb(13, 27, 41);
        let mut fb = FrameBuf::new_filled(160, 60, Format::BGRX, paper).unwrap();
        let mut bar = TabBar::new(Rect::new(10, 10, 70, 24), vec![b"WWWW", b"Tab2"], &theme);
        bar.draw(&mut fb, &theme);
        for y in 0..60 { for x in 0..160 {
            if !(10..80).contains(&x) || !(10..34).contains(&y) {
                assert_eq!(fb.get_pixel(x,y), paper, "bitmap={bitmap} at {x},{y}");
            }
        }}
        let width = libbui::text::text_width(b"WWWW", &theme) + 16;
        let input = InputState { mouse_x: 10+width-1, mouse_y: 20, time_ms: 100,
            mouse_down: false, mouse_pressed: false, mouse_released: true };
        assert_eq!(bar.update(&input), WidgetEvent::None, "tab one ends too soon");
        assert_eq!(bar.update(&InputState { mouse_x: 10+width, ..input }), WidgetEvent::ValueChanged(1.0));
    }
}

#[test]
fn bitmap_tab_hit_testing_precedes_the_first_draw() {
    let mut theme = Theme::dark();
    theme.use_bitmap_font = true;
    let mut bar = TabBar::new(Rect::new(0, 0, 100, 24), vec![b"WWWWWW", b"Second"], &theme);
    let width = libbui::text::text_width(b"WWWWWW", &theme) + 16;
    let input = InputState { mouse_x: width - 1, mouse_y: 12, mouse_down: false, mouse_pressed: false, mouse_released: true, time_ms: 1 };
    assert_eq!(bar.update(&input), WidgetEvent::None);
    assert_eq!(bar.update(&InputState { mouse_x: width, ..input }), WidgetEvent::ValueChanged(1.0));
}
