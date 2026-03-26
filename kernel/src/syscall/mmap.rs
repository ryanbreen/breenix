//! Memory mapping system calls (mmap, munmap)
//!
//! This module implements mmap() and munmap() for mapping and unmapping
//! memory regions in userspace process address spaces.
//!
//! This module is architecture-independent - it uses conditional imports to support
//! both x86_64 and ARM64.

use crate::memory::vma::{MmapFlags, Protection, Vma};
use crate::syscall::{ErrorCode, SyscallResult};

// Conditional imports based on architecture
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Page, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{Page, PhysFrame, Size4KiB, VirtAddr};

// Import common memory syscall helpers
use crate::syscall::memory_common::{
    cleanup_mapped_pages, flush_tlb, get_current_thread_id, is_page_aligned, prot_to_page_flags,
    round_down_to_page, round_up_to_page, PAGE_SIZE,
};

extern crate alloc;

/// Syscall 9: mmap - Map memory into process address space
///
/// Arguments:
/// - addr: Hint address (0 = kernel chooses, or specific addr with MAP_FIXED)
/// - length: Size of mapping (will be rounded up to page size)
/// - prot: Protection flags (PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4)
/// - flags: MAP_SHARED=1, MAP_PRIVATE=2, MAP_FIXED=0x10, MAP_ANONYMOUS=0x20
/// - fd: File descriptor (-1 for anonymous)
/// - offset: File offset (0 for anonymous)
///
/// Returns: Start address of mapping on success, or negative errno
pub fn sys_mmap(
    addr: u64,
    length: u64,
    prot: u32,
    flags: u32,
    fd: i64,
    offset: u64,
) -> SyscallResult {
    let prot = Protection::from_bits_truncate(prot);
    let flags = MmapFlags::from_bits_truncate(flags);

    log::info!(
        "sys_mmap: addr={:#x} length={:#x} prot={:?} flags={:?} fd={} offset={:#x}",
        addr,
        length,
        prot,
        flags,
        fd,
        offset
    );

    // Validate length
    if length == 0 {
        log::warn!("sys_mmap: length is 0");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Round length up to page size
    let length = round_up_to_page(length);

    // Require MAP_ANONYMOUS (file-backed not yet supported)
    if !flags.contains(MmapFlags::ANONYMOUS) {
        log::warn!("sys_mmap: file-backed mappings not yet supported");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Must specify exactly one of MAP_SHARED or MAP_PRIVATE
    let is_shared = flags.contains(MmapFlags::SHARED);
    let is_private = flags.contains(MmapFlags::PRIVATE);
    if !is_shared && !is_private {
        log::warn!("sys_mmap: must specify MAP_SHARED or MAP_PRIVATE");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // File descriptor should be -1 for anonymous mappings
    if fd != -1 {
        log::warn!("sys_mmap: fd must be -1 for MAP_ANONYMOUS");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Get current thread and process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_mmap: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Phase 1: Acquire lock to read process metadata and get a raw pointer to the
    // page table.  We release the lock BEFORE the page-mapping loop so that we do
    // not hold PROCESS_MANAGER (IRQs disabled) across hundreds of frame allocations
    // and TLB flushes — that caused system-wide priority inversion on SMP: other
    // CPUs spinning in manager() never made progress because the holder had IRQs
    // off the entire time.
    //
    // Safety: the raw pointer is valid for the lifetime of this syscall because
    // (a) the process cannot be freed while one of its threads is executing a syscall,
    // (b) no other CPU modifies this process's page table concurrently (user
    //     processes are single-threaded in the current scheduler model), and
    // (c) we re-acquire the lock in Phase 3 before touching process.vmas.
    let (start_addr, end_addr, page_flags, page_table_ptr) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => {
                log::error!(
                    "sys_mmap: No process found for thread_id={}",
                    current_thread_id
                );
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // Determine the start address
        let start_addr = if flags.contains(MmapFlags::FIXED) {
            // MAP_FIXED: use addr directly
            if !is_page_aligned(addr) {
                log::warn!("sys_mmap: MAP_FIXED requires page-aligned address");
                return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
            }
            addr
        } else {
            let hint = process.mmap_hint;
            let new_addr = round_down_to_page(hint.saturating_sub(length));
            if new_addr < 0x1000_0000 {
                log::error!("sys_mmap: out of mmap space");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
            process.mmap_hint = new_addr;
            new_addr
        };

        // Check for overflow when calculating end address
        let end_addr = match start_addr.checked_add(length) {
            Some(a) => a,
            None => {
                log::warn!("sys_mmap: start_addr + length would overflow");
                return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
            }
        };

        log::info!(
            "sys_mmap: allocating region {:#x}..{:#x}",
            start_addr,
            end_addr
        );

        // Check for overlaps with existing VMAs
        for vma in &process.vmas {
            let vma_start = vma.start.as_u64();
            let vma_end = vma.end.as_u64();
            if start_addr < vma_end && end_addr > vma_start {
                log::warn!(
                    "sys_mmap: region overlaps with existing VMA at {:#x}..{:#x}",
                    vma_start,
                    vma_end
                );
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        }

        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => {
                log::error!("sys_mmap: No page table for process!");
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        let page_flags = prot_to_page_flags(prot);
        // SAFETY: page_table lives inside a Box<ProcessPageTable> inside the process,
        // which remains valid for the duration of this syscall (see comment above).
        let page_table_ptr: *mut _ = &mut **page_table;
        (start_addr, end_addr, page_flags, page_table_ptr)
        // manager_guard drops here, releasing PROCESS_MANAGER before the loop
    };

    // Phase 2: Map pages WITHOUT holding PROCESS_MANAGER.
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));
    let physical_memory_offset = crate::memory::physical_memory_offset();
    let mut mapped_pages: alloc::vec::Vec<(Page<Size4KiB>, PhysFrame<Size4KiB>)> =
        alloc::vec::Vec::new();
    let mut current_page = start_page;

    loop {
        let frame = match crate::memory::frame_allocator::allocate_frame() {
            Some(f) => f,
            None => {
                log::error!(
                    "sys_mmap: OOM allocating frame for page {:#x}",
                    current_page.start_address().as_u64()
                );
                // SAFETY: same page_table_ptr lifetime argument as above.
                let page_table = unsafe { &mut *page_table_ptr };
                cleanup_mapped_pages(page_table, &mapped_pages);
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // SAFETY: see comment above — page_table is valid for this syscall's lifetime.
        let page_table = unsafe { &mut *page_table_ptr };
        if let Err(e) = page_table.map_page(current_page, frame, page_flags) {
            log::error!(
                "sys_mmap: map_page failed for {:#x}: {}",
                current_page.start_address().as_u64(),
                e
            );
            crate::memory::frame_allocator::deallocate_frame(frame);
            cleanup_mapped_pages(page_table, &mapped_pages);
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }

        let phys_addr = frame.start_address().as_u64();
        let virt_ptr = (physical_memory_offset.as_u64() + phys_addr) as *mut u8;
        unsafe {
            core::ptr::write_bytes(virt_ptr, 0, PAGE_SIZE as usize);
        }

        flush_tlb(current_page.start_address());
        mapped_pages.push((current_page, frame));

        if current_page >= end_page {
            break;
        }
        current_page += 1;
    }

    log::info!("sys_mmap: Successfully mapped {} pages", mapped_pages.len());

    // Phase 3: Re-acquire lock to register the VMA in the process.
    {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                let vma = Vma::new(
                    VirtAddr::new(start_addr),
                    VirtAddr::new(end_addr),
                    prot,
                    flags,
                );
                process.vmas.push(vma);
            }
        }
    }

    SyscallResult::Ok(start_addr)
}

/// Syscall 10: mprotect - Change protection of memory region
///
/// Arguments:
/// - addr: Start address (must be page-aligned)
/// - length: Size of region (will be rounded up to page size)
/// - prot: New protection flags (PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4)
///
/// Returns: 0 on success, negative errno on error
pub fn sys_mprotect(addr: u64, length: u64, prot: u32) -> SyscallResult {
    let new_prot = Protection::from_bits_truncate(prot);

    log::info!(
        "sys_mprotect: addr={:#x} length={:#x} prot={:?}",
        addr,
        length,
        new_prot
    );

    // Validate addr is page-aligned
    if !is_page_aligned(addr) {
        log::warn!("sys_mprotect: address not page-aligned");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Validate length
    if length == 0 {
        log::warn!("sys_mprotect: length is 0");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Round length up to page size
    let length = round_up_to_page(length);
    let end_addr = match addr.checked_add(length) {
        Some(a) => a,
        None => {
            log::warn!("sys_mprotect: addr + length would overflow");
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
    };

    // Get current thread and process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_mprotect: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_mprotect: No process found for thread_id={}",
                current_thread_id
            );
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Find the VMA that contains this address range
    // For simplicity, require exact match on start address
    let vma_index = process
        .vmas
        .iter()
        .position(|vma| vma.start.as_u64() == addr && vma.end.as_u64() >= end_addr);

    let vma_index = match vma_index {
        Some(idx) => idx,
        None => {
            log::warn!(
                "sys_mprotect: no VMA found containing {:#x}..{:#x}",
                addr,
                end_addr
            );
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
    };

    // Get the process page table
    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => {
            log::error!("sys_mprotect: No page table for process!");
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
    };

    // Update page table flags for each page in the range
    let new_flags = prot_to_page_flags(new_prot);
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

    let mut pages_updated = 0u32;
    for page in Page::range_inclusive(start_page, end_page) {
        match page_table.update_page_flags(page, new_flags) {
            Ok(()) => {
                // Flush TLB for this page to ensure new flags take effect
                flush_tlb(page.start_address());
                pages_updated += 1;
            }
            Err(e) => {
                log::warn!(
                    "sys_mprotect: update_page_flags failed for {:#x}: {}",
                    page.start_address().as_u64(),
                    e
                );
                // Continue trying to update other pages
            }
        }
    }

    log::info!("sys_mprotect: Successfully updated {} pages", pages_updated);

    // Update VMA protection flags
    process.vmas[vma_index].prot = new_prot;

    SyscallResult::Ok(0)
}

/// Syscall 11: munmap - Unmap memory from process address space
///
/// Arguments:
/// - addr: Start address (must be page-aligned)
/// - length: Size to unmap (will be rounded up to page size)
///
/// Returns: 0 on success, negative errno on error
pub fn sys_munmap(addr: u64, length: u64) -> SyscallResult {
    log::info!("sys_munmap: addr={:#x} length={:#x}", addr, length);

    // Validate addr is page-aligned
    if !is_page_aligned(addr) {
        log::warn!("sys_munmap: address not page-aligned");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Validate length
    if length == 0 {
        log::warn!("sys_munmap: length is 0");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Round length up to page size
    let length = round_up_to_page(length);
    let end_addr = addr + length;

    // Get current thread and process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_munmap: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_munmap: No process found for thread_id={}",
                current_thread_id
            );
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Find overlapping VMAs
    // For simplicity, require exact match (don't support partial unmapping yet)
    let vma_index = process
        .vmas
        .iter()
        .position(|vma| vma.start.as_u64() == addr && vma.end.as_u64() == end_addr);

    let vma_index = match vma_index {
        Some(idx) => idx,
        None => {
            log::warn!("sys_munmap: no VMA found at {:#x}..{:#x}", addr, end_addr);
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
    };

    // Get the process page table
    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => {
            log::error!("sys_munmap: No page table for process!");
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
    };

    // Unmap pages
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

    let mut pages_unmapped = 0u32;
    for page in Page::range_inclusive(start_page, end_page) {
        match page_table.unmap_page(page) {
            Ok(frame) => {
                // Flush TLB for this page
                flush_tlb(page.start_address());
                // Free the physical frame
                crate::memory::frame_allocator::deallocate_frame(frame);
                pages_unmapped += 1;
            }
            Err(e) => {
                log::warn!(
                    "sys_munmap: unmap_page failed for {:#x}: {}",
                    page.start_address().as_u64(),
                    e
                );
                // Continue trying to unmap other pages
            }
        }
    }

    log::info!("sys_munmap: Successfully unmapped {} pages", pages_unmapped);

    // Remove VMA from process
    process.vmas.remove(vma_index);

    SyscallResult::Ok(0)
}
