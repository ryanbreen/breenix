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
    wake.node.close();
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
