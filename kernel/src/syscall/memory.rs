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
    log::info!("sys_brk called with addr={:#x}", addr);

    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_brk: No current thread in scheduler");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Find the current process
    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            log::error!("sys_brk: Process manager not available");
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
    };

    let (pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_brk: Thread {} not found in any process",
                current_thread_id
            );
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let current_break = process.heap_end;
    let heap_start = process.heap_start;

    log::debug!(
        "sys_brk: PID {} heap_start={:#x}, current_break={:#x}, requested={:#x}",
        pid.as_u64(),
        heap_start,
        current_break,
        addr
    );

    // If addr is 0, just return current program break
    if addr == 0 {
        log::debug!("sys_brk: Query current break, returning {:#x}", current_break);
        return SyscallResult::Ok(current_break);
    }

    // Page-align the requested address up
    let new_break = (addr + 0xfff) & !0xfff;

    // Validate new break is not below heap start
    if new_break < heap_start {
        log::warn!(
            "sys_brk: Requested break {:#x} is below heap start {:#x}",
            new_break,
            heap_start
        );
        return SyscallResult::Ok(current_break);
    }

    // Validate new break doesn't exceed maximum heap size
    let heap_size = new_break - heap_start;
    if heap_size > MAX_HEAP_SIZE {
        log::warn!(
            "sys_brk: Requested heap size {} exceeds maximum {}",
            heap_size,
            MAX_HEAP_SIZE
        );
        return SyscallResult::Ok(current_break);
    }

    // Validate new break doesn't conflict with stack or other regions
    // The userspace code/data region ends at USERSPACE_CODE_DATA_END
    if new_break > crate::memory::layout::USERSPACE_CODE_DATA_END {
        log::warn!(
            "sys_brk: Requested break {:#x} exceeds code/data region end {:#x}",
            new_break,
            crate::memory::layout::USERSPACE_CODE_DATA_END
        );
        return SyscallResult::Ok(current_break);
    }

    // Handle heap expansion
    if new_break > current_break {
        log::info!(
            "sys_brk: Expanding heap from {:#x} to {:#x} ({} bytes)",
            current_break,
            new_break,
            new_break - current_break
        );

        // Get the process page table
        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => {
                log::error!("sys_brk: Process has no page table");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // Calculate pages to allocate (only map new pages, not already-mapped ones)
        let start_addr = current_break;
        let end_addr = new_break;

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        log::debug!(
            "sys_brk: Mapping pages from {:#x} to {:#x}",
            start_page.start_address().as_u64(),
            end_page.start_address().as_u64()
        );

        // Map new pages with user-accessible, writable permissions
        // NOTE: We use a manual loop instead of Page::range_inclusive because
        // range_inclusive returns an empty iterator when start_page == end_page
        // (single page allocation), which is a common case.
        let mut current_page = start_page;
        log::info!(
            "sys_brk: Starting page mapping loop from {:#x} to {:#x}",
            start_page.start_address().as_u64(),
            end_page.start_address().as_u64()
        );
        loop {
            log::info!(
                "sys_brk: Loop iteration - mapping page at {:#x}",
                current_page.start_address().as_u64()
            );
            // Allocate a physical frame
            let frame = match crate::memory::frame_allocator::allocate_frame() {
                Some(f) => f,
                None => {
                    log::error!("sys_brk: Out of memory allocating frame");
                    return SyscallResult::Ok(current_break); // Return old break on OOM
                }
            };

            // Map the page
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE;

            if let Err(e) = page_table.map_page(current_page, frame, flags) {
                log::error!(
                    "sys_brk: Failed to map page at {:#x}: {}",
                    current_page.start_address().as_u64(),
                    e
                );
                return SyscallResult::Ok(current_break); // Return old break on error
            }

            // Flush TLB for this page so CPU sees the new mapping
            tlb::flush(current_page.start_address());

            log::info!(
                "sys_brk: MAPPED page {:#x} to frame {:#x}",
                current_page.start_address().as_u64(),
                frame.start_address().as_u64()
            );

            // Stop after mapping the end page
            if current_page >= end_page {
                break;
            }
            current_page += 1;
        }

        // Update the heap end
        process.heap_end = new_break;
        process.memory_usage.heap_size = (new_break - heap_start) as usize;

        log::info!(
            "sys_brk: Successfully expanded heap to {:#x} (size={} bytes)",
            new_break,
            process.memory_usage.heap_size
        );

        SyscallResult::Ok(new_break)
    }
    // Handle heap contraction
    else if new_break < current_break {
        log::info!(
            "sys_brk: Contracting heap from {:#x} to {:#x} ({} bytes freed)",
            current_break,
            new_break,
            current_break - new_break
        );

        // Get the process page table
        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => {
                log::error!("sys_brk: Process has no page table");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // Calculate pages to unmap
        let start_addr = new_break;
        let end_addr = current_break;

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        log::debug!(
            "sys_brk: Unmapping pages from {:#x} to {:#x}",
            start_page.start_address().as_u64(),
            end_page.start_address().as_u64()
        );

        // Unmap pages
        for page in Page::range_inclusive(start_page, end_page) {
            match page_table.unmap_page(page) {
                Ok(_frame) => {
                    // Flush TLB for this page so CPU sees it's no longer mapped
                    tlb::flush(page.start_address());
                    log::trace!("sys_brk: Unmapped page {:#x}", page.start_address().as_u64());
                    // TODO: Free the physical frame back to the allocator
                }
                Err(e) => {
                    log::warn!(
                        "sys_brk: Failed to unmap page at {:#x}: {}",
                        page.start_address().as_u64(),
                        e
                    );
                    // Continue trying to unmap other pages
                }
            }
        }

        // Update the heap end
        process.heap_end = new_break;
        process.memory_usage.heap_size = (new_break - heap_start) as usize;

        log::info!(
            "sys_brk: Successfully contracted heap to {:#x} (size={} bytes)",
            new_break,
            process.memory_usage.heap_size
        );

        log::info!("sys_brk: Returning new_break={:#x} from contraction", new_break);
        SyscallResult::Ok(new_break)
    }
    // No change requested
    else {
        log::debug!("sys_brk: No change, break already at {:#x}", current_break);
        SyscallResult::Ok(current_break)
    }
}
