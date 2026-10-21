use libgfx::surface::Surface as FrameBuf;
use crate::drawing as shapes;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::theme::Theme;

const THUMB_RADIUS: i32 = 6;
const TRACK_HEIGHT: i32 = 4;

/// A horizontal slider with a normalized 0.0–1.0 internal value.
pub struct Slider {
    enabled: bool,
    focused: bool,
    redraw: bool,
    pub rect: Rect,
    /// Normalized value in 0.0–1.0.
    pub value: f32,
    pub min_val: f32,
    pub max_val: f32,
    dragging: bool,
    step: f32,
}

impl Slider {
    pub fn new(rect: Rect, min_val: f32, max_val: f32) -> Self {
        Self {
            enabled: true,
            focused: false,
            redraw: false,
            rect,
            value: 0.0,
            min_val,
            max_val,
            dragging: false,
            step: 1.0,
        }
    }

    /// Process input and return `WidgetEvent::ValueChanged(mapped)` when the value changes.
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.redraw = false;
        if !self.enabled {
            return WidgetEvent::None;
        }
        let before = (self.value.to_bits(), self.dragging);
        let result = self.update_inner(input);
        self.redraw = before != (self.value.to_bits(), self.dragging);
        result
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.dragging = false;
        }
    }
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn last_update_redraw(&self) -> bool {
        self.redraw
    }
    pub fn set_step(&mut self, step: f32) {
        if step.is_finite() && step > 0.0 {
            self.step = step;
        }
    }
    pub fn key(&mut self, input: &crate::KeyInput<'_>) -> WidgetEvent {
        self.redraw = false;
        if !self.enabled || !self.focused || self.max_val <= self.min_val {
            return WidgetEvent::None;
        }
        let delta = match input.key {
            crate::Key::Left => -self.step,
            crate::Key::Right => self.step,
            _ => return WidgetEvent::None,
        };
        let mapped = (self.mapped_value() + delta).clamp(self.min_val, self.max_val);
        let value = (mapped - self.min_val) / (self.max_val - self.min_val);
        if self.value == value {
            return WidgetEvent::None;
        }
        self.value = value;
        self.redraw = true;
        WidgetEvent::ValueChanged(mapped)
    }
    fn update_inner(&mut self, input: &InputState) -> WidgetEvent {
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
        if self.focused && self.enabled {
            crate::widget::draw_focus_ring(fb, &self.rect, theme);
        }
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
