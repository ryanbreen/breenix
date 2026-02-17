use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;

const CHECK_SIZE: i32 = 14;

/// A toggleable checkbox with a label.
pub struct Checkbox {
    pub rect: Rect,
    pub label: &'static [u8],
    pub checked: bool,
    hovered: bool,
}

impl Checkbox {
    pub fn new(rect: Rect, label: &'static [u8]) -> Self {
        Self {
            rect,
            label,
            checked: false,
            hovered: false,
        }
    }

    /// Process input and return `WidgetEvent::Toggled(checked)` on click.
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.hovered = self.rect.contains(input.mouse_x, input.mouse_y);

        if self.hovered && input.mouse_pressed {
            self.checked = !self.checked;
            return WidgetEvent::Toggled(self.checked);
        }

        WidgetEvent::None
    }

    /// Draw the checkbox box and label.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        let box_y = self.rect.y + (self.rect.h - CHECK_SIZE) / 2;
        let box_x = self.rect.x;

        // Box background
        let bg = if self.hovered { theme.widget_bg_hover } else { theme.widget_bg };
        shapes::fill_rect(fb, box_x, box_y, CHECK_SIZE, CHECK_SIZE, bg);

        if self.checked {
            shapes::fill_rect(fb, box_x, box_y, CHECK_SIZE, CHECK_SIZE, theme.accent);
            // Checkmark: two lines forming a check shape
            shapes::draw_line(fb, box_x + 3, box_y + 7, box_x + 5, box_y + 10, theme.text_primary);
            shapes::draw_line(fb, box_x + 5, box_y + 10, box_x + 11, box_y + 3, theme.text_primary);
        }

        // Border
        shapes::draw_rect(fb, box_x, box_y, CHECK_SIZE, CHECK_SIZE, theme.border);

        // Label text to the right
        let text_x = box_x + CHECK_SIZE + theme.spacing;
        let th = text::text_height(theme);
        let text_y = self.rect.y + (self.rect.h - th) / 2;
        text::draw_text(fb, self.label, text_x, text_y, theme.text_primary, theme);
    }
}
