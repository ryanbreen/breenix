//! Window policy independent of the compositor and event transport.

pub(super) struct Stack {
    order: Vec<u64>,
    creation: Vec<u64>,
    chromeless: Vec<u64>,
    minimized: Vec<u64>,
    focus: Option<u64>,
}

impl Stack {
    pub(super) fn new() -> Self {
        Self { order: Vec::new(), creation: Vec::new(), chromeless: Vec::new(), minimized: Vec::new(), focus: None }
    }
    pub(super) fn adopt(&mut self, windows: &[(u64, u32, bool, bool, bool)]) {
        let mut windows = windows.to_vec();
        windows.sort_by_key(|&(id, z, _, _, _)| (z, id));
        self.order.clear();
        self.creation.clear();
        self.chromeless.clear();
        self.minimized.clear();
        self.focus = None;
        for (id, _, minimized, focused, chromeless) in windows {
            self.order.push(id);
            self.creation.push(id);
            if minimized { self.minimized.push(id); }
            if chromeless { self.chromeless.push(id); }
            if focused && !minimized { self.focus = Some(id); }
        }
    }
    pub(super) fn order(&self) -> &[u64] { &self.order }
    pub(super) fn focus(&self) -> Option<u64> { self.focus }
    pub(super) fn clear_focus(&mut self) -> (Option<u64>, Option<u64>) { self.transition(None) }
    pub(super) fn minimized(&self, id: u64) -> bool { self.minimized.contains(&id) }
    /// Published z, minimized and focused state for a registered id.
    pub(super) fn state(&self, id: u64) -> Option<(u32, bool, bool)> {
        let z = self.order.iter().position(|&item| item == id)? as u32;
        let minimized = self.minimized(id);
        Some((z, minimized, !minimized && self.focus == Some(id)))
    }
    fn transition(&mut self, next: Option<u64>) -> (Option<u64>, Option<u64>) {
        let old = self.focus;
        self.focus = next;
        (old, next)
    }
    fn top(&self) -> Option<u64> {
        self.order.iter().rev().copied().find(|id| !self.minimized(*id))
    }
    pub(super) fn add(&mut self, id: u64, chromeless: bool) -> (Option<u64>, Option<u64>) {
        if self.order.contains(&id) { return (self.focus, self.focus); }
        self.creation.push(id);
        if chromeless { self.chromeless.push(id); }
        self.order.push(id);
        self.raise(id)
    }
    pub(super) fn raise(&mut self, id: u64) -> (Option<u64>, Option<u64>) {
        let Some(index) = self.order.iter().position(|&item| item == id) else { return (self.focus, self.focus); };
        if self.minimized(id) { return (self.focus, self.focus); }
        self.order.remove(index);
        let index = if self.chromeless.contains(&id) { self.order.len() } else {
            self.order.iter().position(|item| self.chromeless.contains(item)).unwrap_or(self.order.len())
        };
        self.order.insert(index, id);
        self.transition(Some(id))
    }
    pub(super) fn cycle(&self, reverse: bool) -> Option<u64> {
        let visible: Vec<_> = self.creation.iter().copied().filter(|id| !self.minimized(*id)).collect();
        if visible.is_empty() { return None; }
        let current = visible.iter().position(|id| Some(*id) == self.focus);
        let index = match current {
            Some(i) if reverse => (i + visible.len() - 1) % visible.len(),
            Some(i) => (i + 1) % visible.len(),
            None if reverse => visible.len() - 1,
            None => 0,
        };
        Some(visible[index])
    }
    pub(super) fn minimize(&mut self, id: u64) -> (Option<u64>, Option<u64>) {
        if !self.order.contains(&id) || self.minimized(id) { return (self.focus, self.focus); }
        self.minimized.push(id);
        let next = if self.focus == Some(id) { self.top() } else { self.focus };
        self.transition(next)
    }
    pub(super) fn restore(&mut self, id: u64) -> (Option<u64>, Option<u64>) {
        self.minimized.retain(|&item| item != id);
        self.raise(id)
    }
    pub(super) fn remove(&mut self, id: u64) -> (Option<u64>, Option<u64>) {
        self.order.retain(|&item| item != id);
        self.creation.retain(|&item| item != id);
        self.chromeless.retain(|&item| item != id);
        self.minimized.retain(|&item| item != id);
        let next = if self.focus == Some(id) { self.top() } else { self.focus };
        self.transition(next)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FocusNotice { Lost(u64), Gained(u64) }

pub(super) fn publish_focus(
    (old, next): (Option<u64>, Option<u64>),
    mut send: impl FnMut(FocusNotice),
    publish: impl FnOnce(),
) {
    if old != next {
        if let Some(id) = old { send(FocusNotice::Lost(id)); }
    }
    publish();
    if old != next {
        if let Some(id) = next { send(FocusNotice::Gained(id)); }
    }
}
