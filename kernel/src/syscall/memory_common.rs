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

/// Map `frames` at consecutive pages from `start` into the page table of the
/// process whose main thread is `thread_id`, holding PROCESS_MANAGER for each
/// `MAP_CHUNK_PAGES` pages. The leaf frames are allocated and zeroed before
/// this call, with no lock held; under the lock `map_page()` still reserves
/// leaf-record storage and allocates any missing intermediate table frames,
/// and the last section's VMA push can grow `vmas`. Each section re-checks
/// that the process still owns the table whose root is `root` and that no VMA
/// overlaps the range; the last one pushes `vma`.
///
/// New descriptors replace invalid ones, so no TLB entry needs flushing. On
/// failure every frame left unmapped is freed and the pages this call mapped
/// are unmapped by `unmap_prepared_prefix`, in sections of the same size. A
/// table that stopped being the process's keeps the pages it got in its own
/// leaf custody, which its retirement releases.
pub fn map_prepared_frames(
    thread_id: u64,
    root: u64,
    start: u64,
    frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    page_flags: PageTableFlags,
    vma: crate::memory::vma::Vma,
) -> Result<(), crate::syscall::ErrorCode> {
    use crate::syscall::ErrorCode;

    let end = start + (frames.len() as u64) * PAGE_SIZE;
    let free_from = |index: usize| {
        for frame in &frames[index..] {
            crate::memory::frame_allocator::deallocate_frame(*frame);
        }
    };
    let mut mapped = 0usize;
    loop {
        let mut manager_guard = crate::process::manager();
        let Some(process) = manager_guard
            .as_mut()
            .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
            .map(|(_, process)| process)
        else {
            free_from(mapped);
            return Err(ErrorCode::NoSuchProcess);
        };
        let overlaps = process
            .vmas
            .iter()
            .any(|existing| start < existing.end.as_u64() && end > existing.start.as_u64());
        let Some(page_table) = process
            .page_table
            .as_mut()
            .filter(|page_table| page_table.level_4_frame().start_address().as_u64() == root)
        else {
            free_from(mapped);
            return Err(ErrorCode::OutOfMemory);
        };

        let chunk_end = (mapped + MAP_CHUNK_PAGES).min(frames.len());
        let mut failed = overlaps;
        while !failed && mapped < chunk_end {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                start + (mapped as u64) * PAGE_SIZE,
            ));
            if page_table.map_page(page, frames[mapped], page_flags).is_ok() {
                mapped += 1;
            } else {
                failed = true;
            }
        }
        if failed {
            drop(manager_guard);
            free_from(mapped);
            unmap_prepared_prefix(thread_id, root, start, mapped);
            return Err(ErrorCode::OutOfMemory);
        }
        if mapped == frames.len() {
            process.vmas.push(vma);
            return Ok(());
        }
    }
}

/// Unmap the first `count` pages `map_prepared_frames` mapped from `start`,
/// holding PROCESS_MANAGER for each `MAP_CHUNK_PAGES` pages. Each page is
/// flushed before its frame is released. Stops once the process no longer
/// owns the table whose root is `root`; that table's retirement releases the
/// rest.
fn unmap_prepared_prefix(thread_id: u64, root: u64, start: u64, count: usize) {
    let mut unmapped = 0usize;
    while unmapped < count {
        let mut manager_guard = crate::process::manager();
        let Some(page_table) = manager_guard
            .as_mut()
            .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
            .and_then(|(_, process)| process.page_table.as_mut())
            .filter(|page_table| page_table.level_4_frame().start_address().as_u64() == root)
        else {
            return;
        };
        let chunk_end = (unmapped + MAP_CHUNK_PAGES).min(count);
        for index in unmapped..chunk_end {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                start + (index as u64) * PAGE_SIZE,
            ));
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
