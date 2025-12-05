//! Memory-related system calls
//!
//! This module implements memory management syscalls including brk() for heap allocation.

use crate::syscall::{ErrorCode, SyscallResult};
use x86_64::instructions::tlb;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

/// Maximum heap size (64MB) - prevents runaway allocation
const MAX_HEAP_SIZE: u64 = 64 * 1024 * 1024;

/// Syscall 12: brk - change data segment size
///
/// This implements the traditional Unix brk() syscall which allows userspace
/// programs to expand or contract their heap region.
///
/// Arguments:
/// - addr: New program break address (0 = query current break)
///
/// Returns:
/// - Current program break on success
/// - Current program break on failure (cannot expand/contract as requested)
///
/// Behavior follows Linux semantics:
/// - brk(0) returns current program break without modification
/// - brk(addr) attempts to set program break to addr (page-aligned)
/// - Returns new break on success, old break on failure
pub fn sys_brk(addr: u64) -> SyscallResult {
    // Get current thread ID from per-CPU data (authoritative source)
    // per_cpu::current_thread() reads directly from GS segment, which is
    // set by the context switch code - this is what's actually running
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_brk: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Acquire the process manager lock
    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_brk: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let current_break = process.heap_end;
    let heap_start = process.heap_start;

    log::info!(
        "sys_brk: thread={} pid={:?} addr={:#x} heap_start={:#x} heap_end={:#x}",
        current_thread_id, pid, addr, heap_start, current_break
    );

    // If addr is 0, just return current program break
    if addr == 0 {
        return SyscallResult::Ok(current_break);
    }

    // Page-align the requested address up
    let new_break = (addr + 0xfff) & !0xfff;

    // Validate new break is not below heap start
    if new_break < heap_start {
        return SyscallResult::Ok(current_break);
    }

    // Validate new break doesn't exceed maximum heap size
    let heap_size = new_break - heap_start;
    if heap_size > MAX_HEAP_SIZE {
        return SyscallResult::Ok(current_break);
    }

    // Validate new break doesn't conflict with stack or other regions
    if new_break > crate::memory::layout::USERSPACE_CODE_DATA_END {
        return SyscallResult::Ok(current_break);
    }

    // Handle heap expansion
    if new_break > current_break {
        log::info!(
            "sys_brk: EXPANDING from {:#x} to {:#x}",
            current_break, new_break
        );

        // Get the process page table
        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => {
                log::error!("sys_brk: No page table for process!");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // Calculate pages to allocate
        let start_addr = current_break;
        let end_addr = new_break;

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        log::info!(
            "sys_brk: Mapping pages from {:#x} to {:#x}",
            start_page.start_address().as_u64(),
            end_page.start_address().as_u64()
        );

        // Map new pages with user-accessible, writable permissions
        // Use manual loop - Page::range_inclusive can return empty iterator for single page
        let mut current_page = start_page;
        let mut pages_mapped = 0u32;
        loop {
            // Allocate a physical frame
            let frame = match crate::memory::frame_allocator::allocate_frame() {
                Some(f) => f,
                None => {
                    log::error!("sys_brk: OOM allocating frame for page {:#x}", current_page.start_address().as_u64());
                    return SyscallResult::Ok(current_break); // Return old break on OOM
                }
            };

            // Map the page
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE;

            if let Err(e) = page_table.map_page(current_page, frame, flags) {
                log::error!("sys_brk: map_page failed for {:#x}: {}", current_page.start_address().as_u64(), e);
                return SyscallResult::Ok(current_break); // Return old break on error
            }

            // Flush TLB for this page so CPU sees the new mapping
            tlb::flush(current_page.start_address());
            pages_mapped += 1;

            // Stop after mapping the end page
            if current_page >= end_page {
                break;
            }
            current_page += 1;
        }

        log::info!("sys_brk: Successfully mapped {} pages", pages_mapped);

        // Update the heap end
        process.heap_end = new_break;
        process.memory_usage.heap_size = (new_break - heap_start) as usize;

        SyscallResult::Ok(new_break)
    }
    // Handle heap contraction
    else if new_break < current_break {
        log::info!(
            "sys_brk: CONTRACTING from {:#x} to {:#x}",
            current_break, new_break
        );

        // Get the process page table
        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => {
                log::error!("sys_brk: No page table for process during contraction!");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // Calculate pages to unmap
        let start_addr = new_break;
        let end_addr = current_break;

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        log::info!(
            "sys_brk: Unmapping pages from {:#x} to {:#x}",
            start_page.start_address().as_u64(),
            end_page.start_address().as_u64()
        );

        // Unmap pages
        let mut pages_unmapped = 0u32;
        for page in Page::range_inclusive(start_page, end_page) {
            match page_table.unmap_page(page) {
                Ok(frame) => {
                    // Flush TLB for this page
                    tlb::flush(page.start_address());
                    // Free the physical frame
                    crate::memory::frame_allocator::deallocate_frame(frame);
                    pages_unmapped += 1;
                }
                Err(e) => {
                    log::warn!("sys_brk: unmap_page failed for {:#x}: {}", page.start_address().as_u64(), e);
                    // Continue trying to unmap other pages
                }
            }
        }

        log::info!("sys_brk: Successfully unmapped {} pages", pages_unmapped);

        // Update the heap end
        process.heap_end = new_break;
        process.memory_usage.heap_size = (new_break - heap_start) as usize;

        SyscallResult::Ok(new_break)
    }
    // No change requested
    else {
        SyscallResult::Ok(current_break)
    }
}
