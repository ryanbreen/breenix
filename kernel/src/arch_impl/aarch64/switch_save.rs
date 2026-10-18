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
//! A CPU that selects a marked thread waits for the mark to clear before it
//! runs the thread on that stack (`wait_until_saved`). The wait is bounded.
//! If the mark outlasts the bound, the selecting CPU puts the thread back on a
//! queue for a later selection, and the strand census reports a thread whose
//! mark stays set (`is_saving`).

use core::sync::atomic::{AtomicU64, Ordering};

use super::constants::MAX_CPUS;

/// Per CPU, the thread whose kernel stack that CPU is still leaving, or 0.
static SAVING: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// How long a selecting CPU waits for a mark to clear. The marked CPU has a
/// few hundred instructions left to run with interrupts masked, so only a
/// vCPU the host has stopped running takes longer.
const WAIT_BOUND_MS: u64 = 1;

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

/// Whether some CPU is still leaving `tid`'s kernel stack.
pub(crate) fn is_saving(tid: u64) -> bool {
    tid != 0 && SAVING.iter().any(|slot| slot.load(Ordering::Acquire) == tid)
}

/// Wait until no CPU is leaving `tid`'s kernel stack. Returns false if a CPU
/// still was when the bound ran out; the caller must not run the thread then.
pub(crate) fn wait_until_saved(tid: u64) -> bool {
    if !is_saving(tid) {
        return true;
    }
    let bound = super::timer::frequency_hz() / 1000 * WAIT_BOUND_MS;
    let start = super::timer::rdtsc();
    while is_saving(tid) {
        if super::timer::rdtsc().wrapping_sub(start) >= bound {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}
