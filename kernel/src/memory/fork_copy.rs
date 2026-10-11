//! The per-address-space lock a fork holds while it copies its parent's tables.
//!
//! Fork builds the child's page tables from the parent's without
//! PROCESS_MANAGER, so dispatch on every CPU and every other process carry on
//! while it copies. What the copy needs instead is that nothing else changes
//! the parent's tables until it is done. A fork claims the parent's address
//! space under PROCESS_MANAGER, copies with the lock released, and gives the
//! claim up under the lock again.
//!
//! Every change to a process's page tables is made under PROCESS_MANAGER, so a
//! holder sees a claim in force for the whole of its section or for none of
//! it. A section that finds the address space it would change claimed leaves
//! it alone: a fault returns and is taken again, a syscall releases the lock
//! and waits (`process::manager_for_address_space`), and a signal delivery
//! leaves the signal pending. The copy runs with preemption disabled, as every
//! syscall body does, so the CPU making it never switches away and a wait ends
//! when the copy does.
//!
//! Only the row that owns an address space can fork it, and that row's thread
//! is the one copying, inside its syscall. A CLONE_VM thread's row does not
//! own its table, so the paths that can reach a claimed table are the ones that
//! find a table by its root, by a thread's shared address space, or by another
//! process's binding. `ProcessPageTable`'s mutating methods refuse a claimed
//! table outright, so a path that reaches one without checking fails at once
//! rather than changing a table the copy is reading.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::task::scheduler::MAX_CPUS;

/// The root of the address space each CPU is copying for a fork, or 0.
static CLAIMS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// How many entries of `CLAIMS` are set, so a check with no copy anywhere
/// reads one word.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Whether a fork is copying the address space whose root is `root`.
///
/// Authoritative under PROCESS_MANAGER, which a claim is taken and given up
/// under; without the lock it is a hint that may already be stale.
pub(crate) fn copying(root: u64) -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
        && root != 0
        && CLAIMS.iter().any(|claim| claim.load(Ordering::Acquire) == root)
}

/// Wait until no fork is copying the address space rooted at `root`. The CPU
/// making that copy cannot switch away and takes no lock a waiter can hold, so
/// the wait ends when the copy does; that CPU never waits for its own copy.
pub(crate) fn wait(root: u64) {
    let cpu = crate::task::scheduler::current_cpu();
    debug_assert!(cpu >= MAX_CPUS || CLAIMS[cpu].load(Ordering::Relaxed) != root);
    while copying(root) {
        core::hint::spin_loop();
    }
}

/// A fork's claim on its parent's address space, from its taking under
/// PROCESS_MANAGER to its release under it.
pub(crate) struct CopyClaim {
    cpu: usize,
}

impl CopyClaim {
    /// Claim the address space rooted at `root` for a copy made on this CPU.
    /// PROCESS_MANAGER held; the caller copies with preemption disabled and
    /// releases the claim before it can switch away.
    pub(crate) fn take(root: u64) -> Self {
        let cpu = crate::task::scheduler::current_cpu();
        assert!(cpu < MAX_CPUS, "fork copy on CPU {} past the claim table", cpu);
        let previous = CLAIMS[cpu].swap(root, Ordering::AcqRel);
        assert_eq!(previous, 0, "a CPU makes one fork copy at a time");
        ACTIVE.fetch_add(1, Ordering::AcqRel);
        Self { cpu }
    }
}

impl Drop for CopyClaim {
    /// Give the claim up. The fork drops it under PROCESS_MANAGER, so no
    /// holder sees the address space claimed for part of its section.
    fn drop(&mut self) {
        CLAIMS[self.cpu].store(0, Ordering::Release);
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
