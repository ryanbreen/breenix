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

/// Flush TLB for a single page (architecture-specific implementation)
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn flush_tlb(addr: VirtAddr) {
    x86_64::instructions::tlb::flush(addr);
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

    for (page, frame) in mapped_pages.iter() {
        // Unmap the page
        match page_table.unmap_page(*page) {
            Ok(_) => {
                // Flush TLB
                flush_tlb(page.start_address());
                // Free the frame
                crate::memory::frame_allocator::deallocate_frame(*frame);
            }
            Err(e) => {
                log::error!(
                    "cleanup_mapped_pages: failed to unmap page {:#x}: {}",
                    page.start_address().as_u64(),
                    e
                );
                // Still try to free the frame
                crate::memory::frame_allocator::deallocate_frame(*frame);
            }
        }
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
