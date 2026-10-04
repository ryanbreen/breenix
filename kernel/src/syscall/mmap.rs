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
    map_prepared_frames, prot_to_page_flags, round_down_to_page, round_up_to_page, PAGE_SIZE,
};

extern crate alloc;

/// Whether `fd` is an open descriptor of the calling process.
fn descriptor_is_open(fd: i32) -> bool {
    let Some(thread_id) = get_current_thread_id() else {
        return false;
    };
    let manager_guard = crate::process::manager();
    manager_guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(thread_id))
        .map_or(false, |(_pid, process)| process.fd_table.get(fd).is_some())
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

    // A file mapping names a descriptor, which is looked up before any other
    // argument is checked: one that is not open is EBADF. Argument errors
    // (EINVAL) come next, and then the descriptor's kind and access mode.
    if !flags.contains(MmapFlags::ANONYMOUS) && !descriptor_is_open(fd as i32) {
        return SyscallResult::Err(crate::syscall::errno::EBADF as u64);
    }

    // Validate length
    if length == 0 {
        log::warn!("sys_mmap: length is 0");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    let file = if flags.contains(MmapFlags::ANONYMOUS) {
        None
    } else {
        match file_mapping(fd as i32, length, flags, offset) {
            Ok(handle) => Some(handle),
            Err(errno) => return SyscallResult::Err(errno),
        }
    };

    // Round length up to page size
    let length = round_up_to_page(length);

    // Must specify exactly one of MAP_SHARED or MAP_PRIVATE
    let is_shared = flags.contains(MmapFlags::SHARED);
    let is_private = flags.contains(MmapFlags::PRIVATE);
    if !is_shared && !is_private {
        log::warn!("sys_mmap: must specify MAP_SHARED or MAP_PRIVATE");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // File descriptor should be -1 for anonymous mappings
    if file.is_none() && fd != -1 {
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

    // Choose the region under PROCESS_MANAGER, then release it: the frames are
    // allocated and zeroed with no lock held, and map_prepared_frames maps them
    // under the lock a chunk at a time, re-checking the region and the page
    // table in each section. Holding the lock (IRQs off on ARM64) across
    // hundreds of allocations starved every other CPU spinning in manager().
    // The hint before this call chose an address below it; a mapping that
    // then fails gives the address back.
    let mut previous_hint = None;
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
            previous_hint = Some(hint);
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

    let give_back = |errno: u64| {
        if let Some(hint) = previous_hint {
            restore_mmap_hint(current_thread_id, start_addr, hint);
        }
        SyscallResult::Err(errno)
    };

    if let Some(handle) = file {
        return match map_file(
            handle,
            current_thread_id,
            root,
            Vma::new(
                VirtAddr::new(start_addr),
                VirtAddr::new(end_addr),
                prot,
                flags,
            ),
            offset / PAGE_SIZE,
        ) {
            Ok(start) => SyscallResult::Ok(start),
            Err(errno) => give_back(errno),
        };
    }

    let page_count = ((end_addr - start_addr) / PAGE_SIZE) as usize;
    let Some(frames) = allocate_zeroed_frames(page_count) else {
        log::error!("sys_mmap: OOM allocating {} frames", page_count);
        return give_back(ErrorCode::OutOfMemory as u64);
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
        return give_back(error as u64);
    }

    SyscallResult::Ok(start_addr)
}

/// A failed mmap that took the address below `hint` gives it back, unless a
/// later mmap has already moved the hint on.
fn restore_mmap_hint(thread_id: u64, start: u64, hint: u64) {
    let mut manager_guard = crate::process::manager();
    if let Some((_, process)) = manager_guard
        .as_mut()
        .and_then(|manager| manager.find_process_by_thread_mut(thread_id))
    {
        if process.mmap_hint == start {
            process.mmap_hint = hint;
        }
    }
}

/// The file handle for a file-backed mmap request, or its errno. The
/// descriptor is known to be open and `length` nonzero.
fn file_mapping(
    fd: i32,
    length: u64,
    flags: MmapFlags,
    offset: u64,
) -> Result<crate::fs::ext2::live_inode::FileHandle, u64> {
    use crate::syscall::errno::{EACCES, EINVAL, ENODEV, ENOMEM, EOVERFLOW};
    // Shared file mappings (MAP_SHARED, and MAP_SHARED_VALIDATE = 3) are not
    // supported yet; only MAP_PRIVATE is.
    if flags.bits() & 3 != MmapFlags::PRIVATE.bits() || !is_page_aligned(offset) {
        return Err(EINVAL as u64);
    }
    let pages = length.div_ceil(PAGE_SIZE);
    if (offset / PAGE_SIZE).checked_add(pages).is_none() {
        return Err(EOVERFLOW as u64);
    }
    if length.checked_add(PAGE_SIZE - 1).is_none() {
        return Err(ENOMEM as u64);
    }
    if !crate::memory::file_map::local_invalidation_suffices() {
        return Err(ENODEV as u64);
    }
    let thread_id = get_current_thread_id().ok_or(ErrorCode::NoSuchProcess as u64)?;
    let manager_guard = crate::process::manager();
    let (_pid, process) = manager_guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(thread_id))
        .ok_or(ErrorCode::NoSuchProcess as u64)?;
    let descriptor = process
        .fd_table
        .get(fd)
        .ok_or(crate::syscall::errno::EBADF as u64)?;
    let crate::ipc::FdKind::RegularFile(file) = &descriptor.kind else {
        return Err(ENODEV as u64);
    };
    if !descriptor.readable() {
        return Err(EACCES as u64);
    }
    let handle = file.lock().handle.clone();
    Ok(handle)
}

/// Populate the cache for a file VMA under the mount's read guard, then
/// register it in a short PROCESS_MANAGER section. Entries are installed by
/// faults, not here. If registration fails, the pages population read are
/// left for the ext2 service to retire.
fn map_file(
    handle: crate::fs::ext2::live_inode::FileHandle,
    thread_id: u64,
    root: u64,
    mut vma: Vma,
    pgoff: u64,
) -> Result<u64, u64> {
    use crate::memory::file_map::{self, MapError};
    let guard =
        crate::fs::ext2::read_mount(handle.object.mount).map_err(|_| ErrorCode::IoError as u64)?;
    let fs = guard.as_ref().ok_or(ErrorCode::IoError as u64)?;
    file_map::populate(&handle, fs, pgoff, vma.size() / PAGE_SIZE).map_err(
        |error| match error {
            MapError::NoMemory => ErrorCode::OutOfMemory as u64,
            MapError::Io => ErrorCode::IoError as u64,
        },
    )?;
    let object = handle.object.clone();
    let abandon = |errno: ErrorCode| {
        object.map.abandon_population();
        errno as u64
    };
    let mut manager_guard = crate::process::manager();
    let Some((pid, process)) = manager_guard
        .as_mut()
        .and_then(|manager| manager.find_process_by_thread_mut(thread_id))
    else {
        return Err(abandon(ErrorCode::NoSuchProcess));
    };
    if process.vmas.iter().any(|other| vma.overlaps(other)) {
        return Err(abandon(ErrorCode::OutOfMemory));
    }
    let Some(page_table) = process
        .page_table
        .as_deref()
        .filter(|pt| pt.level_4_frame().start_address().as_u64() == root)
    else {
        return Err(abandon(ErrorCode::OutOfMemory));
    };
    if process.vmas.try_reserve(1).is_err() {
        return Err(abandon(ErrorCode::OutOfMemory));
    }
    match file_map::bind(handle, pid, page_table, &vma, pgoff) {
        Ok(binding) => vma.backing = Some(binding),
        Err(_) => return Err(abandon(ErrorCode::OutOfMemory)),
    }
    let start = vma.start.as_u64();
    process.vmas.push(vma);
    Ok(start)
}

/// errno for a failed munmap or mprotect of a file VMA: a range that does not
/// fit one VMA is EINVAL; running out of memory, or an entry whose custody
/// disagreed, is ENOMEM.
fn file_vma_errno(error: &'static str) -> u64 {
    match error {
        "Range spans more than one VMA" | "Split point outside the VMA" => {
            ErrorCode::InvalidArgument as u64
        }
        _ => ErrorCode::OutOfMemory as u64,
    }
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

    // A range within one file VMA is split out and changed on its own.
    if let Some(page_table) = process.page_table.as_deref_mut() {
        match crate::memory::file_map::protect(
            &mut process.vmas,
            page_table,
            addr,
            end_addr,
            new_prot,
        ) {
            Ok(true) => return SyscallResult::Ok(0),
            Ok(false) => {}
            Err(error) => return SyscallResult::Err(file_vma_errno(error)),
        }
    }

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
    let length = round_up_to_page(length);
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

    // A range within one file VMA is split out and unmapped on its own. If an
    // entry's custody disagrees, the VMA and its binding are kept.
    match crate::memory::file_map::isolate(&mut process.vmas, addr, end_addr, 0) {
        Ok(Some(index)) => {
            let Some(page_table) = process.page_table.as_deref_mut() else {
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            };
            return match crate::memory::file_map::unmap(&mut process.vmas, index, page_table) {
                Ok(()) => SyscallResult::Ok(0),
                Err(error) => SyscallResult::Err(file_vma_errno(error)),
            };
        }
        Ok(None) => {}
        Err(error) => return SyscallResult::Err(file_vma_errno(error)),
    }

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

    // Remove VMA from process
    process.vmas.remove(vma_index);

    SyscallResult::Ok(0)
}
