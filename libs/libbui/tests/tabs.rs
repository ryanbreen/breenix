use libbui::{widget::tab_bar::TabBar, InputState, Rect, Theme, WidgetEvent};

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
