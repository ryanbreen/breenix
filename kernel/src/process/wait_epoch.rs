use core::sync::atomic::{AtomicU64, Ordering};

/// Invalidation generation shared by process-manager holders and quiet waits.
pub(super) struct WaitEpoch(AtomicU64);

impl WaitEpoch {
    pub(super) const fn new() -> Self { Self(AtomicU64::new(0)) }

    pub(super) fn load(&self) -> u64 { self.0.load(Ordering::Acquire) }

    pub(super) fn release(&self, mutable_access: bool) {
        if mutable_access { self.0.fetch_add(1, Ordering::Release); }
    }
}
