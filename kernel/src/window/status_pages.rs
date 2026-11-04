//! Shared compositor status and bounded presentation history formatting.
use alloc::{format, string::String};
use core::sync::atomic::{AtomicU64, Ordering};
use breengel_abi::CompositorStatus;

pub fn format_status(s: &CompositorStatus) -> String {
    format!("claim_epoch={} epoch={} handled_serial={} presented={} scene_version={} composites={} next_deadline_ns={} owner={} flags={}\n",
        s.claim_epoch, s.epoch, s.handled_serial, s.presented, s.scene_version,
        s.composites, s.next_deadline_ns, s.owner, s.flags)
}

pub const HISTORY_CAPACITY: usize = 4096;
#[derive(Clone, Copy)]
pub struct HistoryEntry {
    pub composite_serial: u64, pub presented_ns: u64, pub window_id: u64,
    pub commit_id: u64, pub pixel_generation: u64,
}
struct Slot { version: AtomicU64, fields: [AtomicU64; 5] }
impl Slot {
    const fn new() -> Self { Self { version: AtomicU64::new(0), fields: [const { AtomicU64::new(0) }; 5] } }
}
pub struct CompositeHistory { next: AtomicU64, slots: [Slot; HISTORY_CAPACITY] }
impl CompositeHistory {
    pub const fn new() -> Self { Self { next: AtomicU64::new(0), slots: [const { Slot::new() }; HISTORY_CAPACITY] } }
    // Writers are serialized by the window registry lock. Readers do not take that lock.
    pub fn push(&self, entry: HistoryEntry) {
        let n = self.next.load(Ordering::SeqCst);
        let slot = &self.slots[n as usize % HISTORY_CAPACITY];
        slot.version.store(0, Ordering::SeqCst);
        for (field, value) in slot.fields.iter().zip([entry.composite_serial, entry.presented_ns, entry.window_id, entry.commit_id, entry.pixel_generation]) {
            field.store(value, Ordering::SeqCst);
        }
        slot.version.store(n + 1, Ordering::SeqCst);
        self.next.store(n + 1, Ordering::SeqCst);
    }
    pub fn format(&self) -> String {
        let end = self.next.load(Ordering::SeqCst);
        let mut text = String::new();
        for n in end.saturating_sub(HISTORY_CAPACITY as u64)..end {
            let slot = &self.slots[n as usize % HISTORY_CAPACITY];
            if slot.version.load(Ordering::SeqCst) != n + 1 { continue; }
            let values = slot.fields.each_ref().map(|field| field.load(Ordering::SeqCst));
            if slot.version.load(Ordering::SeqCst) == n + 1 {
                text.push_str(&format!("{} {} {} {} {}\n", values[0], values[1], values[2], values[3], values[4]));
            }
        }
        text
    }
}
