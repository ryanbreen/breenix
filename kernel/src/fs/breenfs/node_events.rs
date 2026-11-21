//! Changed-inode notifications for deferred touches and final handle release.
use alloc::sync::Arc;
use core::{ptr, sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering}};
#[cfg(not(test))]
use crate::irq_safe_mutex::IrqSafeMutex as FrozenLock;
#[cfg(test)]
use spin::Mutex as FrozenLock;

#[derive(Clone, Copy, Default)]
pub(super) struct Stamps {
    pub access: u64,
    pub modify: u64,
}
impl Stamps {
    pub fn is_empty(self) -> bool { self.access == 0 && self.modify == 0 }
}

#[derive(Default)]
struct FrozenStamps {
    generation: u64,
    stamps: Stamps,
    inherited: bool,
}

// Deferred times are recorded per generation (E24 coordinator 601(2)): slot
// `generation & 1` belongs to the open generation, the other one to a frozen
// generation that its publication applies. A toucher that races the freeze
// lands in the frozen slot only if it read the old generation first; it then
// records into the new open slot as well (vfs.rs touch, x11c M4). Records and
// takes are SeqCst for that handshake.
pub(super) struct TouchedTimes {
    pub ino: u64,
    access: [AtomicU64; 2],
    modify: [AtomicU64; 2],
    frozen: FrozenLock<FrozenStamps>,
    queued: AtomicBool,
    next: AtomicPtr<TouchedTimes>,
}
impl TouchedTimes {
    pub fn new(ino: u64) -> Self {
        Self { ino, access: [const { AtomicU64::new(0) }; 2], modify: [const { AtomicU64::new(0) }; 2],
            frozen: FrozenLock::new(FrozenStamps::default()),
            queued: AtomicBool::new(false), next: AtomicPtr::new(ptr::null_mut()) }
    }
    pub fn pending(&self) -> bool {
        self.access.iter().chain(&self.modify).any(|slot| slot.load(Ordering::Acquire) != 0)
    }
    /// Record a coarse time (seconds + 1) for `generation`.
    pub fn record(&self, generation: u64, read: bool, stamp: u64) {
        let slots = if read { &self.access } else { &self.modify };
        slots[(generation & 1) as usize].store(stamp, Ordering::SeqCst);
    }
    /// Take `generation`'s stamps; 0 means no stamp.
    pub fn take(&self, generation: u64) -> Stamps {
        let slot = (generation & 1) as usize;
        Stamps {
            access: self.access[slot].swap(0, Ordering::SeqCst),
            modify: self.modify[slot].swap(0, Ordering::SeqCst),
        }
    }
    // The first consumer preserves the sample for both publication and the
    // scratch view. The lock covers a fixed set of scalar accesses; producers
    // still use only the atomic slots and retry any late record in the open slot.
    fn frozen(&self, generation: u64) -> impl core::ops::DerefMut<Target = FrozenStamps> + '_ {
        let mut saved = self.frozen.lock();
        if saved.generation != generation {
            *saved = FrozenStamps { generation, stamps: self.take(generation), inherited: false };
        }
        saved
    }
    pub fn for_publication(&self, generation: u64) -> Stamps {
        self.frozen(generation).stamps
    }
    pub fn inherit_frozen(&self, generation: u64) -> Stamps {
        let mut saved = self.frozen(generation);
        if core::mem::replace(&mut saved.inherited, true) {
            Stamps::default()
        } else {
            saved.stamps
        }
    }
    pub fn pending_frozen(&self, generation: u64) -> bool {
        let saved = self.frozen.lock();
        saved.generation == generation && !saved.inherited && !saved.stamps.is_empty()
    }
    /// Recover a stamp whose notification missed the previous freeze. Once
    /// sampled, any subsequent producer in that slot retries in the open slot.
    pub fn take_unsampled(&self, generation: u64) -> Stamps {
        let stamps = self.take(generation);
        if !stamps.is_empty() && self.frozen.lock().generation == generation {
            Stamps::default()
        } else {
            stamps
        }
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
// SAFETY: the detached list owns its Arc links and has one consumer. Moving
// that ownership between mount-lock holders does not share the iterator.
unsafe impl Send for Events {}
impl Default for Events {
    fn default() -> Self { Self(ptr::null_mut()) }
}
impl Events {
    pub fn is_empty(&self) -> bool { self.0.is_null() }
}
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
