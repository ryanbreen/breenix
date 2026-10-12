//! The per-address-space lock a fork holds while it copies its parent's tables.
//!
//! Fork builds the child's page tables from the parent's without
//! PROCESS_MANAGER, so dispatch on every CPU and every other process carry on
//! while it copies. What the copy needs instead is that nothing else changes
//! the parent's tables until it is done. A fork claims the parent's address
//! space under PROCESS_MANAGER, copies with the lock released, and gives the
//! claim up under the lock again.
//!
//! Every change to a process's page tables is made under PROCESS_MANAGER, and
//! a claim is taken under it, so a holder never sees a claim appear partway
//! through its section. A section that finds the address space it would
//! change claimed leaves it alone: a fault returns and is taken again, a
//! syscall releases the lock and waits (`process::manager_for_address_space`),
//! and a signal delivery leaves the signal pending. A software reader of a
//! claimed table waits for the copy before it reads (`ProcessPageTable`'s
//! readers), since the copy lowers descriptors in place.
//!
//! The copy runs with preemption disabled, as every syscall body does, and
//! the copying CPU gives the claim up as soon as the copy and its flush are
//! done, before it takes PROCESS_MANAGER again (`CopyClaim::end_copy`). So it
//! never needs a lock to end a wait, and a wait under PROCESS_MANAGER, such as
//! a copy-on-write fault taken inside a holder's section, ends when the copy
//! does. A claim seen to vanish partway through a section only means the
//! section held back for nothing.
//!
//! The child's table stays unpublished until the fork takes the lock again.
//! A file revocation that finds no row for a binding of that table waits,
//! with its locks released, until the fork publishes or abandons the child
//! (`forming`), so it never passes over a child that is about to appear.
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

/// The address space (`ProcessPageTable::address_space`) of the child each
/// CPU's fork is building, from the claim to the child's publication or
/// abandonment, or 0.
static FORMING: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// How many entries of `CLAIMS` are set, so a check with no copy anywhere
/// reads one word.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Whether a fork is copying the address space whose root is `root`.
///
/// Under PROCESS_MANAGER a claim cannot appear, and one that is there stays
/// until the copy is done; without the lock it is a hint that may already be
/// stale.
pub(crate) fn copying(root: u64) -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
        && root != 0
        && CLAIMS
            .iter()
            .any(|claim| claim.load(Ordering::Acquire) == root)
}

/// Wait until no fork is copying the address space rooted at `root`. The CPU
/// making that copy cannot switch away and gives the claim up without taking
/// any lock, so the wait ends when the copy does, whatever the waiter holds;
/// that CPU never waits for its own copy.
pub(crate) fn wait(root: u64) {
    if !copying(root) {
        return;
    }
    let cpu = crate::task::scheduler::current_cpu();
    assert!(
        cpu >= MAX_CPUS || CLAIMS[cpu].load(Ordering::Relaxed) != root,
        "a fork waited for its own copy"
    );
    while copying(root) {
        core::hint::spin_loop();
    }
}

/// Whether a fork is building the unpublished child table whose address
/// space is `space`: from its claim until it publishes or abandons the child,
/// under PROCESS_MANAGER. A caller holding the lock must release it before
/// waiting for this to clear.
pub(crate) fn forming(space: u64) -> bool {
    space != 0
        && FORMING
            .iter()
            .any(|forming| forming.load(Ordering::Acquire) == space)
}

/// Under PM, no unpublished fork table can escape a global shm revocation.
/// The caller releases PM and retries while this is true, allowing a child
/// whose pages were copied outside PM to publish before the revocation scans.
pub(crate) fn any_forming() -> bool {
    FORMING
        .iter()
        .any(|space| space.load(Ordering::Acquire) != 0)
}

/// A fork's claim on its parent's address space, from its taking under
/// PROCESS_MANAGER to the end of the copy, and on its child's table until the
/// child is published or abandoned under the lock again.
pub(crate) struct CopyClaim {
    cpu: usize,
    copying: bool,
}

impl CopyClaim {
    /// Claim the address space rooted at `root` for a copy made on this CPU
    /// into the child table whose address space is `child`. PROCESS_MANAGER
    /// held; the caller copies with preemption disabled and ends the copy
    /// before it can switch away.
    pub(crate) fn take(root: u64, child: u64) -> Self {
        let cpu = crate::task::scheduler::current_cpu();
        assert!(
            cpu < MAX_CPUS,
            "fork copy on CPU {} past the claim table",
            cpu
        );
        let previous = CLAIMS[cpu].swap(root, Ordering::AcqRel);
        assert_eq!(previous, 0, "a CPU makes one fork copy at a time");
        FORMING[cpu].store(child, Ordering::Release);
        ACTIVE.fetch_add(1, Ordering::AcqRel);
        Self { cpu, copying: true }
    }

    /// The copy is done and its flush has reached every CPU: give the
    /// parent's address space up. The copying CPU calls this before it takes
    /// PROCESS_MANAGER again, so nothing waiting for the copy, under the lock
    /// or not, waits for that CPU to get the lock.
    pub(crate) fn end_copy(&mut self) {
        if core::mem::replace(&mut self.copying, false) {
            CLAIMS[self.cpu].store(0, Ordering::Release);
            ACTIVE.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for CopyClaim {
    /// End the copy if it has not ended, and give the child's table up. The
    /// fork drops this under PROCESS_MANAGER, in the section that publishes
    /// or abandons the child.
    fn drop(&mut self) {
        self.end_copy();
        FORMING[self.cpu].store(0, Ordering::Release);
    }
}
