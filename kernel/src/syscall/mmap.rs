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
use x86_64::structures::paging::{Page, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};

// Import common memory syscall helpers
use crate::syscall::memory_common::{
    allocate_zeroed_frames, flush_tlb, get_current_thread_id, is_page_aligned,
    map_prepared_frames, prot_to_page_flags, round_down_to_page, PAGE_SIZE,
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

    log::trace!(
        "sys_mmap: addr={:#x} length={:#x} prot={:?} flags={:?} fd={} offset={:#x}",
        addr,
        length,
        prot,
        flags,
        fd,
        offset
    );

    if length == 0 || !is_page_aligned(offset) {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }
    let Some(length) = length.checked_add(PAGE_SIZE - 1).map(round_down_to_page) else {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    };
    if offset.checked_add(length).is_none() {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }
    let mapping_type = flags.bits() & 3;
    if mapping_type != 1 && mapping_type != 2 {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }
    let anonymous = flags.contains(MmapFlags::ANONYMOUS);
    if anonymous && fd != -1 {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }
    let file = if anonymous { None } else {
        let Some(thread_id) = get_current_thread_id() else {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        };
        let guard = crate::process::manager();
        let Some((_, process)) = guard.as_ref().and_then(|m| m.find_process_by_thread(thread_id)) else {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        };
        let Some(descriptor) = i32::try_from(fd).ok().and_then(|fd| process.fd_table.get(fd)) else {
            return SyscallResult::Err(crate::syscall::errno::EBADF as u64);
        };
        // Shared file mappings are enabled in PR C.
        if mapping_type != 2 { return SyscallResult::Err(ErrorCode::InvalidArgument as u64); }
        if !descriptor.readable() { return SyscallResult::Err(crate::syscall::errno::EACCES as u64); }
        let crate::ipc::FdKind::RegularFile(file) = &descriptor.kind else {
            return SyscallResult::Err(19); // ENODEV: this descriptor cannot back a file VMA.
        };
        let handle = file.lock().handle.clone();
        Some(handle)
    };

    // Get current thread and process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_mmap: No current thread in per-CPU data!");
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Choose the region under PROCESS_MANAGER, then release it: the frames are
    // allocated and zeroed with no lock held, and map_prepared_frames maps them
    // under the lock a chunk at a time, re-checking the region and the page
    // table in each section. Holding the lock (IRQs off on ARM64) across
    // hundreds of allocations starved every other CPU spinning in manager().
    let (start_addr, end_addr, root) = {
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
            // The floor is `MMAP_REGION_START` itself -- the same constant
            // `is_valid_user_range`'s mmap arm polices -- not a second,
            // independently hardcoded number. Before this the allocator
            // floor (0x1000_0000) and the validator's floor
            // (`MMAP_REGION_START`, 0x7000_0000_0000) were ~17.6 TB apart,
            // so a sufficiently-descended `mmap_hint` could hand out a
            // "successfully mapped" region every syscall pointer into it
            // would then EFAULT against (#742, the same "validator anchored
            // to a bound the allocator does not use" shape as #729's B4-a).
            if new_addr < crate::memory::vma::MMAP_REGION_START {
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

        // MAP_FIXED bypasses the hint-descent floor above entirely, so it
        // needs its own region check: the mapping must land wholly inside
        // the mmap region the validator polices, or the address mmap
        // returns is memory userspace can touch directly but no syscall
        // can ever accept via a user pointer (#742 M-2). The non-FIXED arm
        // above never needs this check itself -- `new_addr` is bounded
        // below by the floor just enforced, and above by construction
        // (`mmap_hint` only ever descends from `MMAP_REGION_END`, so
        // `end_addr` here equals the pre-allocation hint, which is always
        // <= `MMAP_REGION_END`).
        //
        // This is a Linux-semantics deviation, deliberately (PR #744 review
        // F7): real MAP_FIXED is allowed to land anywhere in the process's
        // address space, including inside its own code/data or stack
        // windows (that is how Linux lets a program remap over itself).
        // This check refuses MAP_FIXED outside `[MMAP_REGION_START,
        // MMAP_REGION_END)` even for addresses `is_valid_user_range` would
        // otherwise accept as code/data or stack -- there are zero in-tree
        // callers of MAP_FIXED today, and a mapping placed there could
        // clobber the ELF image or a live stack, so refusing the wider
        // allow-list is the conservative, correct call here. It is not
        // simply mirroring the validator's allow-list.
        if flags.contains(MmapFlags::FIXED)
            && (start_addr < crate::memory::vma::MMAP_REGION_START
                || end_addr > crate::memory::vma::MMAP_REGION_END)
        {
            log::warn!(
                "sys_mmap: MAP_FIXED region {:#x}..{:#x} escapes the mmap region [{:#x}, {:#x})",
                start_addr,
                end_addr,
                crate::memory::vma::MMAP_REGION_START,
                crate::memory::vma::MMAP_REGION_END
            );
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }

        log::trace!(
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

        let root = page_table.level_4_frame().start_address().as_u64();
        (start_addr, end_addr, root)
    };

    let page_count = ((end_addr - start_addr) / PAGE_SIZE) as usize;
    if let Some(handle) = file {
        let guard = match crate::fs::ext2::read_mount(handle.object.mount) {
            Ok(guard) => guard,
            Err(_) => return SyscallResult::Err(crate::syscall::errno::EIO as u64),
        };
        let fs = guard.as_ref().expect("verified ext2 mount");
        if let Err(error) = crate::memory::file_map::populate(&handle, fs, offset / PAGE_SIZE, page_count as u64) {
            return SyscallResult::Err(if error.starts_with("Out of memory") {
                ErrorCode::OutOfMemory as u64
            } else { crate::syscall::errno::EIO as u64 });
        }
        let mut manager_guard = crate::process::manager();
        let Some((pid, process)) = manager_guard.as_mut().and_then(|m| m.find_process_by_thread_mut(current_thread_id)) else {
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        };
        if process.vmas.iter().any(|vma| start_addr < vma.end.as_u64() && end_addr > vma.start.as_u64()) {
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
        if process.vmas.try_reserve(1).is_err() { return SyscallResult::Err(ErrorCode::OutOfMemory as u64); }
        let Some(pt) = process.page_table.as_mut().filter(|pt| pt.level_4_frame().start_address().as_u64() == root) else {
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        };
        let mut vma = Vma::new(VirtAddr::new(start_addr), VirtAddr::new(end_addr), prot, flags);
        if crate::memory::file_map::install(handle, pid, pt, &mut vma, offset / PAGE_SIZE).is_err() {
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
        process.vmas.push(vma);
        return SyscallResult::Ok(start_addr);
    }
    let Some(frames) = allocate_zeroed_frames(page_count) else {
        log::error!("sys_mmap: OOM allocating {} frames", page_count);
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    };
    let vma = Vma::new(
        VirtAddr::new(start_addr),
        VirtAddr::new(end_addr),
        prot,
        flags,
    );
    if let Err(error) = map_prepared_frames(
        current_thread_id,
        root,
        start_addr,
        frames,
        prot_to_page_flags(prot),
        vma,
    ) {
        return SyscallResult::Err(error as u64);
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

    log::trace!(
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
    let Some(length) = length.checked_add(PAGE_SIZE - 1).map(round_down_to_page) else {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    };
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

    if let Some(binding) = process.vmas[vma_index].backing.as_ref() {
        if process.vmas[vma_index].end.as_u64() != end_addr {
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
        if crate::memory::file_map::protect(page_table, binding, new_prot).is_err() {
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }
        process.vmas[vma_index].prot = new_prot;
        return SyscallResult::Ok(0);
    }

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

    log::trace!("sys_mprotect: Successfully updated {} pages", pages_updated);

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
    log::trace!("sys_munmap: addr={:#x} length={:#x}", addr, length);

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
    let Some(length) = length.checked_add(PAGE_SIZE - 1).map(round_down_to_page) else {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    };
    // `checked_add`, matching `sys_mmap`'s sibling computation above (PR
    // #744 review F8): an overflowing `addr + length` used to fall out as
    // EINVAL only incidentally, via the exact-match VMA lookup below never
    // finding a VMA ending at the wrapped address in a release build
    // (`overflow-checks = false`, no `[profile]` override in this
    // workspace). Refuse it directly instead of relying on that.
    let end_addr = match addr.checked_add(length) {
        Some(end_addr) => end_addr,
        None => {
            log::warn!("sys_munmap: addr + length would overflow");
            return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
        }
    };

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
        match page_table.unmap_page_deferred(page) {
            Ok(leaf) => {
                leaf.flush().release();
                pages_unmapped += 1;
            }
            Err(e) => {
                if process.vmas[vma_index].backing.is_some() { continue; }
                log::warn!(
                    "sys_munmap: unmap_page failed for {:#x}: {}",
                    page.start_address().as_u64(),
                    e
                );
                // Continue trying to unmap other pages
            }
        }
    }

    log::trace!("sys_munmap: Successfully unmapped {} pages", pages_unmapped);

    // Remove both binding owners; the table keeps them through exec/exit retirement.
    if let Some(binding) = process.vmas[vma_index].backing.as_ref() {
        crate::memory::file_map::remove(page_table, binding);
    }
    process.vmas.remove(vma_index);

    SyscallResult::Ok(0)
}
