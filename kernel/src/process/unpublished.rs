//! Ownership of a process that is under construction and not yet published.
//!
//! A process builder allocates an address space before the fallible work that
//! fills it: the ELF load, the user stack allocation and mapping, the initial
//! TLS mapping, the argv/envp/auxv frame, and the main thread. Until the
//! builder inserts a row into the process table, nothing else can reach that
//! address space, so every error exit in between owns its release.
//! claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
//!
//! Two guards cover the two halves of that window, and between them there is no
//! fallible step:
//!
//! * Before a local Process exists, UnpublishedPageTable
//!   (kernel/src/memory/process_memory.rs) owns the boxed table. That is the
//!   same guard exec already uses, and EXEC_FAILED_RELEASE_ORACLE is the proof
//!   of its exactness.
//! claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
//! * Once the table has moved into a local Process, UnpublishedProcess owns the
//!   whole partially-built process and releases its table through the same
//!   never-installed path on drop.
//! claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
//!
//! UnpublishedProcess::commit is the only way to get the process back out, and
//! it disarms the release. A table that reaches a process-table row is
//! therefore never released here, and a table that does not reach one is never
//! dropped Undecided.
//! claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
//!
//! The release is release_mapped_leaves followed by retire_bounded, with no
//! cross-CPU liveness grace period: the address space was never installed in
//! TTBR0_EL1 or CR3, so no other CPU can be walking it.
//! claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
//!
//! What the release deliberately does not do is free the user stack frames.
//! allocate_stack_with_privilege registers user stack frames as external leaf
//! frames on both architectures (kernel/src/memory/stack.rs), so
//! release_mapped_leaves classifies them LeafMapping::External, drops the
//! mapping and returns no frame. The stack owner (GuardedStack) keeps whatever
//! custody it had; issue 583 still describes that leak, and this guard neither
//! repairs nor worsens it.
use super::process::Process;
use crate::memory::process_memory::UnpublishedPageTable;

/// Owns a Process whose address space has not been published in a row.
///
/// Dropping without commit releases the address space; commit hands the process
/// to the caller with its table intact.
pub(crate) struct UnpublishedProcess {
    process: Option<Process>,
    pid: u64,
}

impl UnpublishedProcess {
    pub(crate) fn new(process: Process, pid: u64) -> Self {
        Self {
            process: Some(process),
            pid,
        }
    }

    /// Borrow the process under construction. Callers bind this once and use
    /// the reference for the rest of the builder so that disjoint field borrows
    /// (page_table and stack at the same time) still typecheck.
    pub(crate) fn as_mut(&mut self) -> &mut Process {
        self.process.as_mut().expect("unpublished process")
    }

    /// Disarm the release and take the process. Called only where the builder
    /// goes on to insert the row.
    pub(crate) fn commit(mut self) -> Process {
        self.process.take().expect("unpublished process")
    }
}

impl Drop for UnpublishedProcess {
    fn drop(&mut self) {
        let Some(mut process) = self.process.take() else {
            return;
        };
        if let Some(page_table) = process.page_table.take() {
            // Same release the exec path already proves: mapped user leaves
            // first, then the table frames and the root, with no cross-CPU
            // liveness grace period. from_box keeps the table where it is
            // instead of unboxing and reallocating it.
            // claim-lint:ok: docs/planning/green-program/process/588-UNPUBLISHED-CONSTRUCTION-2026-09-07.md
            drop(UnpublishedPageTable::from_box(page_table, self.pid));
        }
    }
}

/// Gate leg for the unpublished-construction boundary: commit must hand the
/// live address space back untouched, and the same table must release exactly
/// what its construction took.
///
/// Returns the used-frame counts at three points -- before constructing the
/// table, after constructing it, after commit, and after releasing the
/// committed table through the unpublished path -- so the caller states the
/// exactness it requires rather than trusting a boolean.
#[cfg(feature = "boot_tests")]
pub(crate) fn commit_preserves_live_table_for_gate() -> Option<[usize; 4]> {
    use crate::memory::process_memory::ProcessPageTable;
    use crate::process::ProcessId;
    use alloc::boxed::Box;
    use alloc::string::String;

    const GATE_PID: u64 = u64::MAX - 5;

    let used_before = used_frames();
    let page_table = ProcessPageTable::new().ok()?;
    let used_constructed = used_frames();

    let pid = ProcessId::new(GATE_PID);
    let mut process = Process::new(pid, String::from("unpublished_commit_gate"), zero_addr());
    process.page_table = Some(Box::new(page_table));

    let guard = UnpublishedProcess::new(process, GATE_PID);
    let mut process = guard.commit();
    let used_committed = used_frames();

    let page_table = process.page_table.take()?;
    drop(UnpublishedPageTable::from_box(page_table, GATE_PID));
    let used_released = used_frames();

    Some([used_before, used_constructed, used_committed, used_released])
}

#[cfg(feature = "boot_tests")]
fn used_frames() -> usize {
    let stats = crate::memory::frame_allocator::memory_stats();
    stats
        .allocated_frames
        .saturating_sub(crate::memory::frame_allocator::free_list_len_for_gate())
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn zero_addr() -> x86_64::VirtAddr {
    x86_64::VirtAddr::new(0)
}

#[cfg(all(feature = "boot_tests", not(target_arch = "x86_64")))]
fn zero_addr() -> crate::memory::arch_stub::VirtAddr {
    crate::memory::arch_stub::VirtAddr::new(0)
}
