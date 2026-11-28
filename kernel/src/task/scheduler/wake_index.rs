//! ARM64's IRQ-side lookup of scheduler threads. Only the live scheduler
//! writes the buckets; IRQ readers pin records across lookup and publication.

use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use alloc::sync::Arc;

use super::Thread;
use crate::task::{thread::ThreadWake, wake_inbox::WakeNode};

const BUCKETS: usize = 1024;
static HEADS: [AtomicPtr<ThreadWake>; BUCKETS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; BUCKETS];
static READERS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Pin(());

impl Pin {
    pub(super) fn new() -> Self {
        READERS.fetch_add(1, Ordering::SeqCst);
        Self(())
    }

    pub(super) fn lookup(&self, tid: u64) -> Option<(&WakeNode, usize)> {
        // SAFETY: this pin defers reclamation of any record lookup can reach.
        let wake = unsafe { find(tid).as_ref() }?;
        Some((&wake.node, wake.cpu.load(Ordering::Acquire)))
    }

    /// `tid`'s thread. The pin keeps it allocated: a thread is freed only
    /// once no pin is held (`unpinned`). Read only its atomics.
    pub(super) fn thread(&self, tid: u64) -> Option<&Thread> {
        // SAFETY: this pin defers reclamation of any record lookup can reach,
        // and of the thread that record names.
        let wake = unsafe { find(tid).as_ref() }?;
        // SAFETY: as above.
        unsafe { wake.thread.load(Ordering::Acquire).as_ref() }
    }

    pub(super) fn owner(&self, tid: u64) -> Option<usize> {
        self.lookup(tid).map(|(_, cpu)| cpu)
    }

    /// Timer membership is atomic; the pin keeps the record and thread alive
    /// even if an exit unlinks either while a group is being walked.
    pub(super) fn group_members(&self, group: usize) -> impl Iterator<Item = &ThreadWake> {
        HEADS.iter().flat_map(|head| {
            let mut next = head.load(Ordering::SeqCst);
            core::iter::from_fn(move || {
                // SAFETY: each link is reached under this reader pin.
                let wake = unsafe { next.as_ref() }?;
                next = wake.index_next.load(Ordering::SeqCst);
                Some(wake)
            })
        }).filter(move |wake| {
            wake.timer_group.load(Ordering::Acquire) == group
                && !wake.terminated.load(Ordering::Acquire)
        })
    }

}

impl Drop for Pin {
    fn drop(&mut self) {
        READERS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A scheduler writer has unlinked a thread. SeqCst pairs the links with
/// reader registration: a later reader cannot reach any unlinked record.
pub(super) fn unpinned() -> bool {
    READERS.load(Ordering::SeqCst) == 0
}

fn head(tid: u64) -> &'static AtomicPtr<ThreadWake> {
    &HEADS[tid as usize % BUCKETS]
}

/// Caller holds the scheduler lock, or a reader pin for lookup.
fn find(tid: u64) -> *mut ThreadWake {
    let mut wake = head(tid).load(Ordering::SeqCst);
    while !wake.is_null() {
        // SAFETY: the caller keeps records alive. The ID is immutable, the link atomic.
        unsafe {
            if (*wake).node.tid() == tid {
                break;
            }
            wake = (*wake).index_next.load(Ordering::SeqCst);
        }
    }
    wake
}

/// Only the live scheduler, under its lock, may insert a stable boxed thread.
pub(super) fn insert(thread: &mut Thread) {
    let tid = thread.id();
    let wake = Arc::new(ThreadWake::new(tid, ptr::from_mut(thread)));
    wake.timer_group.store(timer_group(thread), Ordering::Relaxed);
    wake.terminated.store(thread.state == super::ThreadState::Terminated, Ordering::Relaxed);
    let head = head(tid);
    wake.index_next
        .store(head.load(Ordering::Relaxed), Ordering::Relaxed);
    head.store(Arc::as_ptr(&wake).cast_mut(), Ordering::SeqCst);
    thread.wake = Some(wake);
}

/// Caller is retiring the thread under the live scheduler lock. Keep its
/// successor link intact for readers already walking this chain.
pub(super) fn remove(thread: &Thread) {
    let wake = thread.wake.as_ref().expect("admitted thread");
    super::rq::release_held_place(wake.node.close());
    let mut link = head(thread.id());
    loop {
        let entry = link.load(Ordering::Relaxed);
        assert!(!entry.is_null(), "scheduler thread missing from wake index");
        // SAFETY: only this scheduler writer can unlink entries.
        let next = unsafe { &(*entry).index_next };
        if ptr::eq(entry, Arc::as_ptr(wake)) {
            link.store(next.load(Ordering::Relaxed), Ordering::SeqCst);
            return;
        }
        link = next;
    }
}

/// # Safety
/// Caller holds the live scheduler lock and controls the lifetime and
/// aliasing of the result, just like its boxed thread list.
pub(super) unsafe fn thread_mut<'a>(tid: u64) -> Option<&'a mut Thread> {
    let wake = unsafe { find(tid).as_ref() }?;
    unsafe { wake.thread.load(Ordering::Relaxed).as_mut() }
}

/// # Safety
/// Caller holds the live scheduler lock for the lifetime of the result.
pub(super) unsafe fn thread_ref<'a>(tid: u64) -> Option<&'a Thread> {
    let wake = unsafe { find(tid).as_ref() }?;
    unsafe { wake.thread.load(Ordering::Relaxed).as_ref() }
}

/// The caller holds the new and old owners' run-queue locks (or every lock).
pub(super) fn set_owner(tid: u64, cpu: usize) {
    // Unlinking an ARM64 thread takes every run-queue lock.
    if let Some(wake) = unsafe { find(tid).as_ref() } {
        wake.cpu.store(cpu, Ordering::Release);
    }
}

/// The caller holds the CPU's run-queue lock and controls the result's aliasing.
pub(super) unsafe fn owned_thread<'a>(tid: u64, cpu: usize) -> Option<&'a mut Thread> {
    // Unlinking takes every run-queue lock and moving a thread takes its
    // owner's lock. The owner must still match after acquiring the lock.
    let wake = unsafe { find(tid).as_ref() }?;
    if wake.cpu.load(Ordering::Acquire) != cpu {
        return None;
    }
    unsafe { wake.thread.load(Ordering::Relaxed).as_mut() }
}

/// The caller holds the CPU's run-queue lock for the duration of the borrow.
pub(super) unsafe fn owned_thread_ref<'a>(tid: u64, cpu: usize) -> Option<&'a Thread> {
    let wake = unsafe { find(tid).as_ref() }?;
    if wake.cpu.load(Ordering::Acquire) != cpu {
        return None;
    }
    unsafe { wake.thread.load(Ordering::Relaxed).as_ref() }
}


pub(super) fn timer_group(thread: &Thread) -> usize {
    thread.signal_timers.as_ref().map_or(0, |group| Arc::as_ptr(group) as usize)
}

pub(super) fn publish_group(thread: &Thread) {
    if let Some(wake) = &thread.wake {
        wake.timer_group.store(timer_group(thread), Ordering::Release);
    }
}
