//! Ordered per-window retries when a client applies input backpressure (#2163).
use std::collections::{BTreeMap, VecDeque};

#[path = "input_limits.rs"]
mod limits;
pub use limits::MAX_PENDING;

#[derive(Clone, Copy)]
pub enum DropReason { Overflow, Gone, Failed }

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
    discarded: Vec<(u64, T, DropReason)>,
}

impl<T> Backlog<T> {
    pub const fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            close_requests: BTreeMap::new(),
            lost: BTreeMap::new(),
            discarded: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.close_requests.is_empty()
            && self.windows.values().all(|queue| queue.as_ref().map_or(true, |q| q.is_empty()))
    }

    pub fn retain_windows(&mut self, mut exists: impl FnMut(u64) -> bool) {
        let discarded = &mut self.discarded;
        self.windows.retain(|&id, queue| {
            if exists(id) { return true; }
            if let Some(queue) = queue { discarded.extend(queue.drain(..).map(|event| (id, event, DropReason::Gone))); }
            false
        });
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
            self.discarded.push((window, queue.pop_front().unwrap(), DropReason::Overflow));
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

    /// Source-aware callers terminate every event that leaves retention.
    pub fn take_discarded(&mut self) -> Vec<(u64, T, DropReason)> {
        std::mem::take(&mut self.discarded)
    }
    pub fn pending(&self) -> impl Iterator<Item = &T> {
        self.windows.values().filter_map(|q| q.as_ref()).flat_map(|q| q.iter())
            .chain(self.close_requests.values())
    }

    /// A full window does not prevent another window's delivery. Retry EAGAIN
    /// heads only; a permanent error disables delivery until registry removal.
    pub fn flush(&mut self, mut send: impl FnMut(u64, &T) -> Delivery, overflow: impl Fn(u32) -> T) {
        let lost = &mut self.lost;
        let discarded = &mut self.discarded;
        self.windows.retain(|&id, entry| {
            let Some(queue) = entry else {
                return true;
            };
            if let Some(&count) = lost.get(&id) {
                match send(id, &overflow(count)) {
                    Delivery::Accepted => { lost.remove(&id); }
                    Delivery::Full => return true,
                    Delivery::Gone => { lost.remove(&id); discarded.extend(queue.drain(..).map(|event| (id, event, DropReason::Gone))); return false; }
                    Delivery::Failed => { lost.remove(&id); discarded.extend(queue.drain(..).map(|event| (id, event, DropReason::Failed))); *entry = None; return true; }
                }
            }
            while let Some(event) = queue.front() {
                match send(id, event) {
                    Delivery::Accepted => {
                        queue.pop_front();
                    }
                    Delivery::Full => return true,
                    Delivery::Gone => {
                        discarded.extend(queue.drain(..).map(|event| (id, event, DropReason::Gone)));
                        return false;
                    }
                    Delivery::Failed => {
                        discarded.extend(queue.drain(..).map(|event| (id, event, DropReason::Failed)));
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
