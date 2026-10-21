//! Ordered per-window retries when a client applies input backpressure (#2163).
use std::collections::{BTreeMap, VecDeque};

pub const MAX_PENDING: usize = 1024;

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
    windows: BTreeMap<u32, Option<VecDeque<T>>>,
    close_requests: BTreeMap<u32, T>,
}

impl<T> Backlog<T> {
    pub const fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            close_requests: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.close_requests.is_empty()
            && self.windows.values().all(|queue| queue.as_ref().map_or(true, |q| q.is_empty()))
    }

    pub fn retain_windows(&mut self, mut exists: impl FnMut(u32) -> bool) {
        self.windows.retain(|&id, _| exists(id));
        self.close_requests.retain(|&id, _| exists(id));
    }

    /// Only adjacent events accepted by `coalesce` may replace one another.
    /// Overflow disables this window's input until it leaves the registry; the
    /// caller reports it once. Keep a failed stream disabled to preserve barriers.
    pub fn push(
        &mut self,
        window: u32,
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
            *entry = None;
            return Admission::Overflow;
        } else {
            queue.push_back(event);
        }
        Admission::Queued
    }

    /// Reserve one close request outside a disabled input stream. Repeated close
    /// hotkeys replace the same request, so a stalled client cannot grow it.
    pub fn request_close(&mut self, window: u32, event: T) {
        self.close_requests.insert(window, event);
    }

    /// A full window does not prevent another window's delivery. Retry EAGAIN
    /// heads only; a permanent error disables delivery until registry removal.
    pub fn flush(&mut self, mut send: impl FnMut(u32, &T) -> Delivery) {
        self.windows.retain(|&id, entry| {
            let Some(queue) = entry else {
                return true;
            };
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
