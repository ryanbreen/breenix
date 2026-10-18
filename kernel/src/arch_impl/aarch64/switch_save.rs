//! The mark that an outgoing thread's context is still being saved (#1785).
//!
//! An exception-return switch queues the thread it switches away from while
//! the scheduler lock is still held, before its CPU has left the thread's
//! kernel stack: the return path stands on that stack until it moves to the
//! frame staged on the CPU's scheduler stack. The switching CPU marks the
//! thread here first, and clears the mark with one release store once SP has
//! left the stack. The return path takes no lock and touches no device to do
//! it (strict-gate ruling S6).
//!
//! Selection skips a marked thread and leaves it queued (ruling S8): the
//! selecting CPU picks another thread or idles, and a later selection, at the
//! latest the next tick, finds the thread with its mark clear. No CPU waits
//! for a mark. The strand census reports a thread whose mark stays set
//! (`is_saving`).

use core::sync::atomic::{AtomicU64, Ordering};

use super::constants::MAX_CPUS;

/// Per CPU, the thread whose kernel stack that CPU is still leaving, or 0.
static SAVING: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Selections that skipped a thread because its mark was set.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Mark `tid` as still standing on `cpu`'s stack. Called by `cpu` itself,
/// under the scheduler lock, before it queues the thread.
pub(crate) fn mark(cpu: usize, tid: u64) {
    if cpu < MAX_CPUS {
        SAVING[cpu].store(tid, Ordering::Release);
    }
}

/// Clear `cpu`'s mark once it no longer stands on the thread's kernel stack.
/// The release orders this CPU's earlier accesses to that stack before it.
pub(crate) fn clear(cpu: usize) {
    if cpu < MAX_CPUS {
        SAVING[cpu].store(0, Ordering::Release);
    }
}

/// The CPU that is still leaving `tid`'s kernel stack, if any.
pub(crate) fn saving_cpu(tid: u64) -> Option<usize> {
    if tid == 0 {
        return None;
    }
    SAVING
        .iter()
        .position(|slot| slot.load(Ordering::Acquire) == tid)
}

/// Whether some CPU is still leaving `tid`'s kernel stack.
pub(crate) fn is_saving(tid: u64) -> bool {
    saving_cpu(tid).is_some()
}

/// Whether selection must skip `tid` for now, counting the skip.
pub(crate) fn skip_selection(tid: u64) -> bool {
    let saving = is_saving(tid);
    if saving {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
    }
    saving
}

/// Selections that skipped a marked thread so far.
#[cfg(feature = "boot_tests")]
pub(crate) fn skipped() -> u64 {
    SKIPPED.load(Ordering::Relaxed)
}
