pub mod button;
pub mod checkbox;
pub mod file_picker;
pub mod label;
pub mod panel;
pub mod slider;

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::theme::Theme;

/// Optional trait for uniform widget handling.
pub trait Widget {
    fn rect(&self) -> Rect;
    fn draw(&self, fb: &mut FrameBuf, theme: &Theme);
    fn update(&mut self, input: &InputState) -> WidgetEvent;
}

/// Draw a filled background for a widget, selecting color based on hover/pressed state.
pub fn draw_widget_bg(
    fb: &mut FrameBuf,
    rect: &Rect,
    theme: &Theme,
    hovered: bool,
    pressed: bool,
) {
    let bg = if pressed {
        theme.widget_bg_active
    } else if hovered {
        theme.widget_bg_hover
    } else {
        theme.widget_bg
    };
    shapes::fill_rect(fb, rect.x, rect.y, rect.w, rect.h, bg);
}

/// Draw a border around a widget rectangle.
pub fn draw_widget_border(fb: &mut FrameBuf, rect: &Rect, color: Color) {
    shapes::draw_rect(fb, rect.x, rect.y, rect.w, rect.h, color);
}
