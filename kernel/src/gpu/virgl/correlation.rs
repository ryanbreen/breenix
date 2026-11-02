//! Bounded correlation storage, independent of DMA ownership and queue policy.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub at_ns: u64,
    pub epoch: u64,
    pub entry: u64,
    pub fence: u64,
    pub serial: u64,
    pub detail: u64,
    pub context: u32,
    pub command: u32,
    pub head: u16,
    pub stage: u16,
    pub used: u16,
    pub taken: u16,
}

impl Record {
    fn words(self) -> [u64; 8] {
        [self.at_ns, self.epoch, self.entry, self.fence, self.serial, self.detail,
         u64::from(self.context) | (u64::from(self.command) << 32),
         u64::from(self.head) | (u64::from(self.stage) << 16)
             | (u64::from(self.used) << 32) | (u64::from(self.taken) << 48)]
    }
    fn from_words(w: [u64; 8]) -> Self {
        Self { at_ns: w[0], epoch: w[1], entry: w[2], fence: w[3], serial: w[4],
            detail: w[5], context: w[6] as u32, command: (w[6] >> 32) as u32,
            head: w[7] as u16, stage: (w[7] >> 16) as u16,
            used: (w[7] >> 32) as u16, taken: (w[7] >> 48) as u16 }
    }
}

struct Slot {
    busy: AtomicBool,
    ticket: AtomicU64,
    words: [AtomicU64; 8],
}
impl Slot {
    const fn new() -> Self {
        Self { busy: AtomicBool::new(false), ticket: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; 8] }
    }
}

/// Recording uses one try-claim. A colliding writer loses its event and increments
/// `lost`. Tickets expose overwritten prefixes and missing records.
pub struct Trace<const N: usize> {
    next: AtomicU64,
    lost: AtomicU64,
    frozen: AtomicBool,
    slots: [Slot; N],
}
impl<const N: usize> Trace<N> {
    pub const fn new() -> Self {
        assert!(N > 0);
        Self { next: AtomicU64::new(0), lost: AtomicU64::new(0),
            frozen: AtomicBool::new(false), slots: [const { Slot::new() }; N] }
    }
    pub fn record(&self, record: Record) {
        if self.frozen.load(Ordering::Acquire) { return; }
        let ticket = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let slot = &self.slots[(ticket - 1) as usize % N];
        if slot.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // A writer delayed for a full lap must not replace a newer ticket.
        if slot.ticket.load(Ordering::Relaxed) < ticket {
            for (word, value) in slot.words.iter().zip(record.words()) {
                word.store(value, Ordering::Relaxed);
            }
            slot.ticket.store(ticket, Ordering::Relaxed);
        } else {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
        slot.busy.store(false, Ordering::Release);
    }
    pub fn freeze(&self) -> bool { !self.frozen.swap(true, Ordering::AcqRel) }
    pub fn written(&self) -> u64 { self.next.load(Ordering::Acquire) }
    pub fn lost(&self) -> u64 { self.lost.load(Ordering::Acquire) }
    pub fn snapshot(&self) -> [Option<(u64, Record)>; N] {
        let end = self.written();
        let start = end.saturating_sub(N as u64);
        let mut result = [None; N];
        for ticket in start + 1..=end {
            let slot = &self.slots[(ticket - 1) as usize % N];
            if slot.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                continue;
            }
            if slot.ticket.load(Ordering::Relaxed) == ticket {
                result[(ticket - start - 1) as usize] = Some((ticket,
                    Record::from_words(core::array::from_fn(|i| slot.words[i].load(Ordering::Relaxed)))));
            }
            slot.busy.store(false, Ordering::Release);
        }
        result
    }
}
