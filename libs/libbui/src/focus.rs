//! Focus traversal in insertion order (BreenGL §15).
use crate::input::{Key, KeyInput};
use crate::{InputState, Rect};
use alloc::vec::Vec;

struct Entry {
    rect: Rect,
    enabled: bool,
}
pub struct FocusChain {
    entries: Vec<Entry>,
    focused: Option<usize>,
}
impl Default for FocusChain {
    fn default() -> Self {
        Self::new()
    }
}
impl FocusChain {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            focused: None,
        }
    }
    pub fn add(&mut self, rect: Rect) -> usize {
        let id = self.entries.len();
        self.entries.push(Entry {
            rect,
            enabled: true,
        });
        id
    }
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }
    pub fn set_enabled(&mut self, id: usize, enabled: bool) {
        if let Some(e) = self.entries.get_mut(id) {
            e.enabled = enabled;
        }
        if !enabled && self.focused == Some(id) {
            self.focused = None;
        }
    }
    pub fn focus(&mut self, id: usize) -> bool {
        if self.entries.get(id).is_some_and(|e| e.enabled) {
            self.focused = Some(id);
            true
        } else {
            false
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>) -> Option<usize> {
        if input.key != Key::Tab {
            return self.focused;
        }
        let n = self.entries.len();
        for step in 1..=n {
            let i = match self.focused {
                Some(i) if input.shift => (i + n - step) % n,
                Some(i) => (i + step) % n,
                None if input.shift => n - step,
                None => step - 1,
            };
            if self.entries[i].enabled {
                self.focused = Some(i);
                break;
            }
        }
        self.focused
    }
    pub fn update(&mut self, input: &InputState) -> Option<usize> {
        if input.mouse_pressed {
            if let Some(i) = self
                .entries
                .iter()
                .position(|e| e.enabled && e.rect.contains(input.mouse_x, input.mouse_y))
            {
                self.focused = Some(i);
            }
        }
        self.focused
    }
}
