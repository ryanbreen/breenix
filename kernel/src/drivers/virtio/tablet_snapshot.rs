//! Retain a tablet release position for the consuming pointer snapshot.
use core::sync::atomic::{AtomicU64, Ordering};

const LEFT: u64 = 1 << 48;
const RELEASE: u64 = 1 << 63;

pub struct TabletSnapshot {
    current: AtomicU64,
    released: AtomicU64,
}

impl TabletSnapshot {
    pub const fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
            released: AtomicU64::new(0),
        }
    }

    /// The tablet IRQ is the producer. Preserve the position at button-up
    /// before publishing the new level; subsequent unheld motion stays current.
    pub fn report(&self, x: u32, y: u32, buttons: u32) {
        let next = x as u64 | (y as u64) << 24 | (buttons as u64) << 48;
        let old = self.current.load(Ordering::Relaxed);
        if old & LEFT != 0 && next & LEFT == 0 {
            self.released.store(next | RELEASE, Ordering::Release);
        }
        self.current.store(next, Ordering::Release);
    }

    pub fn current(&self) -> (u32, u32, u32) {
        unpack(self.current.load(Ordering::Acquire))
    }

    /// Return the release position before motion that followed button-up.
    pub fn take(&self) -> (u32, u32, u32) {
        // Load current first: observing the released level also observes its
        // earlier boundary publication, even if the IRQ races this read.
        let current = self.current.load(Ordering::Acquire);
        let released = self.released.swap(0, Ordering::AcqRel);
        unpack(if released != 0 { released } else { current })
    }
}

fn unpack(value: u64) -> (u32, u32, u32) {
    (
        (value & 0xff_ffff) as u32,
        ((value >> 24) & 0xff_ffff) as u32,
        ((value >> 48) & 0xff) as u32,
    )
}
