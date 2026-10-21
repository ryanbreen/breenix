use libbui::{widget::tab_bar::TabBar, InputState, Rect, Theme, WidgetEvent};
use libgfx::color::Color;
pub fn run() {
    let scale=super::ui_scale();
    for bitmap in [false, true] {
        let mut theme = Theme::dark();
        theme.use_bitmap_font = bitmap;
        let paper = Color::rgb(13, 27, 41);
        let mut fb=super::ui_frame(160,60);fb.reset(paper);
        let width = libbui::text::text_width(b"WWWW", &theme) + 16;
        let mut bar = TabBar::new(Rect::new(10, 10, width + 1, 24), vec![b"WWWW", b"Tab2"], &theme);
        bar.draw(&mut fb.fb(), &theme);
        for y in 0..60*scale { for x in 0..160*scale {
            if !(10*scale..(11+width as usize)*scale).contains(&x) || !(10*scale..34*scale).contains(&y) {
                assert_eq!(fb.color(x,y), paper, "bitmap={bitmap} at {x},{y}");
            }
        }}
        let input = InputState { mouse_x: 10+width-1, mouse_y: 20, time_ms: 100,
            mouse_down: false, mouse_pressed: false, mouse_released: true };
        assert_eq!(bar.update(&input), WidgetEvent::None, "tab one ends too soon");
        assert_eq!(bar.update(&InputState { mouse_x: 10+width, ..input }), WidgetEvent::ValueChanged(1.0));
    }
}
