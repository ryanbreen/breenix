//! Translation Lookaside Buffer (TLB) management
//!
//! This module provides safe wrappers around x86_64 TLB flush operations.
//! The TLB caches virtual-to-physical address translations, and must be
//! flushed when page table entries are modified to ensure the CPU sees
//! the updated mappings.

#[cfg(target_arch = "x86_64")]
use x86_64::instructions::tlb;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{tlb, VirtAddr};

/// Flush a single page from the TLB
///
/// This is more efficient than flushing the entire TLB when only
/// a single page mapping has changed.
#[allow(dead_code)]
#[inline]
pub fn flush_page(addr: VirtAddr) {
    tlb::flush(addr);
}

/// Flush the entire TLB
///
/// This forces the CPU to reload all address translations from the page tables.
/// Note: Writing to CR3 also flushes the entire TLB, but this function
/// provides an explicit way to do it.
#[inline]
pub fn flush_all() {
    tlb::flush_all();
}

/// Ensure TLB consistency after page table switch
///
/// This function should be called after switching page tables (writing to CR3)
/// to ensure all TLB entries are properly invalidated. While writing to CR3
/// flushes the TLB on x86_64, this provides an explicit guarantee and
/// documents the intent.
#[inline]
pub fn flush_after_page_table_switch() {
    // On x86_64, writing to CR3 flushes the entire TLB, but we can
    // explicitly flush to be absolutely certain and for documentation
    flush_all();
}
