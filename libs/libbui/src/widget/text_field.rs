//! Single-line Unicode scalar editing with selection and horizontal scrolling.
use crate::input::{Key, KeyInput};
use crate::{text, widget};
use crate::{InputState, Rect, Theme, WidgetEvent};
use alloc::string::String;
use libgfx::surface::Surface as FrameBuf;
use crate::drawing as shapes;

pub struct TextField {
    pub rect: Rect,
    text: String,
    cursor: usize,
    anchor: usize,
    max_chars: usize,
    scroll: usize,
    focused: bool,
    enabled: bool,
    redraw: bool,
}
impl TextField {
    pub fn new(rect: Rect, max_chars: usize) -> Self {
        Self {
            rect,
            text: String::new(),
            cursor: 0,
            anchor: 0,
            max_chars,
            scroll: 0,
            focused: false,
            enabled: true,
            redraw: false,
        }
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }
    pub fn cursor_x(&self) -> i32 {
        self.rect.x + 4 + self.width_between(self.scroll, self.cursor)
    }
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn last_update_redraw(&self) -> bool {
        self.redraw
    }
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.cursor != self.anchor)
            .then_some((self.cursor.min(self.anchor), self.cursor.max(self.anchor)))
    }
    fn byte(&self, index: usize) -> usize {
        self.text
            .char_indices()
            .nth(index)
            .map_or(self.text.len(), |(i, _)| i)
    }
    pub fn selected_text(&self) -> &str {
        self.selection()
            .map_or("", |(a, b)| &self.text[self.byte(a)..self.byte(b)])
    }
    fn width_between(&self, first:usize, end:usize)->i32 {
        text::text_width(self.text[self.byte(first)..self.byte(end)].as_bytes(), &Theme::dark())
    }
    fn reveal(&mut self) {
        self.scroll = self.scroll.min(self.cursor);
        while self.scroll < self.cursor && self.width_between(self.scroll,self.cursor) >= (self.rect.w-8).max(0) {self.scroll+=1;}
    }
    /// Refuse the entire insertion if its resulting scalar count exceeds the limit.
    pub fn insert_str(&mut self, value: &str) -> bool {
        self.redraw = false;
        if !self.enabled {
            return false;
        }
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let count = value.chars().count();
        if self.text.chars().count() - (b - a) + count > self.max_chars {
            return false;
        }
        if a == b && value.is_empty() {
            return true;
        }
        let range = self.byte(a)..self.byte(b);
        self.text.replace_range(range, value);
        self.cursor = a + count;
        self.anchor = self.cursor;
        self.reveal();
        self.redraw = true;
        true
    }
    pub fn key(&mut self, input: &KeyInput<'_>) -> WidgetEvent {
        self.redraw = false;
        if !self.focused || !self.enabled {
            return WidgetEvent::None;
        }
        let old = (self.cursor, self.anchor, self.scroll);
        let len = self.text.chars().count();
        match input.key {
            Key::Char if !input.ctrl && !input.alt => {
                self.insert_str(input.text);
            }
            Key::Space if !input.ctrl && !input.alt => {
                self.insert_str(" ");
            }
            Key::Backspace | Key::Delete => {
                if self.selection().is_none() {
                    if input.key == Key::Backspace && self.cursor > 0 {
                        self.anchor = self.cursor - 1;
                    }
                    if input.key == Key::Delete && self.cursor < len {
                        self.anchor = self.cursor + 1;
                    }
                }
                if self.selection().is_some() {
                    self.insert_str("");
                }
            }
            Key::Left | Key::Right | Key::Home | Key::End => {
                self.cursor = match input.key {
                    Key::Home => 0,
                    Key::End => len,
                    Key::Left if !input.shift && self.selection().is_some() => {
                        self.cursor.min(self.anchor)
                    }
                    Key::Right if !input.shift && self.selection().is_some() => {
                        self.cursor.max(self.anchor)
                    }
                    Key::Left => self.cursor.saturating_sub(1),
                    _ => (self.cursor + 1).min(len),
                };
                if !input.shift {
                    self.anchor = self.cursor;
                }
            }
            _ => {}
        }
        self.reveal();
        self.redraw |= old != (self.cursor, self.anchor, self.scroll);
        WidgetEvent::None
    }
    pub fn update(&mut self, input: &InputState) -> WidgetEvent {
        self.redraw = false;
        if self.enabled && input.mouse_pressed {
            let focused = self.rect.contains(input.mouse_x, input.mouse_y);
            self.redraw = focused != self.focused;
            self.focused = focused;
            if focused {
                let mut col=0;
                let local=(input.mouse_x-self.rect.x-4).max(0);
                while self.scroll+col<self.text.chars().count() && self.width_between(self.scroll,self.scroll+col+1)<=local {col+=1;}
                let cursor = (self.scroll + col).min(self.text.chars().count());
                self.redraw |= self.cursor != cursor || self.anchor != cursor;
                self.cursor = cursor;
                self.anchor = cursor;
                self.reveal();
            }
        }
        WidgetEvent::None
    }
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        shapes::fill_rect(
            fb,
            self.rect.x,
            self.rect.y,
            self.rect.w,
            self.rect.h,
            theme.widget_bg,
        );
        shapes::draw_rect(
            fb,
            self.rect.x,
            self.rect.y,
            self.rect.w,
            self.rect.h,
            theme.border,
        );
        let y = self.rect.y + (self.rect.h - 16) / 2;
        crate::drawing::push_clip(fb, libgfx::surface::Rect::new(self.rect.x+4,self.rect.y+1,(self.rect.w-8).max(0),(self.rect.h-2).max(0)));
        for (i,ch) in self.text.chars().enumerate().skip(self.scroll) {
            let x = self.rect.x + 4 + self.width_between(self.scroll,i);
            let width=self.width_between(self.scroll,i+1)-self.width_between(self.scroll,i);
            if x+width>self.rect.x+self.rect.w-4 {break;}
            if self.selection().is_some_and(|(a,b)|i>=a && i<b) { shapes::fill_rect(fb,x,y,width,16,theme.accent); }
            let mut buf=[0;4];
            text::draw_text(fb,ch.encode_utf8(&mut buf).as_bytes(),x,y,
                if self.enabled {theme.text_primary}else{theme.text_secondary},theme);
        }
        fb.pop_clip();
        if self.focused && self.enabled {
            shapes::fill_rect(fb, self.cursor_x(), y, 1, 16, theme.text_primary);
            widget::draw_focus_ring(fb, &self.rect, theme);
        }
    }
}
