use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;

const TITLE_BAR_HEIGHT: i32 = 20;

/// A visual container with border and optional title.
pub struct Panel {
    pub rect: Rect,
    pub title: Option<&'static [u8]>,
}

impl Panel {
    pub fn new(rect: Rect) -> Self {
        Self { rect, title: None }
    }

    pub fn with_title(rect: Rect, title: &'static [u8]) -> Self {
        Self {
            rect,
            title: Some(title),
        }
    }

    /// Draw the panel background, border, and optional title bar.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        // Background
        shapes::fill_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.panel_bg);

        // Title bar
        if let Some(title) = self.title {
            let title_rect = Rect::new(self.rect.x, self.rect.y, self.rect.w, TITLE_BAR_HEIGHT);
            shapes::fill_rect(
                fb,
                title_rect.x,
                title_rect.y,
                title_rect.w,
                title_rect.h,
                theme.widget_bg,
            );
            text::draw_text_centered(fb, title, &title_rect, theme.text_primary, theme);
            // Separator line below title
            shapes::draw_line(
                fb,
                self.rect.x,
                self.rect.y + TITLE_BAR_HEIGHT,
                self.rect.x + self.rect.w - 1,
                self.rect.y + TITLE_BAR_HEIGHT,
                theme.border,
            );
        }

        // Border
        shapes::draw_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.border);
    }

    /// Returns the inner content area, accounting for border and title bar.
    pub fn content_rect(&self, theme: &Theme) -> Rect {
        let inner = self.rect.inset(theme.border_width);
        if self.title.is_some() {
            let (_, content) = inner.split_top(TITLE_BAR_HEIGHT);
            content
        } else {
            inner
        }
    }
}
