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
use x86_64::VirtAddr;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;

// Import common memory syscall helpers
use crate::syscall::memory_common::{
    allocate_zeroed_frames, flush_tlb, get_current_thread_id, is_page_aligned, map_prepared_frames,
    place_below_hint, PAGE_SIZE,
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
    let noreplace = flags.contains(MmapFlags::FIXED_NOREPLACE);
    let fixed = noreplace || flags.contains(MmapFlags::FIXED);

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
        match file_mapping(fd as i32, length, prot, flags, offset) {
            Ok(handle) => Some(handle),
            Err(errno) => return SyscallResult::Err(errno),
        }
    };

    // Round length up to page size
    let Some(rounded) = length.checked_add(PAGE_SIZE - 1) else {
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    };
    let length = rounded & !(PAGE_SIZE - 1);

    // Must specify exactly one of MAP_SHARED or MAP_PRIVATE
    let is_shared = flags.contains(MmapFlags::SHARED);
    let is_private = flags.contains(MmapFlags::PRIVATE);
    if (!is_shared && !is_private) || (file.is_none() && is_shared && is_private) {
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

        let (_pid, process) = match manager.find_address_space_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => {
                log::error!(
                    "sys_mmap: No process found for thread_id={}",
                    current_thread_id
                );
                return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
            }
        };

        // A fixed mapping (MAP_FIXED, or MAP_FIXED_NOREPLACE, which refuses to
        // replace anything) is placed at `addr` exactly. It bypasses the
        // placement below, so it needs its own region check: the mapping must
        // land wholly inside the mmap region the validator polices, or the
        // address mmap returns is memory userspace can touch directly but no
        // syscall can ever accept via a user pointer (#742 M-2).
        //
        // This is a Linux-semantics deviation, deliberately (PR #744 review
        // F7): real MAP_FIXED is allowed to land anywhere in the process's
        // address space, including inside its own code/data or stack
        // windows. A mapping placed there could clobber the ELF image or a
        // live stack, so this check refuses MAP_FIXED outside the mmap region.
        let (covered, covered_data) = if fixed {
            if !is_page_aligned(addr) {
                log::warn!("sys_mmap: MAP_FIXED requires page-aligned address");
                return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
            }
            let Some(end) = addr.checked_add(length) else {
                log::warn!("sys_mmap: addr + length would overflow");
                return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
            };
            if addr < crate::memory::vma::MMAP_REGION_START
                || end > crate::memory::vma::MMAP_REGION_END
            {
                log::warn!(
                    "sys_mmap: MAP_FIXED region {:#x}..{:#x} escapes the mmap region [{:#x}, {:#x})",
                    addr,
                    end,
                    crate::memory::vma::MMAP_REGION_START,
                    crate::memory::vma::MMAP_REGION_END
                );
                return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
            }
            if noreplace && range_is_mapped(process, addr, end) {
                return SyscallResult::Err(crate::syscall::errno::EEXIST as u64);
            }
            // What the mapping replaces is not charged again.
            covered_bytes(process, addr, end)
        } else {
            (0, 0)
        };

        // Checked again against the live mappings where the VMA is installed.
        if !within_limits(process, length, (covered, covered_data), prot, flags) {
            return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
        }

        // Under mlockall(MCL_FUTURE) the mapping is locked, beside what is
        // locked already.
        let lock_bytes = if fixed {
            process.memory_locks.additional(addr, addr + length)
        } else {
            length
        };
        if super::memory_advice::prepare_future(process, lock_bytes).is_err() {
            return SyscallResult::Err(crate::syscall::errno::EAGAIN as u64);
        }

        // Otherwise a free, page-aligned hint is used as it is, and failing
        // that the mapping goes in the highest free range below the process's
        // mmap hint, whose floor is `MMAP_REGION_START` itself -- the same
        // constant `is_valid_user_range`'s mmap arm polices (#742).
        let start_addr = if fixed {
            addr
        } else if let Some(start) = free_hint(process, addr, length) {
            start
        } else {
            previous_hint = Some(process.mmap_hint);
            match place_below_hint(process, length) {
                Some(start) => start,
                None => {
                    log::error!("sys_mmap: out of mmap space");
                    return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
                }
            }
        };
        let end_addr = start_addr + length;

        log::trace!(
            "sys_mmap: allocating region {:#x}..{:#x}",
            start_addr,
            end_addr
        );

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

    if let Some((handle, may_write)) = file {
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
            may_write,
        ) {
            Ok((start, eager)) => {
                if eager {
                    let _ = super::memory_advice::populate(current_thread_id, start, end_addr);
                }
                SyscallResult::Ok(start)
            }
            Err(errno) => give_back(errno),
        };
    }

    if is_private && !flags.contains(MmapFlags::POPULATE) {
        let mut guard = crate::process::manager();
        let Some((_, process)) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(current_thread_id))
        else {
            drop(guard);
            return give_back(ErrorCode::NoSuchProcess as u64);
        };
        let vma = Vma::new(
            VirtAddr::new(start_addr),
            VirtAddr::new(end_addr),
            prot,
            flags,
        );
        if let Err(errno) = clear_for_mapping(process, start_addr, end_addr, prot, flags) {
            drop(guard);
            return give_back(errno);
        }
        process.vmas.push(vma);
        super::memory_advice::record_future(process, start_addr, end_addr);
        let eager = process.memory_locks.future && !process.memory_locks.onfault;
        drop(guard);
        if eager {
            let _ = super::memory_advice::populate(current_thread_id, start_addr, end_addr);
        }
        return SyscallResult::Ok(start_addr);
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
    // Pages another thread unmapped while they were installed stay unmapped,
    // as if it had unmapped them after this call returned.
    let populate = match map_prepared_frames(
        current_thread_id,
        root,
        start_addr,
        frames,
        crate::memory::anon_map::page_flags(prot),
        vma,
    ) {
        Ok(mapping) => mapping.populate,
        Err(errno) => return give_back(errno),
    };
    if populate {
        let _ = super::memory_advice::populate(current_thread_id, start_addr, end_addr);
    }

    SyscallResult::Ok(start_addr)
}

/// A failed mmap that took the address below `hint` gives it back, unless a
/// later mmap has already moved the hint on.
fn restore_mmap_hint(thread_id: u64, start: u64, hint: u64) {
    let mut manager_guard = crate::process::manager();
    if let Some((_, process)) = manager_guard
        .as_mut()
        .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
    {
        if process.mmap_hint == start {
            process.mmap_hint = hint;
        }
    }
}

/// Whether any VMA of `process` overlaps `[start, end)`.
fn range_is_mapped(process: &crate::process::Process, start: u64, end: u64) -> bool {
    process
        .vmas
        .iter()
        .any(|vma| vma.start.as_u64() < end && start < vma.end.as_u64())
}

/// The bytes of `[start, end)` that VMAs of `process` already cover, and how
/// many of those are private and writable (charged to RLIMIT_DATA).
fn covered_bytes(process: &crate::process::Process, start: u64, end: u64) -> (u64, u64) {
    let mut covered = (0, 0);
    for vma in &process.vmas {
        let bytes = end
            .min(vma.end.as_u64())
            .saturating_sub(start.max(vma.start.as_u64()));
        covered.0 += bytes;
        if vma.flags.contains(MmapFlags::PRIVATE) && vma.prot.contains(Protection::WRITE) {
            covered.1 += bytes;
        }
    }
    covered
}

/// Whether a mapping of `length` bytes with `prot` and `flags` fits
/// `process`'s RLIMIT_AS and RLIMIT_DATA, given the bytes of the live mappings
/// it replaces and how many of those are private and writable (`covered`),
/// which are not charged again.
fn within_limits(
    process: &crate::process::Process,
    length: u64,
    covered: (u64, u64),
    prot: Protection,
    flags: MmapFlags,
) -> bool {
    process
        .mapped_bytes()
        .saturating_sub(covered.0)
        .saturating_add(length)
        <= process.limits.get(crate::process::limits::AS).soft
        && !(flags.contains(MmapFlags::PRIVATE)
            && prot.contains(Protection::WRITE)
            && process
                .data_bytes()
                .saturating_sub(covered.1)
                .saturating_add(length)
                > process.limits.get(crate::process::limits::DATA).soft)
}

/// The hint `addr`, rounded down to a page as Linux rounds it, if `length`
/// bytes there lie inside the mmap region with nothing mapped: a mapping made
/// without MAP_FIXED is placed exactly there, as Linux places it.
fn free_hint(process: &crate::process::Process, addr: u64, length: u64) -> Option<u64> {
    let start = addr & !(PAGE_SIZE - 1);
    if start == 0 {
        return None;
    }
    let end = start.checked_add(length)?;
    (start >= crate::memory::vma::MMAP_REGION_START
        && end <= crate::memory::vma::MMAP_REGION_END
        && !range_is_mapped(process, start, end))
    .then_some(start)
}

/// Make `[start, end)` ready for a new VMA with `prot` and `flags`, with room
/// reserved to push it and, under mlockall(MCL_FUTURE), to record its lock
/// (EAGAIN past RLIMIT_MEMLOCK). The mapping must fit RLIMIT_AS and
/// RLIMIT_DATA against the live mappings (ENOMEM), which may have changed
/// since the caller first checked. MAP_FIXED unmaps what is there, splitting
/// VMAs that extend past either end, in the same PROCESS_MANAGER section as
/// the caller's push, so no other thread sees the range empty. Any other
/// mapping finds the range taken because another thread mapped there after it
/// was chosen: EEXIST for MAP_FIXED_NOREPLACE, ENOMEM otherwise.
pub(crate) fn clear_for_mapping(
    process: &mut crate::process::Process,
    start: u64,
    end: u64,
    prot: Protection,
    flags: MmapFlags,
) -> Result<(), u64> {
    // A replacement removes VMAs and pushes up to two pieces of them.
    if process.vmas.try_reserve(3).is_err() {
        return Err(ErrorCode::OutOfMemory as u64);
    }
    let mapped = range_is_mapped(process, start, end);
    if mapped && flags.contains(MmapFlags::FIXED_NOREPLACE) {
        return Err(crate::syscall::errno::EEXIST as u64);
    }
    if mapped && !flags.contains(MmapFlags::FIXED) {
        return Err(ErrorCode::OutOfMemory as u64);
    }
    if !within_limits(
        process,
        end - start,
        covered_bytes(process, start, end),
        prot,
        flags,
    ) {
        return Err(ErrorCode::OutOfMemory as u64);
    }
    // Under mlockall(MCL_FUTURE), room to record the new VMA's lock, which
    // the caller records once it has pushed the VMA.
    super::memory_advice::prepare_future(process, process.memory_locks.additional(start, end))
        .map_err(|_| crate::syscall::errno::EAGAIN as u64)?;
    if !mapped {
        return Ok(());
    }
    unmap_range(process, start, end)
}

/// The file handle for a file-backed mmap request, or its errno. The
/// descriptor is known to be open and `length` nonzero.
fn file_mapping(
    fd: i32,
    length: u64,
    prot: Protection,
    flags: MmapFlags,
    offset: u64,
) -> Result<(crate::fs::ext2::live_inode::FileHandle, bool), u64> {
    use crate::syscall::errno::{EACCES, EINVAL, ENODEV, ENOMEM, EOVERFLOW};
    let kind = flags.bits() & 3;
    if kind == 0 || !is_page_aligned(offset) {
        return Err(EINVAL as u64);
    }
    // These Linux hints need no additional machinery: mmap populates the
    // resident cache eagerly, physical pages are unswappable, and virtual
    // address reservations do not commit anonymous backing here.
    const SHARED_FLAGS: u32 = 3 | 0x10 | 0x2000 | 0x4000 | 0x8000;
    if kind == 3 && flags.bits() & !SHARED_FLAGS != 0 {
        return Err(crate::syscall::errno::EOPNOTSUPP as u64);
    }
    let pages = length.div_ceil(PAGE_SIZE);
    if (offset / PAGE_SIZE).checked_add(pages).is_none() {
        return Err(EOVERFLOW as u64);
    }
    if length.checked_add(PAGE_SIZE - 1).is_none() {
        return Err(ENOMEM as u64);
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
    let may_write = descriptor.writable();
    if kind != 2 && prot.contains(Protection::WRITE) && !may_write {
        return Err(EACCES as u64);
    }
    let handle = file.lock().handle.clone();
    Ok((handle, may_write))
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
    may_write: bool,
) -> Result<(u64, bool), u64> {
    use crate::memory::file_map::{self, MapError};
    let guard =
        crate::fs::ext2::read_mount(handle.object.mount).map_err(|_| ErrorCode::IoError as u64)?;
    let fs = guard.as_ref().ok_or(ErrorCode::IoError as u64)?;
    let ino = handle.verify(fs).map_err(|_| ErrorCode::IoError as u64)?;
    let append_only = fs
        .read_inode(ino)
        .map_err(|_| ErrorCode::IoError as u64)?
        .i_flags
        & 0x20
        != 0;
    if vma.flags.bits() & 3 != 2 && vma.prot.contains(Protection::WRITE) && append_only {
        return Err(crate::syscall::errno::EACCES as u64);
    }
    file_map::populate(&handle, fs, pgoff, vma.size() / PAGE_SIZE).map_err(
        |error| match error {
            MapError::NoMemory => ErrorCode::OutOfMemory as u64,
            MapError::Io => ErrorCode::IoError as u64,
        },
    )?;
    let object = handle.object.clone();
    let abandon = |errno: u64| {
        object.map.abandon_population();
        errno
    };
    let mut manager_guard = crate::process::manager();
    let Some((pid, process)) = manager_guard
        .as_mut()
        .and_then(|manager| manager.find_address_space_by_thread_mut(thread_id))
    else {
        return Err(abandon(ErrorCode::NoSuchProcess as u64));
    };
    if process
        .page_table
        .as_deref()
        .map(|pt| pt.level_4_frame().start_address().as_u64())
        != Some(root)
    {
        return Err(abandon(ErrorCode::OutOfMemory as u64));
    }
    let (start, end) = (vma.start.as_u64(), vma.end.as_u64());
    if let Err(errno) = clear_for_mapping(process, start, end, vma.prot, vma.flags) {
        return Err(abandon(errno));
    }
    let Some(page_table) = process.page_table.as_deref() else {
        return Err(abandon(ErrorCode::OutOfMemory as u64));
    };
    match file_map::bind(
        handle,
        pid,
        page_table,
        &vma,
        pgoff,
        may_write && !append_only,
    ) {
        Ok(binding) => vma.backing = Some(binding),
        Err(_) => return Err(abandon(ErrorCode::OutOfMemory as u64)),
    }
    process.vmas.push(vma);
    super::memory_advice::record_future(process, start, end);
    Ok((
        start,
        process.memory_locks.future && !process.memory_locks.onfault,
    ))
}

/// errno for a failed munmap or mprotect of a file VMA: a range that does not
/// fit one VMA is EINVAL; running out of memory, or an entry whose custody
/// disagreed, is ENOMEM.
fn file_vma_errno(error: &'static str) -> u64 {
    match error {
        "Permission denied" => crate::syscall::errno::EACCES as u64,
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
    // A protection bit other than PROT_READ, PROT_WRITE and PROT_EXEC is
    // refused, as Linux refuses it.
    let Some(new_prot) = Protection::from_bits(prot) else {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    };

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

    // A zero length changes nothing and succeeds, as on Linux.
    if length == 0 {
        return SyscallResult::Ok(0);
    }

    // Round length up to page size. A length or end past the top of the
    // address space is ENOMEM, as on Linux.
    let Some(end_addr) = length
        .checked_add(PAGE_SIZE - 1)
        .and_then(|rounded| addr.checked_add(rounded & !(PAGE_SIZE - 1)))
    else {
        log::warn!("sys_mprotect: addr + length would overflow");
        return SyscallResult::Err(super::errno::ENOMEM as u64);
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

    let (_pid, process) = match manager.find_address_space_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_mprotect: No process found for thread_id={}",
                current_thread_id
            );
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    // Validate complete coverage and the added DATA charge before changing any VMA.
    let mut cursor = addr;
    let mut added_data = 0u64;
    while cursor < end_addr {
        let Some(vma) = process
            .vmas
            .iter()
            .find(|v| v.start.as_u64() <= cursor && cursor < v.end.as_u64())
        else {
            return SyscallResult::Err(super::errno::ENOMEM as u64);
        };
        let next = vma.end.as_u64().min(end_addr);
        if vma.flags.contains(MmapFlags::PRIVATE)
            && !vma.prot.contains(Protection::WRITE)
            && new_prot.contains(Protection::WRITE)
        {
            added_data = added_data.saturating_add(next - cursor);
        }
        cursor = next;
    }
    if added_data != 0
        && process.data_bytes().saturating_add(added_data)
            > process.limits.get(crate::process::limits::DATA).soft
    {
        return SyscallResult::Err(super::errno::ENOMEM as u64);
    }
    if process.vmas.try_reserve(2).is_err() {
        return SyscallResult::Err(super::errno::ENOMEM as u64);
    }
    let Some(page_table) = process.page_table.as_deref_mut() else {
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    };
    let mut cursor = addr;
    while cursor < end_addr {
        let index = process
            .vmas
            .iter()
            .position(|v| v.start.as_u64() <= cursor && cursor < v.end.as_u64())
            .unwrap();
        let next = process.vmas[index].end.as_u64().min(end_addr);
        match crate::memory::file_map::protect(
            &mut process.vmas,
            page_table,
            cursor,
            next,
            new_prot,
        ) {
            Ok(true) => {
                cursor = next;
                continue;
            }
            Ok(false) => {}
            Err(error) => return SyscallResult::Err(file_vma_errno(error)),
        }
        let (start, end) = (
            process.vmas[index].start.as_u64(),
            process.vmas[index].end.as_u64(),
        );
        let new_flags = crate::memory::anon_map::page_flags(new_prot);
        let mut from = cursor;
        while let Some(page) = page_table.next_mapped_page(from, next) {
            from = page.start_address().as_u64() + PAGE_SIZE;
            let Some((_, old_flags)) = page_table.get_page_info(page) else {
                continue;
            };
            // Once a shared private page loses WRITE, its CoW flag must not
            // turn a forbidden write into an allowed copy. Privatize resident
            // CoW pages before changing permissions; untouched pages stay sparse.
            if crate::memory::process_memory::is_cow_page(old_flags)
                && !page_table.resolve_cow_write(page.start_address().as_u64(), process.id.as_u64())
            {
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
            if page_table.update_page_flags(page, new_flags).is_err() {
                return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
            }
            flush_tlb(page.start_address());
        }
        // Leave the untouched prefix/suffix with their original fault permissions.
        let prefix = (start < cursor)
            .then(|| process.vmas[index].piece(VirtAddr::new(start), VirtAddr::new(cursor)));
        let suffix = (next < end)
            .then(|| process.vmas[index].piece(VirtAddr::new(next), VirtAddr::new(end)));
        process.vmas[index].start = VirtAddr::new(cursor);
        process.vmas[index].end = VirtAddr::new(next);
        process.vmas[index].prot = new_prot;
        process.vmas.extend(prefix.into_iter().chain(suffix));
        cursor = next;
    }

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

    // Round length up to page size, refusing a length whose rounding or end
    // passes the top of the address space with EINVAL, as Linux refuses it.
    // Unchecked, the rounding wraps to zero in a release build
    // (`overflow-checks = false`), and so can `addr + length`.
    let Some(end_addr) = length
        .checked_add(PAGE_SIZE - 1)
        .and_then(|rounded| addr.checked_add(rounded & !(PAGE_SIZE - 1)))
    else {
        log::warn!("sys_munmap: addr + length would overflow");
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
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

    let (_pid, process) = match manager.find_address_space_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_munmap: No process found for thread_id={}",
                current_thread_id
            );
            return SyscallResult::Err(ErrorCode::NoSuchProcess as u64);
        }
    };

    match unmap_range(process, addr, end_addr) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// Unmap `[addr, end_addr)` (page-aligned) from `process`, splitting VMAs that
/// extend past either end, and drop the memory locks on it. A file VMA keeps
/// its binding for the parts outside the range, with their file offsets; the
/// parts inside are removed with their bindings. If an entry's custody
/// disagrees, that file VMA and its binding are kept and the error returned.
/// PROCESS_MANAGER held.
pub(crate) fn unmap_range(
    process: &mut crate::process::Process,
    addr: u64,
    end_addr: u64,
) -> Result<(), u64> {
    use crate::memory::file_map;

    // Removing the locks on a middle range can split one interval.
    process.memory_locks.reserve_split()?;

    // Split the file VMAs that extend past either end of the range first:
    // those are the only splits, and so the only steps that allocate.
    let straddling = |vmas: &[Vma], at: u64| {
        vmas.iter()
            .find(|v| v.backing.is_some() && v.start.as_u64() < at && at < v.end.as_u64())
            .map(|v| (v.start.as_u64(), v.end.as_u64()))
    };
    if let Some((_, end)) = straddling(&process.vmas, addr) {
        file_map::isolate(&mut process.vmas, addr, end.min(end_addr), 0)
            .map_err(file_vma_errno)?;
    }
    if let Some((start, _)) = straddling(&process.vmas, end_addr) {
        file_map::isolate(&mut process.vmas, start.max(addr), end_addr, 0)
            .map_err(file_vma_errno)?;
    }
    let Some(page_table) = process.page_table.as_deref_mut() else {
        return Err(ErrorCode::OutOfMemory as u64);
    };
    // Every file VMA the range touches now lies inside it.
    while let Some(index) = process
        .vmas
        .iter()
        .position(|v| v.backing.is_some() && v.start.as_u64() < end_addr && addr < v.end.as_u64())
    {
        file_map::unmap(&mut process.vmas, index, page_table).map_err(file_vma_errno)?;
    }

    if process.vmas.try_reserve(2).is_err() {
        return Err(ErrorCode::OutOfMemory as u64);
    }
    let mut from = addr;
    while let Some(page) = page_table.next_mapped_page(from, end_addr) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        match page_table.unmap_page_deferred(page) {
            Ok(leaf) => leaf.flush().release(),
            Err(_) => return Err(ErrorCode::OutOfMemory as u64),
        }
    }
    let mut index = 0;
    while index < process.vmas.len() {
        let vma = &process.vmas[index];
        let (start, end) = (vma.start.as_u64(), vma.end.as_u64());
        if start >= end_addr || end <= addr {
            index += 1;
            continue;
        }
        let prefix = (start < addr).then(|| vma.piece(vma.start, VirtAddr::new(addr)));
        let suffix = (end_addr < end).then(|| vma.piece(VirtAddr::new(end_addr), vma.end));
        process.vmas.remove(index);
        process.vmas.extend(prefix.into_iter().chain(suffix));
    }

    process.memory_locks.remove(addr, end_addr);
    Ok(())
}

/// msync validates the complete virtual range before doing any filesystem
/// work. Only covered shared file ranges enter the snapshot; no mount guard
/// is acquired while PROCESS_MANAGER is held.
pub fn sys_msync(addr: u64, length: u64, flags: u32) -> SyscallResult {
    use super::errno::{EBUSY, EINVAL, EIO, ENOMEM};
    const MS_ASYNC: u32 = 1;
    const MS_INVALIDATE: u32 = 2;
    const MS_SYNC: u32 = 4;
    if !is_page_aligned(addr)
        || flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0
        || flags & (MS_ASYNC | MS_SYNC) == (MS_ASYNC | MS_SYNC)
    {
        return SyscallResult::Err(EINVAL as u64);
    }
    let Some(length) = length
        .checked_add(PAGE_SIZE - 1)
        .map(|n| n & !(PAGE_SIZE - 1))
    else {
        return SyscallResult::Err(ENOMEM as u64);
    };
    let Some(end) = addr.checked_add(length) else {
        return SyscallResult::Err(ENOMEM as u64);
    };
    if length == 0 {
        return SyscallResult::Ok(0);
    }
    let mut ranges = alloc::vec::Vec::new();
    {
        let thread = match get_current_thread_id() {
            Some(thread) => thread,
            None => return SyscallResult::Err(ENOMEM as u64),
        };
        let guard = crate::process::manager();
        let Some((_, process)) = guard
            .as_ref()
            .and_then(|pm| pm.find_process_by_thread(thread))
        else {
            return SyscallResult::Err(ENOMEM as u64);
        };
        if flags & MS_INVALIDATE != 0 && process.memory_locks.overlaps(addr, end) {
            return SyscallResult::Err(EBUSY as u64);
        }
        let mut cursor = addr;
        while cursor < end {
            let Some(vma) = process
                .vmas
                .iter()
                .find(|vma| vma.start.as_u64() <= cursor && cursor < vma.end.as_u64())
            else {
                return SyscallResult::Err(ENOMEM as u64);
            };
            let stop = end.min(vma.end.as_u64());
            if vma.flags.bits() & 3 != 2 {
                if let Some(binding) = &vma.backing {
                    if ranges.try_reserve(1).is_err() {
                        return SyscallResult::Err(ENOMEM as u64);
                    }
                    match binding.sync_range(cursor, stop) {
                        Ok(range) => ranges.push(range),
                        Err(_) => return SyscallResult::Err(EIO as u64),
                    }
                }
            }
            cursor = stop;
        }
    }
    for (handle, first, last) in ranges {
        if flags & MS_SYNC == 0 {
            handle.object.map.request_writeback();
            continue;
        }
        let result = crate::memory::file_map::sync_range(&handle, first, last);
        if result.is_err() {
            return SyscallResult::Err(EIO as u64);
        }
    }
    SyscallResult::Ok(0)
}
