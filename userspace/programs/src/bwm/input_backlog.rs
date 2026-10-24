//! Ordered per-window retries when a client applies input backpressure (#2163).
use std::collections::{BTreeMap, VecDeque};

#[path = "input_limits.rs"]
mod input_limits;
pub use input_limits::MAX_PENDING;

pub enum Delivery {
    Accepted,
    Full,
    Gone,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Queued,
    Overflow,
    Disabled,
}

// A failed-window tombstone stays until registry removal so later
// keys cannot overtake the event whose delivery failed.
pub struct Backlog<T> {
    windows: BTreeMap<u64, Option<VecDeque<T>>>,
    close_requests: BTreeMap<u64, T>,
    lost: BTreeMap<u64, u32>,
}

impl<T> Backlog<T> {
    pub const fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            close_requests: BTreeMap::new(),
            lost: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.close_requests.is_empty()
            && self.windows.values().all(|queue| queue.as_ref().map_or(true, |q| q.is_empty()))
    }

    pub fn retain_windows(&mut self, mut exists: impl FnMut(u64) -> bool) {
        self.windows.retain(|&id, _| exists(id));
        self.close_requests.retain(|&id, _| exists(id));
        self.lost.retain(|&id, _| exists(id));
    }

    /// Only adjacent events accepted by `coalesce` may replace one another.
    /// B48: at the bound, drop the oldest event and report its loss to the client.
    /// Keep a permanently failed stream disabled to preserve barriers.
    pub fn push(
        &mut self,
        window: u64,
        event: T,
        coalesce: impl FnOnce(&T, &T) -> bool,
    ) -> Admission {
        let entry = self
            .windows
            .entry(window)
            .or_insert_with(|| Some(VecDeque::new()));
        let Some(queue) = entry else {
            return Admission::Disabled;
        };
        if let Some(last) = queue.back_mut().filter(|last| coalesce(last, &event)) {
            *last = event;
        } else if queue.len() == MAX_PENDING {
            queue.pop_front();
            queue.push_back(event);
            let lost = self.lost.entry(window).or_default();
            *lost = lost.saturating_add(1);
            return Admission::Overflow;
        } else {
            queue.push_back(event);
        }
        Admission::Queued
    }

    /// Reserve one close request outside a disabled input stream. Repeated close
    /// hotkeys replace the same request, so a stalled client cannot grow it.
    pub fn request_close(&mut self, window: u64, event: T) {
        self.close_requests.insert(window, event);
    }

    /// A full window does not prevent another window's delivery. Retry EAGAIN
    /// heads only; a permanent error disables delivery until registry removal.
    pub fn flush(&mut self, mut send: impl FnMut(u64, &T) -> Delivery, overflow: impl Fn(u32) -> T) {
        let lost = &mut self.lost;
        self.windows.retain(|&id, entry| {
            let Some(queue) = entry else {
                return true;
            };
            if let Some(&count) = lost.get(&id) {
                match send(id, &overflow(count)) {
                    Delivery::Accepted => { lost.remove(&id); }
                    Delivery::Full => return true,
                    Delivery::Gone => { lost.remove(&id); return false; }
                    Delivery::Failed => { lost.remove(&id); *entry = None; return true; }
                }
            }
            while let Some(event) = queue.front() {
                match send(id, event) {
                    Delivery::Accepted => {
                        queue.pop_front();
                    }
                    Delivery::Full => return true,
                    Delivery::Gone => return false,
                    Delivery::Failed => {
                        *entry = None;
                        return true;
                    }
                }
            }
            false
        });
        self.close_requests.retain(|&id, event| {
            matches!(send(id, event), Delivery::Full)
        });
    }
}
