//! Intrusive, multi-producer wake inbox with one consumer per CPU.

use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

const PENDING: u64 = 1 << 63;
const CLOSED: u64 = 1 << 62;
const DRAIN_BUDGET: usize = 32;

/// A scheduler thread owns this node from admission through retirement.
pub(crate) struct WakeNode {
    tid: u64,
    state: AtomicU64,
    next: AtomicPtr<WakeNode>,
}

impl WakeNode {
    pub(crate) const fn new(tid: u64) -> Self {
        Self {
            tid,
            state: AtomicU64::new(0),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub(crate) fn tid(&self) -> u64 {
        self.tid
    }

    /// Forbid new publications and return any reasons that need cancellation.
    /// A linked node stays pending until its consumer unlinks it.
    pub(crate) fn close(&self) -> u64 {
        let previous = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & PENDING) | CLOSED)
            })
            .unwrap();
        previous & !(PENDING | CLOSED)
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn pending_bits(&self) -> u64 {
        self.state.load(Ordering::Acquire) & !(PENDING | CLOSED)
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & PENDING != 0
    }
}

pub(crate) struct WakeInbox {
    incoming: AtomicPtr<WakeNode>,
    /// The consumer finishes this batch before detaching newer arrivals.
    batch: AtomicPtr<WakeNode>,
    depth: AtomicUsize,
}

impl WakeInbox {
    pub(crate) const fn new() -> Self {
        Self {
            incoming: AtomicPtr::new(ptr::null_mut()),
            batch: AtomicPtr::new(ptr::null_mut()),
            depth: AtomicUsize::new(0),
        }
    }

    /// Merge the wake reasons and return the previous reasons. The first
    /// producer to set PENDING owns the link and must finish publishing it.
    /// Only a retired thread can decline publication.
    ///
    /// # Safety
    /// The caller pins the node through publication. The thread's retirement
    /// must also keep the node allocated while PENDING is set.
    pub(crate) unsafe fn publish(&self, node: &WakeNode, bits: u64, replace: u64) -> Option<u64> {
        let previous = node
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & CLOSED == 0).then_some((state & !replace) | PENDING | bits)
            })
            .ok()?;
        if previous & PENDING == 0 {
            self.depth.fetch_add(1, Ordering::Relaxed);
            let node_ptr = ptr::from_ref(node).cast_mut();
            let mut head = self.incoming.load(Ordering::Relaxed);
            loop {
                node.next.store(head, Ordering::Relaxed);
                match self.incoming.compare_exchange_weak(
                    head,
                    node_ptr,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(current) => head = current,
                }
            }
        }
        Some(previous & !PENDING)
    }

    /// Process a bounded batch and report whether another pass is needed.
    ///
    /// # Safety
    /// The scheduler serializes consumers with its lock for this inbox.
    pub(crate) unsafe fn drain(&self, mut apply: impl FnMut(u64, u64)) -> bool {
        let mut node = self.batch.load(Ordering::Relaxed);
        if node.is_null() {
            node = self.incoming.swap(ptr::null_mut(), Ordering::Acquire);
        }
        for _ in 0..DRAIN_BUDGET {
            if node.is_null() {
                break;
            }
            // SAFETY: PENDING keeps the linked node alive. Read its link and
            // identity before clearing PENDING, which permits reuse or free.
            let (next, tid, bits) = unsafe {
                let next = (*node).next.load(Ordering::Relaxed);
                let tid = (*node).tid;
                self.depth.fetch_sub(1, Ordering::Relaxed);
                let state = (*node).state.fetch_and(CLOSED, Ordering::AcqRel);
                (next, tid, state & !(PENDING | CLOSED))
            };
            node = next;
            if bits != 0 {
                apply(tid, bits);
            }
        }
        self.batch.store(node, Ordering::Release);
        !node.is_null() || !self.incoming.load(Ordering::Acquire).is_null()
    }

    pub(crate) fn has_work(&self) -> bool {
        !self.batch.load(Ordering::Acquire).is_null()
            || !self.incoming.load(Ordering::Acquire).is_null()
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}
