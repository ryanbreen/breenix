//! Memory mapping system calls (mmap, munmap)
//!
//! This module implements mmap() and munmap() for mapping and unmapping
//! memory regions in userspace process address spaces.

use crate::memory::vma::{MmapFlags, Protection, Vma};
use crate::syscall::{ErrorCode, SyscallResult};
use x86_64::structures::paging::{PhysFrame, Page, PageTableFlags, Size4KiB};
use x86_64::instructions::tlb;
use x86_64::VirtAddr;

extern crate alloc;

/// Page size constant (4 KiB)
const PAGE_SIZE: u64 = 4096;

/// Convert protection flags to page table flags
fn prot_to_page_flags(prot: Protection) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if prot.contains(Protection::WRITE) {
        flags |= PageTableFlags::WRITABLE;
    }
    // Note: x86_64 doesn't have a built-in execute-disable bit in basic paging
    // NX (No-Execute) requires enabling NXE bit in EFER MSR, which we can add later
    flags
}

/// Round up to page size
#[inline]
fn round_up_to_page(size: u64) -> u64 {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Round down to page size
#[inline]
fn round_down_to_page(addr: u64) -> u64 {
    addr & !(PAGE_SIZE - 1)
}

/// Check if address is page-aligned
#[inline]
fn is_page_aligned(addr: u64) -> bool {
    (addr & (PAGE_SIZE - 1)) == 0
}

/// Helper function to clean up mapped pages on mmap failure (Issue 2)
fn cleanup_mapped_pages(
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
    mapped_pages: &[(Page<Size4KiB>, PhysFrame<Size4KiB>)],
) {
    log::warn!("sys_mmap: cleaning up {} already-mapped pages due to failure", mapped_pages.len());

    for (page, frame) in mapped_pages.iter() {
        // Unmap the page
        match page_table.unmap_page(*page) {
            Ok(_) => {
                // Flush TLB
                tlb::flush(page.start_address());
                // Free the frame
                crate::memory::frame_allocator::deallocate_frame(*frame);
            }
            Err(e) => {
                log::error!("sys_mmap cleanup: failed to unmap page {:#x}: {}", page.start_address().as_u64(), e);
                // Still try to free the frame
                crate::memory::frame_allocator::deallocate_frame(*frame);
            }
        }
    }
}

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
pub fn sys_mmap(addr: u64, length: u64, prot: u32, flags: u32, fd: i64, offset: u64) -> SyscallResult {
    let prot = Protection::from_bits_truncate(prot);
    let flags = MmapFlags::from_bits_truncate(flags);

    log::info!(
        "sys_mmap: addr={:#x} length={:#x} prot={:?} flags={:?} fd={} offset={:#x}",
        addr, length, prot, flags, fd, offset
    );

    // Validate length
    if length == 0 {
        log::warn!("sys_mmap: length is 0");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Round length up to page size
    let length = round_up_to_page(length);

    // For now, only support MAP_ANONYMOUS | MAP_PRIVATE
    if !flags.contains(MmapFlags::ANONYMOUS) {
        log::warn!("sys_mmap: file-backed mappings not yet supported");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    if !flags.contains(MmapFlags::PRIVATE) {
        log::warn!("sys_mmap: only MAP_PRIVATE is supported");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // File descriptor should be -1 for anonymous mappings
    if fd != -1 {
        log::warn!("sys_mmap: fd must be -1 for MAP_ANONYMOUS");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Get current thread and process
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
        None => {
            log::error!("sys_mmap: No current thread in per-CPU data!");
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
            log::error!("sys_mmap: No process found for thread_id={}", current_thread_id);
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
        // Find a free region using the mmap hint (grows downward)
        // For simplicity, just use the current hint and decrement it
        let hint = process.mmap_hint;
        let new_addr = hint.saturating_sub(length);

        // Make sure it's page-aligned
        let new_addr = round_down_to_page(new_addr);

        // Validate it doesn't go below a reasonable minimum
        if new_addr < 0x1000_0000 {
            log::error!("sys_mmap: out of mmap space");
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }

        // Update hint for next allocation
        process.mmap_hint = new_addr;
        new_addr
    };

    // Check for overflow when calculating end address (Issue 3)
    let end_addr = match start_addr.checked_add(length) {
        Some(addr) => addr,
        None => {
            log::warn!("sys_mmap: start_addr + length would overflow (start={:#x}, length={:#x})", start_addr, length);
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
    };

    log::info!("sys_mmap: allocating region {:#x}..{:#x}", start_addr, end_addr);

    // Check for overlaps with existing VMAs
    for vma in &process.vmas {
        let vma_start = vma.start.as_u64();
        let vma_end = vma.end.as_u64();

        // Check if regions overlap
        if start_addr < vma_end && end_addr > vma_start {
            log::warn!("sys_mmap: region overlaps with existing VMA at {:#x}..{:#x}", vma_start, vma_end);
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
    }

    // Get the process page table
    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => {
            log::error!("sys_mmap: No page table for process!");
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
    };

    // Map pages
    let page_flags = prot_to_page_flags(prot);
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

    // Get physical memory offset for zeroing pages (Issue 1)
    let physical_memory_offset = crate::memory::physical_memory_offset();

    // Track successfully mapped pages for cleanup on failure (Issue 2)
    let mut mapped_pages: alloc::vec::Vec<(Page<Size4KiB>, PhysFrame<Size4KiB>)> = alloc::vec::Vec::new();

    let mut current_page = start_page;

    loop {
        // Allocate a physical frame
        let frame = match crate::memory::frame_allocator::allocate_frame() {
            Some(f) => f,
            None => {
                log::error!("sys_mmap: OOM allocating frame for page {:#x}", current_page.start_address().as_u64());
                // Issue 2: Clean up already-mapped pages
                cleanup_mapped_pages(page_table, &mapped_pages);
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
        };

        // Map the page
        if let Err(e) = page_table.map_page(current_page, frame, page_flags) {
            log::error!("sys_mmap: map_page failed for {:#x}: {}", current_page.start_address().as_u64(), e);
            // Free the frame we just allocated but failed to map
            crate::memory::frame_allocator::deallocate_frame(frame);
            // Issue 2: Clean up already-mapped pages
            cleanup_mapped_pages(page_table, &mapped_pages);
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }

        // Issue 1: Zero the page contents (POSIX requires MAP_ANONYMOUS pages to be zeroed)
        // Convert physical frame address to kernel-accessible virtual address via HHDM
        let phys_addr = frame.start_address().as_u64();
        let virt_ptr = (physical_memory_offset.as_u64() + phys_addr) as *mut u8;
        unsafe {
            core::ptr::write_bytes(virt_ptr, 0, PAGE_SIZE as usize);
        }

        // Flush TLB for this page
        tlb::flush(current_page.start_address());

        // Track this mapping for potential cleanup
        mapped_pages.push((current_page, frame));

        // Stop after mapping the end page
        if current_page >= end_page {
            break;
        }
        current_page += 1;
    }

    let pages_mapped = mapped_pages.len();

    log::info!("sys_mmap: Successfully mapped {} pages", pages_mapped);

    // Create VMA and add to process
    let vma = Vma::new(
        VirtAddr::new(start_addr),
        VirtAddr::new(end_addr),
        prot,
        flags,
    );
    process.vmas.push(vma);

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
        addr, length, new_prot
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
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
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
            log::error!("sys_mprotect: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Find the VMA that contains this address range
    // For simplicity, require exact match on start address
    let vma_index = process.vmas.iter().position(|vma| {
        vma.start.as_u64() == addr && vma.end.as_u64() >= end_addr
    });

    let vma_index = match vma_index {
        Some(idx) => idx,
        None => {
            log::warn!("sys_mprotect: no VMA found containing {:#x}..{:#x}", addr, end_addr);
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
                tlb::flush(page.start_address());
                pages_updated += 1;
            }
            Err(e) => {
                log::warn!("sys_mprotect: update_page_flags failed for {:#x}: {}", page.start_address().as_u64(), e);
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
    let current_thread_id = match crate::per_cpu::current_thread() {
        Some(thread) => thread.id,
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
            log::error!("sys_munmap: No process found for thread_id={}", current_thread_id);
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Find overlapping VMAs
    // For simplicity, require exact match (don't support partial unmapping yet)
    let vma_index = process.vmas.iter().position(|vma| {
        vma.start.as_u64() == addr && vma.end.as_u64() == end_addr
    });

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
                tlb::flush(page.start_address());
                // Free the physical frame
                crate::memory::frame_allocator::deallocate_frame(frame);
                pages_unmapped += 1;
            }
            Err(e) => {
                log::warn!("sys_munmap: unmap_page failed for {:#x}: {}", page.start_address().as_u64(), e);
                // Continue trying to unmap other pages
            }
        }
    }

    log::info!("sys_munmap: Successfully unmapped {} pages", pages_unmapped);

    // Remove VMA from process
    process.vmas.remove(vma_index);

    SyscallResult::Ok(0)
}
