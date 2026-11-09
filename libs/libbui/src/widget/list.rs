//! A vertically scrolling, single-selection list with caller-supplied item keys.
use alloc::{format, string::String, vec::Vec};
use libgfx::surface::Surface as FrameBuf;
use crate::{drawing, text, InputState, Key, KeyInput, Rect, Theme, WidgetEvent};
use crate::tree::{Flag, Node, Role};
use super::Widget;

pub struct ListItem {
    pub key: String,
    pub name: String,
}
impl ListItem {
    pub fn new(key: &str, name: &str) -> Self {
        Self { key: key.into(), name: name.into() }
    }
}

pub struct List {
    rect: Rect,
    items: Vec<ListItem>,
    row_height: i32,
    offset: i32,
    selected: Option<usize>,
    enabled: bool,
    focused: bool,
    redraw: bool,
}
impl List {
    pub fn new(rect: Rect, row_height: i32, items: Vec<ListItem>) -> Self {
        Self { rect, items, row_height: row_height.max(1), offset: 0, selected: None,
            enabled: true, focused: false, redraw: false }
    }
    pub fn selected(&self) -> Option<usize> { self.selected }
    pub fn offset(&self) -> i32 { self.offset }
    pub fn last_update_redraw(&self) -> bool { self.redraw }
    pub fn set_enabled(&mut self, enabled: bool) { self.enabled = enabled; }
    pub fn set_focused(&mut self, focused: bool) { self.focused = focused; }
    pub fn set_rect(&mut self, rect: Rect) {
        if self.rect != rect {
            self.rect = rect;
            self.set_offset(self.offset);
            crate::layout::layout_changed();
            self.redraw = true;
        }
    }
    fn max_offset(&self) -> i32 {
        (self.items.len() as i64 * i64::from(self.row_height) - i64::from(self.rect.h.max(0)))
            .clamp(0, i64::from(i32::MAX)) as i32
    }
    pub fn set_offset(&mut self, offset: i32) {
        let offset = offset.clamp(0, self.max_offset());
        if self.offset != offset {
            self.offset = offset;
            self.redraw = true;
            crate::layout::layout_changed();
        }
    }
    /// Wheel direction follows ScrollBar: positive up, 40 pixels per unit.
    pub fn scroll(&mut self, delta_y: i32) {
        self.redraw = false;
        if self.enabled {
            self.set_offset((i64::from(self.offset) - i64::from(delta_y) * 40)
                .clamp(0, i64::from(self.max_offset())) as i32);
        }
    }
    fn item_rect(&self, i: usize) -> Rect {
        let y = (i64::from(self.rect.y) + i as i64 * i64::from(self.row_height) - i64::from(self.offset))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        Rect::new(self.rect.x, y, self.rect.w, self.row_height)
    }
    fn select(&mut self, i: usize) -> WidgetEvent {
        if self.selected == Some(i) { return WidgetEvent::None; }
        self.selected = Some(i);
        self.redraw = true;
        WidgetEvent::ValueChanged(i as f32)
    }
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.redraw = false;
        if !self.enabled || !input.mouse_pressed || !self.rect.contains(input.mouse_x, input.mouse_y) {
            return WidgetEvent::None;
        }
        let i = ((i64::from(input.mouse_y) - i64::from(self.rect.y) + i64::from(self.offset))
            / i64::from(self.row_height)) as usize;
        if i < self.items.len() { self.select(i) } else { WidgetEvent::None }
    }
    pub fn key(&mut self, input: &KeyInput<'_>) -> WidgetEvent {
        self.redraw = false;
        if !self.enabled || !self.focused || self.items.is_empty() { return WidgetEvent::None; }
        let last = self.items.len() - 1;
        let i = match input.key {
            Key::Up => self.selected.map_or(last, |i| i.saturating_sub(1)),
            Key::Down => self.selected.map_or(0, |i| (i + 1).min(last)),
            Key::Home => 0,
            Key::End => last,
            _ => return WidgetEvent::None,
        };
        let event = self.select(i);
        let row = self.item_rect(i);
        if row.y < self.rect.y { self.set_offset(self.offset.saturating_sub(self.rect.y - row.y)); }
        else if row.bottom() > self.rect.bottom() { self.set_offset(self.offset.saturating_add(row.bottom() - self.rect.bottom())); }
        event
    }
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        drawing::push_clip(fb, libgfx::surface::Rect::new(self.rect.x, self.rect.y, self.rect.w, self.rect.h));
        drawing::fill_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.widget_bg);
        for (i, item) in self.items.iter().enumerate() {
            let row = self.item_rect(i);
            if row.intersection(self.rect).h == 0 { continue; }
            if self.selected == Some(i) {
                drawing::fill_rect(fb, row.x, row.y, row.w, row.h, theme.accent);
            }
            text::draw_text(fb, item.name.as_bytes(), row.x + 4,
                row.y + (row.h - text::text_height(theme)) / 2,
                if self.enabled { theme.text_primary } else { theme.text_secondary }, theme);
        }
        fb.pop_clip();
        if self.focused && self.enabled { super::draw_focus_ring(fb, &self.rect, theme); }
    }
}
impl Widget for List {
    type Event = WidgetEvent;
    fn rect(&self) -> Rect { self.rect }
    fn update(&mut self, input: &InputState) -> WidgetEvent { List::update(self, input) }
    fn draw(&self, fb: &mut FrameBuf, theme: &Theme) { List::draw(self, fb, theme); }
    fn tree_node(&self, key: &str, parent: Option<&str>, clip: Rect) -> Node {
        let mut n = super::node(key, parent, Role::List, "", self.rect, clip);
        super::flag(&mut n, Flag::Enabled, self.enabled);
        super::flag(&mut n, Flag::Focused, self.focused);
        n.value = self.selected.map(|i| self.items[i].key.clone()).into();
        n
    }
    fn tree_nodes(&self, key: &str, parent: Option<&str>, clip: Rect, _theme: &Theme) -> Vec<Node> {
        let mut nodes = alloc::vec![self.tree_node(key, parent, clip)];
        let clip = self.rect.intersection(clip);
        for (i, item) in self.items.iter().enumerate() {
            let mut n = super::node(&format!("{key}/{}", item.key), Some(key), Role::Listitem,
                &item.name, self.item_rect(i), clip);
            super::flag(&mut n, Flag::Enabled, self.enabled);
            super::flag(&mut n, Flag::Selected, self.selected == Some(i));
            nodes.push(n);
        }
        nodes
    }
}
