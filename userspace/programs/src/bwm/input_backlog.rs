//! Ordered per-window retries when a client applies input backpressure (#2163).
use std::collections::{BTreeMap,VecDeque};

pub enum Delivery { Accepted, Full, Gone }

pub struct Backlog<T> { windows: BTreeMap<u32,VecDeque<T>> }
impl<T> Backlog<T> {
    pub const fn new() -> Self { Self { windows:BTreeMap::new() } }
    pub fn is_empty(&self) -> bool { self.windows.is_empty() }

    /// Only adjacent events accepted by `coalesce` may replace one another.
    pub fn push(&mut self, window:u32, event:T, coalesce:impl FnOnce(&T,&T)->bool) {
        let queue = self.windows.entry(window).or_default();
        if let Some(last) = queue.back_mut().filter(|last| coalesce(last,&event)) {
            *last = event;
        } else { queue.push_back(event); }
    }

    /// A full window does not prevent another window's delivery. Refused heads stay queued.
    pub fn flush(&mut self, mut send:impl FnMut(u32,&T)->Delivery) {
        self.windows.retain(|&id,queue| {
            while let Some(event) = queue.front() {
                match send(id,event) {
                    Delivery::Accepted => { queue.pop_front(); }
                    Delivery::Full => return true,
                    Delivery::Gone => return false,
                }
            }
            false
        });
    }
}
