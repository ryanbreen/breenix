//! Memory-related system calls
//!
//! This module implements memory management syscalls including brk() for heap allocation.
//!
//! This module is architecture-independent - it uses conditional imports to support
//! both x86_64 and ARM64.

use crate::syscall::{ErrorCode, SyscallResult};

// Conditional imports based on architecture
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};

// Import common memory syscall helpers
use crate::syscall::memory_common::{
    allocate_zeroed_frames, flush_new_mapping, get_current_thread_id, MAX_HEAP_SIZE, PAGE_SIZE,
};

extern crate alloc;

/// Syscall 12: brk - change data segment size
///
/// This implements the traditional Unix brk() syscall which allows userspace
/// programs to expand or contract their heap region.
///
/// Arguments:
/// - addr: New program break address (0 = query current break)
///
/// Returns:
/// - The new program break on success: exactly `addr`
/// - The current program break on failure (cannot expand/contract as requested)
///
/// Behavior follows Linux semantics:
/// - brk(0) returns current program break without modification
/// - brk(addr) sets the program break to exactly `addr` and maps the whole
///   pages behind it; the page rounding stays internal
/// - The pages the break grows over read as zero (SUSv2: "The newly-allocated
///   space is set to 0"), never what their frames held before
/// - Returns new break on success, old break on failure
pub fn sys_brk(addr: u64) -> SyscallResult {
    // Get current thread ID from per-CPU data (architecture-independent)
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_brk: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Growth needs zeroed frames. They are allocated and zeroed with no lock
    // held, as `sys_mmap` does, then the request is checked again under the
    // lock: frames prepared for a break another thread has since moved are
    // freed and prepared again. `prepared` holds the page they start at.
    let mut prepared: Option<(u64, alloc::vec::Vec<PhysFrame<Size4KiB>>)> = None;
    loop {
        let mut manager_guard = crate::process::manager();
        let Some((pid, process)) = manager_guard
            .as_mut()
            .and_then(|manager| manager.find_address_space_by_thread_mut(current_thread_id))
        else {
            drop(manager_guard);
            free_prepared(prepared);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        };

        let current_break = process.heap_end;
        let heap_start = process.heap_start;

        log::info!(
            "sys_brk: thread={} pid={:?} addr={:#x} heap_start={:#x} heap_end={:#x}",
            current_thread_id,
            pid,
            addr,
            heap_start,
            current_break
        );

        // If addr is 0, just return current program break. A request that
        // cannot be met leaves the break where it is and returns it.
        let refused = addr == 0
            || addr < heap_start
            || addr - heap_start > MAX_HEAP_SIZE
            || (addr > current_break
                && (process.data_bytes().saturating_add(addr - current_break)
                    > process.limits.get(crate::process::limits::DATA).soft
                    || process.mapped_bytes().saturating_add(addr - current_break)
                        > process.limits.get(crate::process::limits::AS).soft));
        // The pages behind the break end at its page-rounded value, which must
        // not reach the stack or other regions.
        let Some(new_top) = addr
            .checked_add(PAGE_SIZE - 1)
            .map(|end| end & !(PAGE_SIZE - 1))
            .filter(|&top| !refused && top <= crate::memory::layout::USERSPACE_CODE_DATA_END)
        else {
            drop(manager_guard);
            free_prepared(prepared);
            return SyscallResult::Ok(current_break);
        };
        let old_top = (current_break + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        // Under mlockall(MCL_FUTURE) the pages grown over are locked; removing
        // the locks on the pages shrunk over can split one interval.
        if (new_top > old_top
            && super::memory_advice::prepare_future(process, new_top - old_top).is_err())
            || (new_top < old_top && process.memory_locks.reserve_split().is_err())
        {
            drop(manager_guard);
            free_prepared(prepared);
            return SyscallResult::Ok(current_break);
        }

        let Some(page_table) = process.page_table.as_mut() else {
            log::error!("sys_brk: No page table for process!");
            drop(manager_guard);
            free_prepared(prepared);
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        };

        if new_top > old_top {
            let needed = ((new_top - old_top) / PAGE_SIZE) as usize;
            let frames = match prepared.take() {
                Some((from, frames)) if from == old_top && frames.len() == needed => frames,
                stale => {
                    drop(manager_guard);
                    free_prepared(stale);
                    match allocate_zeroed_frames(needed) {
                        Some(frames) => prepared = Some((old_top, frames)),
                        None => {
                            log::error!("sys_brk: OOM allocating {} frames", needed);
                            return SyscallResult::Ok(current_break);
                        }
                    }
                    continue;
                }
            };

            log::info!(
                "sys_brk: EXPANDING from {:#x} to {:#x}, mapping {:#x}..{:#x}",
                current_break,
                addr,
                old_top,
                new_top
            );

            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE;
            for (index, frame) in frames.iter().enumerate() {
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                    old_top + index as u64 * PAGE_SIZE,
                ));
                if let Err(e) = page_table.map_page(page, *frame, flags) {
                    log::error!(
                        "sys_brk: map_page failed for {:#x}: {}",
                        page.start_address().as_u64(),
                        e
                    );
                    // Give back the pages this call mapped, and the frames it
                    // did not, and leave the break where it was.
                    unmap_heap_pages(page_table, old_top, page.start_address().as_u64());
                    for frame in &frames[index..] {
                        crate::memory::frame_allocator::deallocate_frame(*frame);
                    }
                    return SyscallResult::Ok(current_break);
                }
                // The page was not present, so only this CPU's entry is dropped.
                flush_new_mapping(page.start_address());
            }
            super::memory_advice::record_future(process, old_top, new_top);
        } else if new_top < old_top {
            log::info!(
                "sys_brk: CONTRACTING from {:#x} to {:#x}, unmapping {:#x}..{:#x}",
                current_break,
                addr,
                new_top,
                old_top
            );
            unmap_heap_pages(page_table, new_top, old_top);
            process.memory_locks.remove(new_top, old_top);
        }

        process.heap_end = addr;
        process.memory_usage.heap_size = (addr - heap_start) as usize;
        drop(manager_guard);
        free_prepared(prepared);
        return SyscallResult::Ok(addr);
    }
}

/// Frees frames `sys_brk` prepared and did not map.
fn free_prepared(prepared: Option<(u64, alloc::vec::Vec<PhysFrame<Size4KiB>>)>) {
    for frame in prepared.into_iter().flat_map(|(_, frames)| frames) {
        crate::memory::frame_allocator::deallocate_frame(frame);
    }
}

/// Unmaps the heap pages in `[start, end)`, flushing each before its frame is
/// released.
fn unmap_heap_pages(
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
    start: u64,
    end: u64,
) {
    let mut page_start = start;
    while page_start < end {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_start));
        match page_table.unmap_page_deferred(page) {
            Ok(leaf) => leaf.flush().release(),
            Err(e) => log::warn!("sys_brk: unmap_page failed for {:#x}: {}", page_start, e),
        }
        page_start += PAGE_SIZE;
    }
}
