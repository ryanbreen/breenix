//! Horizontal tab bar widget for multi-view applications.

use alloc::vec::Vec;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::{InputState, WidgetEvent};
use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;

/// A horizontal tab bar with clickable tab labels.
///
/// Tracks which tab is selected and returns `WidgetEvent::ValueChanged(index)`
/// when the user clicks a different tab.
pub struct TabBar {
    rect: Rect,
    labels: Vec<&'static [u8]>,
    selected: usize,
    hovered: Option<usize>,
}

impl TabBar {
    /// Create a tab bar occupying `rect` with the given tab labels.
    pub fn new(rect: Rect, labels: Vec<&'static [u8]>) -> Self {
        Self {
            rect,
            labels,
            selected: 0,
            hovered: None,
        }
    }

    /// The index of the currently selected tab.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Programmatically select a tab. Returns false if index is out of range.
    pub fn set_selected(&mut self, index: usize) -> bool {
        if index < self.labels.len() {
            self.selected = index;
            true
        } else {
            false
        }
    }

    /// Update the tab bar's bounding rectangle (e.g. after a window resize).
    pub fn set_rect(&mut self, rect: Rect) {
        self.rect = rect;
    }

    /// Number of tabs.
    pub fn count(&self) -> usize {
        self.labels.len()
    }

    /// Add a tab with the given label. Returns the new tab's index.
    pub fn add_tab(&mut self, label: &'static [u8]) -> usize {
        self.labels.push(label);
        self.labels.len() - 1
    }

    /// Remove a tab by index. Adjusts selected index if needed.
    pub fn remove_tab(&mut self, index: usize) {
        if index >= self.labels.len() { return; }
        self.labels.remove(index);
        if self.selected >= self.labels.len() && !self.labels.is_empty() {
            self.selected = self.labels.len() - 1;
        }
    }

    /// The content area below the tab bar (everything except the tab row).
    pub fn content_rect(&self, full_rect: &Rect) -> Rect {
        Rect::new(
            full_rect.x,
            full_rect.y + self.rect.h,
            full_rect.w,
            full_rect.h - self.rect.h,
        )
    }

    fn tab_rect(&self, index: usize) -> Rect {
        let n = self.labels.len().max(1) as i32;
        let tab_w = self.rect.w / n;
        Rect::new(
            self.rect.x + (index as i32) * tab_w,
            self.rect.y,
            tab_w,
            self.rect.h,
        )
    }

    fn hit_test(&self, px: i32, py: i32) -> Option<usize> {
        if !self.rect.contains(px, py) { return None; }
        for i in 0..self.labels.len() {
            if self.tab_rect(i).contains(px, py) {
                return Some(i);
            }
        }
        None
    }

    /// Process input. Returns `ValueChanged(new_index)` if a different tab was clicked.
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.hovered = self.hit_test(input.mouse_x, input.mouse_y);

        if input.mouse_released {
            if let Some(idx) = self.hovered {
                if idx != self.selected {
                    self.selected = idx;
                    return WidgetEvent::ValueChanged(idx as f32);
                }
            }
        }

        WidgetEvent::None
    }

    /// Draw the tab bar.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        // Background
        shapes::fill_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.panel_bg);

        for (i, label) in self.labels.iter().enumerate() {
            let r = self.tab_rect(i);
            let is_selected = i == self.selected;
            let is_hovered = self.hovered == Some(i);

            // Tab background
            let bg = if is_selected {
                theme.widget_bg_active
            } else if is_hovered {
                theme.widget_bg_hover
            } else {
                theme.widget_bg
            };
            shapes::fill_rect(fb, r.x, r.y, r.w, r.h, bg);

            // Bottom highlight for selected tab
            if is_selected {
                shapes::fill_rect(fb, r.x, r.y + r.h - 2, r.w, 2, theme.accent);
            }

            // Separator between tabs
            if i > 0 {
                let sep_color = Color::rgb(80, 80, 80);
                shapes::fill_rect(fb, r.x, r.y + 2, 1, r.h - 4, sep_color);
            }

            // Label
            let text_color = if is_selected {
                Color::WHITE
            } else {
                theme.text_secondary
            };
            text::draw_text_centered(fb, label, &r, text_color, theme);
        }
    }
}
