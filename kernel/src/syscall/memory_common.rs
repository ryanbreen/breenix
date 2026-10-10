//! Architecture-independent memory syscall helpers
//!
//! This module provides common utilities used by both x86_64 and ARM64
//! memory syscall implementations (brk, mmap, mprotect, munmap).
//!
//! The architecture-specific syscall implementations import these helpers
//! and provide arch-specific TLB flush operations.

// Conditional imports based on architecture
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};

use crate::memory::vma::Protection;

/// Page size constant (4 KiB)
pub const PAGE_SIZE: u64 = 4096;

/// Maximum heap size (64MB) - prevents runaway allocation
pub const MAX_HEAP_SIZE: u64 = 64 * 1024 * 1024;

/// Round up to page size
#[inline]
pub fn round_up_to_page(size: u64) -> u64 {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Round down to page size
#[inline]
pub fn round_down_to_page(addr: u64) -> u64 {
    addr & !(PAGE_SIZE - 1)
}

/// Check if address is page-aligned
#[inline]
pub fn is_page_aligned(addr: u64) -> bool {
    (addr & (PAGE_SIZE - 1)) == 0
}

/// Convert protection flags to page table flags
pub fn prot_to_page_flags(prot: Protection) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if prot.contains(Protection::WRITE) {
        flags |= PageTableFlags::WRITABLE;
    }
    // Note: x86_64 doesn't have a built-in execute-disable bit in basic paging
    // NX (No-Execute) requires enabling NXE bit in EFER MSR, which we can add later
    flags
}

/// Get the current thread ID from per-CPU data (architecture-independent)
///
/// Returns None if no current thread is set.
#[cfg(target_arch = "x86_64")]
pub fn get_current_thread_id() -> Option<u64> {
    crate::per_cpu::current_thread().map(|thread| thread.id)
}

#[cfg(target_arch = "aarch64")]
pub fn get_current_thread_id() -> Option<u64> {
    crate::per_cpu_aarch64::current_thread().map(|thread| thread.id)
}

/// Flush TLB for a single page on every CPU that may cache it
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn flush_tlb(addr: VirtAddr) {
    crate::memory::tlb::flush_page(addr);
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub fn flush_tlb(addr: VirtAddr) {
    crate::memory::arch_stub::tlb::flush(addr);
}

/// Flush after mapping a page whose entry was not present. x86 caches no
/// translation for a non-present page, so no other CPU has one to drop and
/// only this CPU's entry is invalidated.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn flush_new_mapping(addr: VirtAddr) {
    x86_64::instructions::tlb::flush(addr);
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub fn flush_new_mapping(addr: VirtAddr) {
    flush_tlb(addr);
}

/// Helper function to clean up mapped pages on mmap failure
///
/// This is used when a multi-page mapping fails partway through.
/// It unmaps and frees any pages that were successfully mapped before the failure.
pub fn cleanup_mapped_pages(
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
    mapped_pages: &[(Page<Size4KiB>, PhysFrame<Size4KiB>)],
) {
    log::warn!(
        "cleanup_mapped_pages: cleaning up {} already-mapped pages due to failure",
        mapped_pages.len()
    );

    for (page, _) in mapped_pages.iter() {
        match page_table.unmap_page_deferred(*page) {
            Ok(leaf) => {
                leaf.flush().release();
            }
            Err(e) => {
                log::error!(
                    "cleanup_mapped_pages: failed to unmap page {:#x}: {}",
                    page.start_address().as_u64(),
                    e
                );
            }
        }
    }
}

/// The highest page-aligned range of `length` bytes below `process`'s mmap
/// hint that no VMA overlaps, which the hint then moves down to. None once the
/// range would fall below `MMAP_REGION_START`, the floor
/// `is_valid_user_range`'s mmap arm polices (#742). Mappings placed at a hint
/// or with MAP_FIXED can sit below the hint, so the descent steps under each
/// one it meets. PROCESS_MANAGER held.
pub fn place_below_hint(process: &mut crate::process::Process, length: u64) -> Option<u64> {
    let mut top = process.mmap_hint;
    loop {
        let start = round_down_to_page(top.checked_sub(length)?);
        if start < crate::memory::vma::MMAP_REGION_START {
            return None;
        }
        let end = start + length;
        match process
            .vmas
            .iter()
            .filter(|vma| vma.start.as_u64() < end && start < vma.end.as_u64())
            .map(|vma| vma.start.as_u64())
            .min()
        {
            // Strictly below `top`, since the overlap starts before `end`.
            Some(lowest) => top = lowest,
            None => {
                process.mmap_hint = start;
                return Some(start);
            }
        }
    }
}

/// Pages mapped per PROCESS_MANAGER section by `map_prepared_frames`.
const MAP_CHUNK_PAGES: usize = 64;

/// Allocate and zero `count` frames with no lock held, for
/// `map_prepared_frames`. On failure every frame already allocated is freed.
pub fn allocate_zeroed_frames(count: usize) -> Option<alloc::vec::Vec<PhysFrame<Size4KiB>>> {
    let mut frames = alloc::vec::Vec::new();
    if frames.try_reserve_exact(count).is_err() {
        return None;
    }
    let physical_memory_offset = crate::memory::physical_memory_offset().as_u64();
    for _ in 0..count {
        let Some(frame) = crate::memory::frame_allocator::allocate_frame() else {
            for frame in frames {
                crate::memory::frame_allocator::deallocate_frame(frame);
            }
            return None;
        };
        unsafe {
            core::ptr::write_bytes(
                (physical_memory_offset + frame.start_address().as_u64()) as *mut u8,
                0,
                PAGE_SIZE as usize,
            );
        }
        frames.push(frame);
    }
    // Publish initialized contents before any caller installs a user PTE.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
    }
    Some(frames)
}

/// What `map_prepared_frames` installed.
pub struct PreparedMapping {
    /// Every frame went in: no other thread unmapped or filled part of the
    /// range first.
    pub complete: bool,
    /// The process is under mlockall(MCL_FUTURE) without MCL_ONFAULT, so the
    /// caller populates the range.
    pub populate: bool,
}

/// Source of `Vma::reservation` tokens; 0 means no reservation.
static NEXT_RESERVATION: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Map `frames` at consecutive pages from `start` into the page table of the
/// process whose main thread is `thread_id`, holding PROCESS_MANAGER for each
/// `MAP_CHUNK_PAGES` pages. The leaf frames are allocated and zeroed before
/// this call, with no lock held; under the lock `map_page()` still reserves
/// leaf-record storage and allocates any missing intermediate table frames.
///
/// The first section makes the range ready (`clear_for_mapping`: the limit
/// checks against the live mappings, MAP_FIXED's replacement of what is there,
/// MAP_FIXED_NOREPLACE's EEXIST, room for an mlockall(MCL_FUTURE) lock) and
/// publishes `vma` whole, carrying a reservation token, and its lock, so no
/// other thread sees the range empty and no other mapping can claim any of
/// it. Until the last section clears the token, faults and kernel copies into
/// a page not yet installed retry. Each section re-checks that the process still owns the table whose root is `root`, and
/// installs a frame only where a VMA carrying the token still covers its page,
/// with that VMA's protection, and only where no page is mapped yet: another
/// thread may unmap, mprotect or populate part of the range meanwhile, and the
/// result is as if it had done so after this call. The frames not installed
/// are freed.
///
/// New descriptors replace invalid ones, so no TLB entry needs flushing. If
/// `map_page` fails, every frame not installed is freed and
/// `unmap_reservation` removes the VMAs that still carry the token, with
/// their pages and locks, and nothing else. A table that stopped being the
/// process's keeps the pages it got in its own leaf custody, which its
/// retirement releases.
pub fn map_prepared_frames(
    thread_id: u64,
    root: u64,
    start: u64,
    frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    page_flags: PageTableFlags,
    mut vma: crate::memory::vma::Vma,
) -> Result<PreparedMapping, u64> {
    use crate::syscall::ErrorCode;

    let end = start + (frames.len() as u64) * PAGE_SIZE;
    let free_from = |index: usize| {
        for frame in &frames[index..] {
            crate::memory::frame_allocator::deallocate_frame(*frame);
        }
    };
    let token = NEXT_RESERVATION.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let prot = vma.prot;
    vma.reservation = token;
    let mut unpublished = Some(vma);
    let mut populate = false;
    let mut complete = true;
    let mut next = 0usize;
    loop {
        let mut manager_guard = crate::process::manager();
        let Some(process) = manager_guard
            .as_mut()
            .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
            .map(|(_, process)| process)
        else {
            free_from(next);
            return Err(ErrorCode::NoSuchProcess as u64);
        };
        if process
            .page_table
            .as_ref()
            .map(|page_table| page_table.level_4_frame().start_address().as_u64())
            != Some(root)
        {
            free_from(next);
            return Err(ErrorCode::OutOfMemory as u64);
        }
        if let Some(vma) = unpublished.take() {
            if let Err(errno) = crate::syscall::mmap::clear_for_mapping(
                process, start, end, vma.prot, vma.flags,
            ) {
                free_from(0);
                return Err(errno);
            }
            // clear_for_mapping reserved room for the push and the lock.
            process.vmas.push(vma);
            crate::syscall::memory_advice::record_future(process, start, end);
            populate = process.memory_locks.future && !process.memory_locks.onfault;
        }
        let crate::process::Process {
            vmas, page_table, ..
        } = process;
        let Some(page_table) = page_table.as_mut() else {
            free_from(next);
            return Err(ErrorCode::OutOfMemory as u64);
        };

        let chunk_end = (next + MAP_CHUNK_PAGES).min(frames.len());
        let mut failed = false;
        while next < chunk_end {
            let address = start + (next as u64) * PAGE_SIZE;
            let Some(owner) = vmas.iter().find(|v| {
                v.reservation == token && v.start.as_u64() <= address && address < v.end.as_u64()
            }) else {
                crate::memory::frame_allocator::deallocate_frame(frames[next]);
                complete = false;
                next += 1;
                continue;
            };
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
            if page_table.translate(page.start_address()).is_some() {
                crate::memory::frame_allocator::deallocate_frame(frames[next]);
                complete = false;
                next += 1;
                continue;
            }
            let flags = if owner.prot == prot {
                page_flags
            } else {
                crate::memory::anon_map::page_flags(owner.prot)
            };
            if page_table.map_page(page, frames[next], flags).is_err() {
                failed = true;
                break;
            }
            next += 1;
        }
        if failed {
            drop(manager_guard);
            free_from(next);
            unmap_reservation(thread_id, root, token, start, next);
            return Err(ErrorCode::OutOfMemory as u64);
        }
        if next == frames.len() {
            for vma in vmas.iter_mut().filter(|v| v.reservation == token) {
                vma.reservation = 0;
            }
            return Ok(PreparedMapping { complete, populate });
        }
    }
}

/// Undo a failed `map_prepared_frames`: unmap the pages among the first
/// `count` from `start` that a VMA carrying `token` still covers, holding
/// PROCESS_MANAGER for each `MAP_CHUNK_PAGES` pages, then remove those VMAs
/// and their locks. No page outside those VMAs is touched. Each page is flushed before its frame is released.
/// Stops once the process no longer owns the table whose root is `root`; that
/// table's retirement releases the rest.
fn unmap_reservation(thread_id: u64, root: u64, token: u64, start: u64, count: usize) {
    let mut unmapped = 0usize;
    loop {
        let mut manager_guard = crate::process::manager();
        let Some(process) = manager_guard
            .as_mut()
            .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
            .map(|(_, process)| process)
        else {
            return;
        };
        let crate::process::Process {
            vmas,
            page_table,
            memory_locks,
            ..
        } = process;
        let Some(page_table) = page_table
            .as_mut()
            .filter(|page_table| page_table.level_4_frame().start_address().as_u64() == root)
        else {
            return;
        };
        if unmapped == count {
            // Removing a lock can split one interval; without room for that,
            // the lock is left.
            if memory_locks.reserve_split().is_ok() {
                for vma in vmas.iter().filter(|v| v.reservation == token) {
                    memory_locks.remove(vma.start.as_u64(), vma.end.as_u64());
                }
            }
            vmas.retain(|v| v.reservation != token);
            return;
        }
        let chunk_end = (unmapped + MAP_CHUNK_PAGES).min(count);
        for index in unmapped..chunk_end {
            let address = start + (index as u64) * PAGE_SIZE;
            if !vmas.iter().any(|v| {
                v.reservation == token && v.start.as_u64() <= address && address < v.end.as_u64()
            }) {
                continue;
            }
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
            if let Ok(leaf) = page_table.unmap_page_deferred(page) {
                leaf.flush().release();
            }
        }
        unmapped = chunk_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_alignment() {
        assert!(is_page_aligned(0));
        assert!(is_page_aligned(4096));
        assert!(is_page_aligned(8192));
        assert!(!is_page_aligned(1));
        assert!(!is_page_aligned(4097));
    }

    #[test]
    fn test_round_up_to_page() {
        assert_eq!(round_up_to_page(0), 0);
        assert_eq!(round_up_to_page(1), 4096);
        assert_eq!(round_up_to_page(4096), 4096);
        assert_eq!(round_up_to_page(4097), 8192);
    }

    #[test]
    fn test_round_down_to_page() {
        assert_eq!(round_down_to_page(0), 0);
        assert_eq!(round_down_to_page(4095), 0);
        assert_eq!(round_down_to_page(4096), 4096);
        assert_eq!(round_down_to_page(8191), 4096);
    }
}
