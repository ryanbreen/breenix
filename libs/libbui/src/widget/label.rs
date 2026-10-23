use libgfx::color::Color;
use libgfx::surface::Surface as FrameBuf;

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
        crate::drawing::push_clip(fb, libgfx::surface::Rect::new(self.rect.x, self.rect.y, self.rect.w, self.rect.h));
        if text::text_width(self.text, theme) <= self.rect.w {
            text::draw_text(fb, self.text, self.rect.x, self.rect.y, color, theme);
        } else {
            let mut end = 0;
            while end < self.text.len() {
                let mut candidate = alloc::vec::Vec::from(&self.text[..end + 1]);
                candidate.extend_from_slice(b"...");
                if text::text_width(&candidate, theme) > self.rect.w { break; }
                end += 1;
            }
            let mut clipped = alloc::vec::Vec::from(&self.text[..end]);
            clipped.extend_from_slice(b"...");
            text::draw_text(fb, &clipped, self.rect.x, self.rect.y, color, theme);
        }
        fb.pop_clip();
    }
}
