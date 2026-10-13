//! Ownership of a single device-owned DMA slot. Detaching a killed waiter
//! does not release the slot: a later caller must consume its completion first.
use core::sync::atomic::{AtomicU8, Ordering};

const FREE: u8 = 0;
const OWNED: u8 = 1;
const DETACHED: u8 = 2;
const WEDGED: u8 = 3;

pub(super) struct RequestState(AtomicU8);

impl RequestState {
    pub(super) const fn new() -> Self { Self(AtomicU8::new(FREE)) }

    pub(super) fn try_acquire(&self) -> bool {
        self.0.compare_exchange(FREE, OWNED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// Only one contender may consume the detached request's completion.
    pub(super) fn try_reap(&self, completed: bool) -> bool {
        completed && self.0.compare_exchange(DETACHED, OWNED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub(super) fn available(&self, completed: bool) -> bool {
        match self.0.load(Ordering::Acquire) { FREE => true, DETACHED => completed, _ => false }
    }

    pub(super) fn detach(&self) { self.0.store(DETACHED, Ordering::Release); }
    pub(super) fn release(&self) { self.0.store(FREE, Ordering::Release); }
    pub(super) fn wedge(&self) { self.0.store(WEDGED, Ordering::Release); }
    pub(super) fn is_wedged(&self) -> bool { self.0.load(Ordering::Acquire) == WEDGED }
}
