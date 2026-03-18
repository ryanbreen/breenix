//! Vertical scroll bar widget.
//!
//! Tracks a scroll offset for content that exceeds the visible area.
//! Renders a track with a draggable thumb.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::InputState;
use crate::rect::Rect;
use crate::theme::Theme;

/// Vertical scroll bar widget.
///
/// Tracks a scroll offset for content taller than the visible area.
/// Renders a track with a proportional draggable thumb.
///
/// # Usage
///
/// ```no_run
/// let mut bar = ScrollBar::new(
///     Rect::new(win_w - ScrollBar::DEFAULT_WIDTH, 0, ScrollBar::DEFAULT_WIDTH, win_h),
///     content_height,
///     visible_height,
/// );
///
/// // In the event loop:
/// if let Event::Scroll { delta_y } = event {
///     bar.scroll(delta_y);
/// }
/// bar.update(&input);
///
/// // When rendering content, subtract bar.offset() from all y positions.
/// // Draw the bar last so it renders on top of content.
/// bar.draw(fb, &theme);
/// ```
pub struct ScrollBar {
    /// Position and size of the scroll bar track.
    rect: Rect,
    /// Total content height in pixels.
    content_height: i32,
    /// Visible area height in pixels.
    view_height: i32,
    /// Current scroll offset (0 = top, max = content_height - view_height).
    offset: i32,
    /// Whether the thumb is currently being dragged.
    dragging: bool,
    /// Y offset within the thumb where the drag began.
    drag_anchor: i32,
}

impl ScrollBar {
    /// Default scrollbar track width in pixels.
    pub const DEFAULT_WIDTH: i32 = 12;

    /// Create a new vertical scroll bar.
    ///
    /// `rect` is the bounding box of the track (typically a narrow vertical strip
    /// along the right edge of the content area).
    ///
    /// `content_height` is the total rendered content height in pixels.
    /// `view_height` is how many pixels are actually visible.
    pub fn new(rect: Rect, content_height: i32, view_height: i32) -> Self {
        Self {
            rect,
            content_height,
            view_height,
            offset: 0,
            dragging: false,
            drag_anchor: 0,
        }
    }

    /// The current scroll offset in pixels (0 = top).
    pub fn offset(&self) -> i32 {
        self.offset
    }

    /// Update content and view heights after content changes or window resizes.
    ///
    /// The offset is clamped to the new valid range automatically.
    pub fn set_dimensions(&mut self, content_height: i32, view_height: i32) {
        self.content_height = content_height;
        self.view_height = view_height;
        self.clamp_offset();
    }

    /// Update the scrollbar's bounding rect (e.g., after a window resize).
    pub fn set_rect(&mut self, rect: Rect) {
        self.rect = rect;
    }

    /// Scroll by `delta_y` units.
    ///
    /// Positive `delta_y` scrolls up (offset decreases toward 0).
    /// Negative `delta_y` scrolls down (offset increases toward max).
    ///
    /// Each unit corresponds to approximately 3 rows of 13px text (about 40px).
    pub fn scroll(&mut self, delta_y: i32) {
        self.offset -= delta_y * 40;
        self.clamp_offset();
    }

    /// Process mouse input for drag interaction.
    ///
    /// Returns `true` if the scrollbar consumed the event (the caller should
    /// not pass this input to widgets behind the scrollbar).
    pub fn update(&mut self, input: &InputState) -> bool {
        let max_scroll = self.max_scroll();
        if max_scroll <= 0 {
            self.dragging = false;
            return false;
        }

        let thumb = self.thumb_rect();

        if input.mouse_pressed && self.rect.contains(input.mouse_x, input.mouse_y) {
            if thumb.contains(input.mouse_x, input.mouse_y) {
                // Start drag from within the thumb
                self.dragging = true;
                self.drag_anchor = input.mouse_y - thumb.y;
            } else {
                // Click on track: jump to the clicked position
                let track_y = input.mouse_y - self.rect.y;
                let ratio = track_y as f32 / self.rect.h as f32;
                self.offset = (ratio * max_scroll as f32) as i32;
                self.clamp_offset();
            }
            return true;
        }

        if self.dragging {
            if input.mouse_down {
                // Continue dragging — map mouse Y to scroll offset
                let thumb_top = input.mouse_y - self.drag_anchor - self.rect.y;
                let track_range = self.rect.h - thumb.h;
                if track_range > 0 {
                    let ratio = thumb_top as f32 / track_range as f32;
                    self.offset = (ratio * max_scroll as f32) as i32;
                    self.clamp_offset();
                }
                return true;
            } else {
                self.dragging = false;
            }
        }

        false
    }

    /// Draw the scrollbar track and thumb.
    ///
    /// When `can_scroll()` is false (content fits in the view), nothing is drawn.
    pub fn draw(&self, fb: &mut FrameBuf, _theme: &Theme) {
        if !self.can_scroll() {
            return;
        }

        // Track background
        let track_color = Color::rgb(40, 42, 48);
        shapes::fill_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, track_color);

        // Thumb — inset by 2px on sides, 1px top/bottom for a rounded appearance
        let thumb = self.thumb_rect();
        let thumb_color = if self.dragging {
            Color::rgb(140, 150, 170)
        } else {
            Color::rgb(80, 85, 100)
        };
        shapes::fill_rect(
            fb,
            thumb.x + 2,
            thumb.y + 1,
            (thumb.w - 4).max(2),
            (thumb.h - 2).max(2),
            thumb_color,
        );
    }

    /// Whether the content exceeds the view height (scrollbar is needed).
    pub fn can_scroll(&self) -> bool {
        self.content_height > self.view_height
    }

    /// Maximum scroll offset = content_height - view_height.
    pub fn max_scroll(&self) -> i32 {
        (self.content_height - self.view_height).max(0)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn clamp_offset(&mut self) {
        let max = self.max_scroll();
        if self.offset < 0 {
            self.offset = 0;
        }
        if self.offset > max {
            self.offset = max;
        }
    }

    fn thumb_rect(&self) -> Rect {
        let max_scroll = self.max_scroll();
        if max_scroll <= 0 || self.content_height <= 0 {
            return Rect::new(self.rect.x, self.rect.y, self.rect.w, self.rect.h);
        }

        // Thumb height proportional to visible fraction of total content
        let ratio = self.view_height as f32 / self.content_height as f32;
        let thumb_h = ((ratio * self.rect.h as f32) as i32)
            .max(20)      // minimum 20px so it's always clickable
            .min(self.rect.h);

        // Thumb Y position proportional to scroll offset
        let track_range = self.rect.h - thumb_h;
        let thumb_y = if max_scroll > 0 {
            self.rect.y
                + (self.offset as f32 / max_scroll as f32 * track_range as f32) as i32
        } else {
            self.rect.y
        };

        Rect::new(self.rect.x, thumb_y, self.rect.w, thumb_h)
    }
}
