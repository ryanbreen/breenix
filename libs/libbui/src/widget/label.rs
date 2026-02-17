use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;

use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;

/// A non-interactive text display widget.
pub struct Label {
    pub rect: Rect,
    pub text: &'static [u8],
    pub color: Option<Color>,
}

impl Label {
    pub fn new(rect: Rect, text: &'static [u8]) -> Self {
        Self {
            rect,
            text,
            color: None,
        }
    }

    /// Create a label with a custom text color.
    pub fn with_color(rect: Rect, text: &'static [u8], color: Color) -> Self {
        Self {
            rect,
            text,
            color: Some(color),
        }
    }

    /// Draw the label text at the widget's position.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        let color = self.color.unwrap_or(theme.text_primary);
        text::draw_text(fb, self.text, self.rect.x, self.rect.y, color, theme);
    }
}
