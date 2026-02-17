use libgfx::framebuf::FrameBuf;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;
use crate::widget;

/// A clickable button with a text label.
pub struct Button {
    pub rect: Rect,
    pub label: &'static [u8],
    hovered: bool,
    pressed: bool,
}

impl Button {
    pub fn new(rect: Rect, label: &'static [u8]) -> Self {
        Self {
            rect,
            label,
            hovered: false,
            pressed: false,
        }
    }

    /// Process input and return `WidgetEvent::Clicked` on click.
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.hovered = self.rect.contains(input.mouse_x, input.mouse_y);

        if self.hovered && input.mouse_pressed {
            self.pressed = true;
        }

        if input.mouse_released {
            if self.pressed && self.hovered {
                self.pressed = false;
                return WidgetEvent::Clicked;
            }
            self.pressed = false;
        }

        WidgetEvent::None
    }

    /// Draw the button with theme-appropriate colors.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        widget::draw_widget_bg(fb, &self.rect, theme, self.hovered, self.pressed);
        widget::draw_widget_border(fb, &self.rect, theme.border);
        text::draw_text_centered(fb, self.label, &self.rect, theme.text_primary, theme);
    }

    /// Convenience: update and return true if clicked.
    pub fn clicked(&mut self, input: &InputState) -> bool {
        self.update(input) == WidgetEvent::Clicked
    }
}
