//! Changed-inode notifications for deferred touches and final handle release.
use alloc::sync::Arc;
use core::{ptr, sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering}};

// Deferred times are recorded per generation (E24 coordinator 601(2)): slot
// `generation & 1` belongs to the open generation, the other one to a frozen
// generation that its publication applies. A toucher that races the freeze
// lands in the frozen slot only if it read the old generation first.
pub(super) struct TouchedTimes {
    pub ino: u64,
    access: [AtomicU64; 2],
    modify: [AtomicU64; 2],
    queued: AtomicBool,
    next: AtomicPtr<TouchedTimes>,
}
impl TouchedTimes {
    pub fn new(ino: u64) -> Self {
        Self { ino, access: [const { AtomicU64::new(0) }; 2], modify: [const { AtomicU64::new(0) }; 2],
            queued: AtomicBool::new(false), next: AtomicPtr::new(ptr::null_mut()) }
    }
    pub fn pending(&self) -> bool {
        self.access.iter().chain(&self.modify).any(|slot| slot.load(Ordering::Acquire) != 0)
    }
    /// Record a coarse time (seconds + 1) for `generation`.
    pub fn record(&self, generation: u64, read: bool, stamp: u64) {
        let slots = if read { &self.access } else { &self.modify };
        slots[(generation & 1) as usize].store(stamp, Ordering::Release);
    }
    /// Take `generation`'s (access, modify) stamps; 0 means no stamp.
    pub fn take(&self, generation: u64) -> (u64, u64) {
        let slot = (generation & 1) as usize;
        (self.access[slot].swap(0, Ordering::AcqRel), self.modify[slot].swap(0, Ordering::AcqRel))
    }
}
#[derive(Default)]
pub(super) struct NodeEvents(AtomicPtr<TouchedTimes>);
impl NodeEvents {
    // The queue owns one Arc per linked notification. The queued bit permits
    // only its producer to write next until the consumer has read that link.
    // Arc cloning and atomic links require neither allocation nor a lock.
    pub fn push(&self, times: &Arc<TouchedTimes>) {
        if times.queued.swap(true, Ordering::AcqRel) { return; }
        let raw = Arc::into_raw(times.clone()) as *mut TouchedTimes;
        let mut head = self.0.load(Ordering::Acquire);
        loop {
            times.next.store(head, Ordering::Relaxed);
            match self.0.compare_exchange_weak(head, raw, Ordering::Release, Ordering::Acquire) {
                Ok(_) => break,
                Err(current) => head = current,
            }
        }
    }
    pub fn take(&self) -> Events {
        Events(self.0.swap(ptr::null_mut(), Ordering::Acquire))
    }
}
impl Drop for NodeEvents {
    fn drop(&mut self) { for times in self.take() { drop(times); } }
}
pub(super) struct Events(*mut TouchedTimes);
impl Iterator for Events {
    type Item = Arc<TouchedTimes>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() { return None; }
        // SAFETY: push transferred one Arc into this list. take detached it
        // from the sole mount consumer; read next before permitting requeue.
        let times = unsafe { Arc::from_raw(self.0) };
        self.0 = times.next.load(Ordering::Relaxed);
        times.queued.store(false, Ordering::Release);
        Some(times)
    }
}
impl Drop for Events {
    fn drop(&mut self) { for times in self.by_ref() { drop(times); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_batch_allows_requeue_without_losing_its_tail() {
        let queue = NodeEvents::default();
        let a = Arc::new(TouchedTimes::new(1));
        let b = Arc::new(TouchedTimes::new(2));
        queue.push(&a);
        queue.push(&b);
        queue.push(&a); // Coalesced while the old list owns its link.
        let mut old = queue.take();
        assert_eq!(old.next().unwrap().ino, 2);
        queue.push(&b); // Its link can now belong to the next batch.
        assert_eq!(old.next().unwrap().ino, 1);
        assert!(old.next().is_none());
        assert_eq!(queue.take().next().unwrap().ino, 2);
        assert_eq!((Arc::strong_count(&a), Arc::strong_count(&b)), (1, 1));
        queue.push(&a);
        drop(queue);
        assert_eq!(Arc::strong_count(&a), 1);
    }
}
