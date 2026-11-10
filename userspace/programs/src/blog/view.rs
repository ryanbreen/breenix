//! Pure navigation and search state for a log tab.
#[derive(Clone, Copy)]
pub enum Command { Up, Down, PageUp, PageDown, Top, Bottom, ToggleFollow, Wheel(i32), Search, Character(u8), Backspace, Enter, Escape, Next, Previous }

pub struct View {
    pub offset: usize,
    pub follow: bool,
    pub query: String,
    pub editing: bool,
    pub matched: Option<usize>,
    pub matches: usize,
    draft: String,
}
impl View {
    pub fn new() -> Self { Self { offset: 0, follow: true, query: String::new(), editing: false, matched: None, matches: 0, draft: String::new() } }
    pub fn draft(&self) -> &str { &self.draft }
    pub fn clamp(&mut self, count: usize, visible: usize) { self.offset = self.offset.min(count.saturating_sub(visible)); }
    pub fn bottom(&mut self, count: usize, visible: usize) { self.offset = count.saturating_sub(visible); }
    pub fn refresh(&mut self, lines: &[&[u8]], visible: usize) {
        self.clamp(lines.len(), visible);
        self.matches = if self.query.is_empty() { 0 } else { lines.iter().filter(|line| line.windows(self.query.len()).any(|w| w == self.query.as_bytes())).count() };
        if self.matched.is_some_and(|i| i >= lines.len() || self.query.is_empty() || !lines[i].windows(self.query.len()).any(|w| w == self.query.as_bytes())) { self.matched = None; }
    }
    pub fn apply(&mut self, cmd: Command, lines: &[&[u8]], visible: usize) {
        let max = lines.len().saturating_sub(visible);
        if self.editing {
            match cmd {
                Command::Character(c) if c.is_ascii_graphic() || c == b' ' => self.draft.push(c as char),
                Command::Backspace => { self.draft.pop(); },
                Command::Escape => self.editing = false,
                Command::Enter => { self.query = self.draft.clone(); self.editing = false; self.find(lines, visible, 0); },
                _ => {}
            }
            return;
        }
        match cmd {
            Command::Up => { self.offset = self.offset.saturating_sub(1); self.follow = false; },
            Command::Down => { self.offset = self.offset.saturating_add(1).min(max); self.follow = false; },
            Command::PageUp => { self.offset = self.offset.saturating_sub(visible); self.follow = false; },
            Command::PageDown => { self.offset = self.offset.saturating_add(visible).min(max); self.follow = false; },
            Command::Top => { self.offset = 0; self.follow = false; },
            Command::Bottom => { self.offset = max; self.follow = true; },
            Command::ToggleFollow => { self.follow = !self.follow; if self.follow { self.offset = max; } },
            Command::Wheel(y) => { let n = (y.unsigned_abs() as usize).saturating_mul(3); if y > 0 { self.offset = self.offset.saturating_sub(n); } else { self.offset = self.offset.saturating_add(n).min(max); } self.follow = false; },
            Command::Search => { self.draft.clear(); self.editing = true; },
            Command::Next => self.find(lines, visible, 1),
            Command::Previous => self.find(lines, visible, -1),
            _ => {}
        }
        self.clamp(lines.len(), visible);
    }
    fn find(&mut self, lines: &[&[u8]], visible: usize, direction: i8) {
        let hits: Vec<usize> = if self.query.is_empty() { Vec::new() } else { lines.iter().enumerate().filter_map(|(i, line)| line.windows(self.query.len()).any(|w| w == self.query.as_bytes()).then_some(i)).collect() };
        self.matches = hits.len();
        self.matched = if hits.is_empty() { None } else if direction < 0 { let at = self.matched.unwrap_or(self.offset); Some(*hits.iter().rev().find(|&&i| i < at).unwrap_or(hits.last().unwrap())) } else { let at = if direction > 0 { self.matched.map_or(self.offset, |i| i.saturating_add(1)) } else { self.offset }; Some(*hits.iter().find(|&&i| i >= at).unwrap_or(&hits[0])) };
        if let Some(row) = self.matched { if row < self.offset { self.offset = row; } else if row >= self.offset.saturating_add(visible) { self.offset = row.saturating_add(1).saturating_sub(visible); } self.clamp(lines.len(), visible); self.follow = false; }
    }
}
