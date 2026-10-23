pub mod button;
pub mod checkbox;
pub mod file_picker;
pub mod label;
pub mod menu_bar;
pub mod panel;
pub mod scroll_bar;
pub mod slider;
pub mod tab_bar;
pub mod text_field;

use libgfx::color::Color;
use libgfx::surface::Surface as FrameBuf;
use crate::drawing as shapes;

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
pub fn draw_widget_bg(fb: &mut FrameBuf, rect: &Rect, theme: &Theme, hovered: bool, pressed: bool) {
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

/// Draw the contract focus perimeter, two pixels outside the widget bounds.
pub fn draw_focus_ring(fb: &mut FrameBuf, rect: &Rect, theme: &Theme) {
    shapes::draw_rect(
        fb,
        rect.x - 2,
        rect.y - 2,
        rect.w + 4,
        rect.h + 4,
        theme.focus_ring,
    );
}
