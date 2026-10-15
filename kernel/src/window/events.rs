//! Window events (section 19): each window's ring of 64 and its overflow accounting.

use alloc::{collections::VecDeque, vec::Vec};

use breengel_abi::{event_kind, limits, Plain, WindowEvent};

/// A window's event ring. A full ring keeps what it holds and counts what it could not
/// queue; the count is delivered as an `Overflow` event after the kept events and before
/// any later one.
pub struct Ring {
    queue: VecDeque<WindowEvent>,
    lost: u64,
}

impl Ring {
    /// An empty ring with room for its 64 events, or `None` when that cannot be allocated.
    pub fn new() -> Option<Ring> {
        let mut queue = VecDeque::new();
        queue.try_reserve_exact(limits::EVENT_RING).ok()?;
        Some(Ring { queue, lost: 0 })
    }

    fn free(&self) -> usize {
        limits::EVENT_RING - self.queue.len()
    }

    /// Queue `event`. False when it was counted as lost instead.
    pub fn push(&mut self, event: WindowEvent) -> bool {
        if self.lost > 0 {
            if self.free() >= 2 {
                self.queue.push_back(overflow(self.lost));
                self.lost = 0;
                self.queue.push_back(event);
                true
            } else {
                self.lost += 1;
                false
            }
        } else if self.free() >= 1 {
            self.queue.push_back(event);
            true
        } else {
            self.lost = 1;
            false
        }
    }

    /// Move up to `capacity` events into `out`, oldest first; when the ring empties with
    /// events lost and room left, an `Overflow` event carrying the count follows them.
    /// Returns the lost count that `Overflow` reported.
    pub fn take(&mut self, out: &mut Vec<WindowEvent>, capacity: usize) -> u64 {
        while out.len() < capacity {
            match self.queue.pop_front() {
                Some(e) => out.push(e),
                None => break,
            }
        }
        if self.queue.is_empty() && self.lost > 0 && out.len() < capacity {
            let lost = self.lost;
            out.push(overflow(lost));
            self.lost = 0;
            return lost;
        }
        0
    }

    /// Put back what `take` removed, when it could not be delivered.
    pub fn untake(&mut self, mut taken: Vec<WindowEvent>, lost: u64) {
        if lost > 0 {
            taken.pop();
            self.lost += lost;
        }
        while let Some(e) = taken.pop() {
            if self.queue.len() == limits::EVENT_RING {
                self.lost += 1;
            } else {
                self.queue.push_front(e);
            }
        }
    }

    pub fn has_event(&self) -> bool {
        !self.queue.is_empty() || self.lost > 0
    }
}

/// The current monotonic time in nanoseconds.
pub fn now_ns() -> u64 {
    let (sec, nsec) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

/// A kernel-made event of `kind`.
pub fn kernel_event(kind: u16) -> WindowEvent {
    let mut e = WindowEvent::ZERO;
    e.kind = kind;
    e.time_ns = now_ns();
    e
}

fn overflow(lost: u64) -> WindowEvent {
    let mut e = kernel_event(event_kind::OVERFLOW);
    e.lost = lost.min(u32::MAX as u64) as u32;
    e
}

/// Whether a compositor may write `event` (section 19.2): a kind of 1 to 14 other than
/// `FontChanged`, which Breengel makes, and `Overflow`, which the kernel makes; text of at
/// most 240 bytes; the reserved field zero.
pub fn writable(event: &WindowEvent) -> bool {
    (1..=event_kind::OVERFLOW).contains(&event.kind)
        && event.kind != event_kind::FONT_CHANGED
        && event.kind != event_kind::OVERFLOW
        && event.text_len as usize <= limits::EVENT_TEXT
        && event.reserved == 0
}
