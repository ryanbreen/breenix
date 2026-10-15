use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::theme::Theme;

const THUMB_RADIUS: i32 = 6;
const TRACK_HEIGHT: i32 = 4;

/// A horizontal slider with a normalized 0.0–1.0 internal value.
pub struct Slider {
    pub rect: Rect,
    /// Normalized value in 0.0–1.0.
    pub value: f32,
    pub min_val: f32,
    pub max_val: f32,
    dragging: bool,
}

impl Slider {
    pub fn new(rect: Rect, min_val: f32, max_val: f32) -> Self {
        Self {
            rect,
            value: 0.0,
            min_val,
            max_val,
            dragging: false,
        }
    }

    /// Process input and return `WidgetEvent::ValueChanged(mapped)` when the value changes.
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        if input.mouse_pressed && self.rect.contains(input.mouse_x, input.mouse_y) {
            self.dragging = true;
        }

        if input.mouse_released {
            self.dragging = false;
        }

        if self.dragging && input.mouse_down {
            let track_x = self.rect.x + THUMB_RADIUS;
            let track_w = self.rect.w - THUMB_RADIUS * 2;
            if track_w > 0 {
                let rel = (input.mouse_x - track_x) as f32 / track_w as f32;
                let new_value = if rel < 0.0 {
                    0.0
                } else if rel > 1.0 {
                    1.0
                } else {
                    rel
                };
                if new_value != self.value {
                    self.value = new_value;
                    return WidgetEvent::ValueChanged(self.mapped_value());
                }
            }
        }

        WidgetEvent::None
    }

    /// Draw the slider track, fill, and thumb.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        let track_y = self.rect.y + (self.rect.h - TRACK_HEIGHT) / 2;
        let track_x = self.rect.x + THUMB_RADIUS;
        let track_w = self.rect.w - THUMB_RADIUS * 2;

        // Track background
        shapes::fill_rect(fb, track_x, track_y, track_w, TRACK_HEIGHT, theme.widget_bg);
        shapes::draw_rect(fb, track_x, track_y, track_w, TRACK_HEIGHT, theme.border);

        // Filled portion
        let fill_w = (self.value * track_w as f32) as i32;
        if fill_w > 0 {
            shapes::fill_rect(fb, track_x, track_y, fill_w, TRACK_HEIGHT, theme.accent);
        }

        // Thumb
        let thumb_x = track_x + fill_w;
        let thumb_y = self.rect.y + self.rect.h / 2;
        shapes::fill_circle(fb, thumb_x, thumb_y, THUMB_RADIUS, theme.widget_bg_hover);
        shapes::draw_circle(fb, thumb_x, thumb_y, THUMB_RADIUS, theme.border);
    }

    /// Get the value mapped to the `min_val..max_val` range.
    pub fn mapped_value(&self) -> f32 {
        self.min_val + self.value * (self.max_val - self.min_val)
    }

    /// Get the mapped value as an integer.
    pub fn int_value(&self) -> i32 {
        self.mapped_value() as i32
    }
}
