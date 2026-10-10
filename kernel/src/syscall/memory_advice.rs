//! Memory residency, locking and advice. All metadata belongs to the CLONE_VM owner.
use super::errno::{EINVAL, ENOMEM, EPERM};
use super::memory_common::{get_current_thread_id, PAGE_SIZE};
use super::SyscallResult;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB, VirtAddr};
use crate::memory::locked::MemoryLocks;
use crate::memory::vma::MmapFlags;
use crate::process::Process;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, Size4KiB},
    VirtAddr,
};

fn result(value: Result<(), u64>) -> SyscallResult {
    match value {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

fn range(addr: u64, length: u64, aligned: bool) -> Result<(u64, u64), u64> {
    if aligned && addr & (PAGE_SIZE - 1) != 0 {
        return Err(EINVAL as u64);
    }
    let start = addr & !(PAGE_SIZE - 1);
    let end = addr
        .checked_add(length)
        .and_then(|end| end.checked_add(PAGE_SIZE - 1))
        .map(|end| end & !(PAGE_SIZE - 1))
        .ok_or(ENOMEM as u64)?;
    if end > crate::memory::layout::USER_STACK_REGION_END {
        return Err(ENOMEM as u64);
    }
    Ok((start, if length == 0 { start } else { end }))
}

/// VMA reservations count as mapped even before their first touch; elsewhere
/// only present user leaves count. Do not resurrect holes in an ELF or heap.
fn covered(process: &Process, start: u64, end: u64) -> bool {
    let mut cursor = start;
    while cursor < end {
        if let Some(vma) = process
            .vmas
            .iter()
            .find(|v| v.start.as_u64() <= cursor && cursor < v.end.as_u64())
        {
            cursor = end.min(vma.end.as_u64());
        } else {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(cursor));
            if !process
                .page_table
                .as_ref()
                .and_then(|t| t.get_page_info(page))
                .is_some_and(|(_, flags)| flags.contains(PageTableFlags::USER_ACCESSIBLE))
            {
                return false;
            }
            cursor += PAGE_SIZE;
        }
    }
    true
}

pub(crate) fn check_limit(process: &Process, additional: u64) -> Result<(), u64> {
    // Root has the kernel's CAP_IPC_LOCK equivalent. Locks are independent
    // of RLIMIT_AS and RLIMIT_DATA and charged in whole pages, once per VA.
    if process.cred.euid == 0 {
        return Ok(());
    }
    let limit = process.limits.get(crate::process::limits::MEMLOCK).soft & !(PAGE_SIZE - 1);
    if limit == 0 {
        return Err(EPERM as u64);
    }
    if process.memory_locks.bytes().saturating_add(additional) > limit {
        return Err(ENOMEM as u64);
    }
    Ok(())
}

/// Prepare bookkeeping before mapping new pages. Caller holds PM; the
/// resident frames themselves are unswappable. Called by mmap, brk and stack growth.
pub(crate) fn prepare_future(process: &mut Process, bytes: u64) -> Result<(), u64> {
    if process.memory_locks.future {
        check_limit(process, bytes)?;
        process.memory_locks.reserve_split()?;
    }
    Ok(())
}

pub(crate) fn record_future(process: &mut Process, start: u64, end: u64) {
    if process.memory_locks.future && start < end {
        // prepare_future reserved the single insertion, under the same PM lock.
        process
            .memory_locks
            .insert(start, end)
            .expect("reserved memory lock interval");
    }
}

/// Make an anonymous VMA resident without touching user VA or changing its
/// protections. A locked PROT_NONE reservation still owns physical pages.
fn populate_page(process: &mut Process, address: u64) -> Result<bool, u64> {
    let table = process.page_table.as_deref_mut().ok_or(ENOMEM as u64)?;
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    if table.translate(page.start_address()).is_some() {
        return Ok(true);
    }
    let vma = process
        .vmas
        .iter()
        .find(|v| v.contains(page.start_address()))
        .ok_or(ENOMEM as u64)?;
    if vma.backing.is_some() {
        use crate::memory::file_map::{Access, FaultOutcome};
        let access = if vma.prot.contains(crate::memory::vma::Protection::READ) {
            Access::Read
        } else if vma.prot.contains(crate::memory::vma::Protection::WRITE) {
            Access::Write
        } else {
            Access::Execute
        };
        if !matches!(
            crate::memory::file_map::resolve_page(table, &process.vmas, address, access),
            FaultOutcome::Resolved
        ) {
            return Err(ENOMEM as u64);
        }
        return Ok(table.translate(page.start_address()).is_some());
    }
    let frames = super::memory_common::allocate_zeroed_frames(1).ok_or(ENOMEM as u64)?;
    let frame = frames[0];
    if table
        .map_page(page, frame, crate::memory::anon_map::page_flags(vma.prot))
        .is_err()
    {
        crate::memory::frame_allocator::deallocate_frame(frame);
        return Err(ENOMEM as u64);
    }
    super::memory_common::flush_new_mapping(page.start_address());
    Ok(true)
}

fn populate(thread: u64, start: u64, end: u64) -> Result<(), u64> {
    let mut address = start;
    while address < end {
        // One page per PM section: a cache transition can ask us to retry,
        // but its owner must be able to acquire PM in between attempts.
        let mut guard = crate::process::manager();
        let (_, process) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ENOMEM as u64)?;
        if !covered(process, address, address + PAGE_SIZE) {
            return Err(ENOMEM as u64);
        }
        if populate_page(process, address)? {
            address += PAGE_SIZE;
        }
    }
    Ok(())
}

pub fn sys_mlock(addr: u64, length: u64) -> SyscallResult {
    result((|| {
        let (start, end) = range(addr, length, false)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        {
            let mut guard = crate::process::manager();
            let (_, p) = guard
                .as_mut()
                .and_then(|m| m.find_address_space_by_thread_mut(thread))
                .ok_or(ENOMEM as u64)?;
            check_limit(p, p.memory_locks.additional(start, end))?;
            if !covered(p, start, end) {
                return Err(ENOMEM as u64);
            }
            p.memory_locks.insert(start, end)?;
        }
        // On OOM Linux may have locked only part of the range; keep the
        // reservation until munlock rather than exposing resident pages to discard.
        populate(thread, start, end)
    })())
}

pub fn sys_munlock(addr: u64, length: u64) -> SyscallResult {
    result((|| {
        let (start, end) = range(addr, length, false)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        let mut guard = crate::process::manager();
        let (_, p) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ENOMEM as u64)?;
        if !covered(p, start, end) {
            return Err(ENOMEM as u64);
        }
        p.memory_locks.reserve_split()?;
        p.memory_locks.remove(start, end);
        Ok(())
    })())
}

pub fn sys_mlockall(flags: u64) -> SyscallResult {
    const CURRENT: u64 = 1;
    const FUTURE: u64 = 2;
    const ONFAULT: u64 = 4;
    if flags & !(CURRENT | FUTURE | ONFAULT) != 0 || flags & (CURRENT | FUTURE) == 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    result((|| {
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        let ranges = {
            let mut guard = crate::process::manager();
            let (_, p) = guard
                .as_mut()
                .and_then(|m| m.find_address_space_by_thread_mut(thread))
                .ok_or(ENOMEM as u64)?;
            let mut locks = MemoryLocks::default();
            if flags & CURRENT != 0 {
                for &(start, end) in &p.memory_locks.ranges {
                    locks.insert(start, end)?;
                }
                for vma in &p.vmas {
                    locks.insert(vma.start.as_u64(), vma.end.as_u64())?;
                }
                let mut failure = None;
                p.page_table
                    .as_deref()
                    .ok_or(ENOMEM as u64)?
                    .walk_mapped_pages(|address, _, f| {
                        if failure.is_none() && f.contains(PageTableFlags::USER_ACCESSIBLE) {
                            let start = address.as_u64();
                            failure = locks.insert(start, start + PAGE_SIZE).err();
                        }
                    })
                    .map_err(|_| ENOMEM as u64)?;
                if let Some(errno) = failure {
                    return Err(errno);
                }
                check_limit(p, locks.bytes().saturating_sub(p.memory_locks.bytes()))?;
            } else {
                check_limit(p, 0)?;
                for &(start, end) in &p.memory_locks.ranges {
                    locks.insert(start, end)?;
                }
            }
            let mut ranges = alloc::vec::Vec::new();
            if flags & CURRENT != 0 && flags & ONFAULT == 0 {
                ranges
                    .try_reserve_exact(locks.ranges.len())
                    .map_err(|_| ENOMEM as u64)?;
                ranges.extend_from_slice(&locks.ranges);
            }
            locks.future = flags & FUTURE != 0;
            locks.onfault = flags & ONFAULT != 0;
            p.memory_locks = locks;
            ranges
        };
        for (start, end) in ranges {
            populate(thread, start, end)?;
        }
        Ok(())
    })())
}

pub fn sys_munlockall() -> SyscallResult {
    result((|| {
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        let mut guard = crate::process::manager();
        let (_, p) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ENOMEM as u64)?;
        p.memory_locks.clear();
        Ok(())
    })())
}

/// Finish a new mmap's future lock after its VMA has been published. The
/// mapping is rolled back by the caller on failure, without leaving a charge.
pub(crate) fn lock_future_mapping(thread: u64, start: u64, length: u64) -> Result<(), u64> {
    let (start, end) = range(start, length, true)?;
    let eager = {
        let mut guard = crate::process::manager();
        let (_, p) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ENOMEM as u64)?;
        if !p.memory_locks.future {
            return Ok(());
        }
        check_limit(p, p.memory_locks.additional(start, end))?;
        p.memory_locks.insert(start, end)?;
        !p.memory_locks.onfault
    };
    if eager {
        populate(thread, start, end)?;
    }
    Ok(())
}

pub fn sys_madvise(addr: u64, length: u64, advice: u64) -> SyscallResult {
    if advice > 4 {
        return SyscallResult::Err(EINVAL as u64);
    }
    result((|| {
        let (start, end) = range(addr, length, true)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        let mut guard = crate::process::manager();
        let (_, p) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ENOMEM as u64)?;
        if !covered(p, start, end) {
            return Err(ENOMEM as u64);
        }
        if advice != 4 {
            return Ok(());
        }
        if p.memory_locks.overlaps(start, end) {
            return Err(EINVAL as u64);
        }
        let table = p.page_table.as_deref_mut().ok_or(ENOMEM as u64)?;
        let mut cursor = start;
        while let Some(page) = table.next_mapped_page(cursor, end) {
            cursor = page.start_address().as_u64() + PAGE_SIZE;
            if p.vmas.iter().any(|v| {
                v.contains(page.start_address())
                    && v.flags.contains(MmapFlags::ANONYMOUS)
                    && v.flags.contains(MmapFlags::PRIVATE)
            }) {
                table
                    .unmap_page_deferred(page)
                    .map_err(|_| ENOMEM as u64)?
                    .flush()
                    .release();
            }
        }
        Ok(())
    })())
}

pub fn sys_mincore(addr: u64, length: u64, vector: u64) -> SyscallResult {
    result((|| {
        let (start, end) = range(addr, length, true)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        let mut bytes = alloc::vec::Vec::new();
        bytes
            .try_reserve_exact(((end - start) / PAGE_SIZE) as usize)
            .map_err(|_| ENOMEM as u64)?;
        {
            let mut guard = crate::process::manager();
            let (_, p) = guard
                .as_mut()
                .and_then(|m| m.find_address_space_by_thread_mut(thread))
                .ok_or(ENOMEM as u64)?;
            if !covered(p, start, end) {
                return Err(ENOMEM as u64);
            }
            let table = p.page_table.as_deref().ok_or(ENOMEM as u64)?;
            let mut cursor = start;
            while cursor < end {
                let resident = table.translate(VirtAddr::new(cursor)).is_some()
                    || p.vmas
                        .iter()
                        .find(|v| v.contains(VirtAddr::new(cursor)))
                        .and_then(|v| v.backing.as_ref())
                        .is_some_and(|b| b.resident(cursor));
                bytes.push(u8::from(resident));
                cursor += PAGE_SIZE;
            }
        }
        super::userptr::write_user_bytes(vector, bytes.as_ptr(), bytes.len())
    })())
}
