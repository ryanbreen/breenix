//! Memory residency, locking and advice for the CLONE_VM address-space owner.
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
        .ok_or(EINVAL as u64)?;
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
    let raw_limit = process.limits.get(crate::process::limits::MEMLOCK).soft;
    if raw_limit == 0 {
        return Err(EPERM as u64);
    }
    if process.memory_locks.bytes().saturating_add(additional) > (raw_limit & !(PAGE_SIZE - 1)) {
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
            .insert_reserved(start, end)
            .expect("reserved memory lock interval");
    }
}

// Own a prepared frame until publication or a racing mapping makes it redundant.
struct OwnedPopulationFrame {
    #[cfg(target_arch = "aarch64")]
    frame: crate::memory::arch_stub::PhysFrame<Size4KiB>,
    #[cfg(target_arch = "x86_64")]
    frame: x86_64::structures::paging::PhysFrame<Size4KiB>,
}

impl OwnedPopulationFrame {
    fn new() -> Option<Self> {
        let frame = crate::memory::frame_allocator::allocate_frame()?;
        let offset = crate::memory::physical_memory_offset().as_u64();
        // SAFETY: an exclusively owned frame through the kernel direct map.
        unsafe {
            core::ptr::write_bytes(
                (offset + frame.start_address().as_u64()) as *mut u8,
                0,
                PAGE_SIZE as usize,
            );
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        }
        Some(Self { frame })
    }

    fn publish(self) {
        core::mem::forget(self);
    }
}

impl Drop for OwnedPopulationFrame {
    fn drop(&mut self) {
        crate::memory::frame_allocator::deallocate_frame(self.frame);
    }
}

enum Population {
    Resident,
    Retry,
    Anonymous,
}

// The anonymous leaf frame is prepared outside PM and revalidated on publication.
fn populate_page(
    process: &mut Process,
    address: u64,
    prepared: &mut Option<OwnedPopulationFrame>,
) -> Result<Population, u64> {
    let table = process.page_table.as_deref_mut().ok_or(ENOMEM as u64)?;
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    let vma = process
        .vmas
        .iter()
        .find(|v| v.contains(page.start_address()));
    let private_write = vma.is_some_and(|v| {
        v.flags.contains(MmapFlags::PRIVATE)
            && v.prot.contains(crate::memory::vma::Protection::WRITE)
    });
    if table.translate(page.start_address()).is_some() {
        if private_write
            && table
                .get_page_info(page)
                .is_some_and(|(_, f)| crate::memory::process_memory::is_cow_page(f))
            && !table.resolve_cow_write(address, process.id.as_u64())
        {
            return Err(ENOMEM as u64);
        }
        return Ok(Population::Resident);
    }
    let vma = vma.ok_or(ENOMEM as u64)?;
    if vma.backing.is_some() {
        use crate::memory::file_map::{Access, FaultOutcome};
        let access = if vma.prot.contains(crate::memory::vma::Protection::WRITE) {
            Access::Write
        } else if vma.prot.contains(crate::memory::vma::Protection::READ) {
            Access::Read
        } else {
            Access::Execute
        };
        if !matches!(
            crate::memory::file_map::resolve_page(table, &process.vmas, address, access),
            FaultOutcome::Resolved
        ) {
            return Err(ENOMEM as u64);
        }
        if table.translate(page.start_address()).is_none() {
            return Ok(Population::Retry);
        }
        if private_write
            && table
                .get_page_info(page)
                .is_some_and(|(_, f)| crate::memory::process_memory::is_cow_page(f))
            && !table.resolve_cow_write(address, process.id.as_u64())
        {
            return Err(ENOMEM as u64);
        }
        return Ok(Population::Resident);
    }
    let Some(frame) = prepared.as_ref() else {
        return Ok(Population::Anonymous);
    };
    table
        .map_page(
            page,
            frame.frame,
            crate::memory::anon_map::page_flags(vma.prot),
        )
        .map_err(|_| ENOMEM as u64)?;
    prepared.take().expect("prepared anonymous frame").publish();
    super::memory_common::flush_new_mapping(page.start_address());
    Ok(Population::Resident)
}

pub(crate) fn populate(thread: u64, start: u64, end: u64) -> Result<(), u64> {
    let mut address = start;
    while address < end {
        let mut prepared = None;
        loop {
            let outcome = {
                let mut guard = crate::process::manager();
                let (_, process) = guard
                    .as_mut()
                    .and_then(|m| m.find_address_space_by_thread_mut(thread))
                    .ok_or(ENOMEM as u64)?;
                if !covered(process, address, address + PAGE_SIZE) {
                    return Err(ENOMEM as u64);
                }
                populate_page(process, address, &mut prepared)?
            };
            match outcome {
                Population::Resident => break,
                Population::Anonymous => {
                    prepared = Some(OwnedPopulationFrame::new().ok_or(ENOMEM as u64)?);
                }
                Population::Retry => {
                    crate::task::scheduler::yield_current();
                    crate::arch_halt_with_interrupts();
                }
            }
        }
        address += PAGE_SIZE;
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
            locks.reserve_split()?;
            locks.future = flags & FUTURE != 0;
            locks.onfault = flags & ONFAULT != 0;
            p.memory_locks = locks;
            ranges
        };
        for (start, end) in ranges {
            // mlockall installs policy even when a mapping cannot be populated.
            let _ = populate(thread, start, end);
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

/// Charge at VMA publication, under its existing PM section.
pub(crate) fn publish_future(process: &mut Process, start: u64, end: u64) -> Result<bool, u64> {
    prepare_future(process, process.memory_locks.additional(start, end))
        .map_err(|_| super::errno::EAGAIN as u64)?;
    record_future(process, start, end);
    Ok(process.memory_locks.future && !process.memory_locks.onfault)
}

pub fn sys_madvise(addr: u64, length: u64, advice: u64) -> SyscallResult {
    if !matches!(advice, 0..=4 | 8..=11) {
        return SyscallResult::Err(EINVAL as u64);
    }
    result((|| {
        let (start, end) = range(addr, length, true)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        // Validate and discard one page per PM section.
        let mut cursor = start;
        while cursor < end {
            let mut guard = crate::process::manager();
            let (_, p) = guard
                .as_mut()
                .and_then(|m| m.find_address_space_by_thread_mut(thread))
                .ok_or(ENOMEM as u64)?;
            if !covered(p, cursor, cursor + PAGE_SIZE) {
                return Err(ENOMEM as u64);
            }
            if advice == 4 {
                if p.memory_locks.overlaps(cursor, cursor + PAGE_SIZE) {
                    return Err(EINVAL as u64);
                }
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(cursor));
                let vma = p.vmas.iter().find(|v| v.contains(page.start_address()));
                let anonymous = vma.is_some_and(|v| {
                    v.flags.contains(MmapFlags::ANONYMOUS) && v.flags.contains(MmapFlags::PRIVATE)
                });
                let non_vma = vma.is_none();
                let table = p.page_table.as_deref_mut().ok_or(ENOMEM as u64)?;
                if anonymous && table.get_page_info(page).is_some() {
                    table
                        .unmap_page_deferred(page)
                        .map_err(|_| ENOMEM as u64)?
                        .flush()
                        .release();
                } else if non_vma {
                    // Heap, stack and writable ELF leaves lack faultable VMAs.
                    // Keep their mapping and supply zero bytes in private custody.
                    if let Some((_, flags)) = table.get_page_info(page) {
                        if flags.contains(PageTableFlags::WRITABLE)
                            || crate::memory::process_memory::is_cow_page(flags)
                        {
                            if crate::memory::process_memory::is_cow_page(flags)
                                && !table.resolve_cow_write(cursor, p.id.as_u64())
                            {
                                return Err(ENOMEM as u64);
                            }
                            let (frame, _) = table.get_page_info(page).ok_or(ENOMEM as u64)?;
                            let offset = crate::memory::physical_memory_offset().as_u64();
                            // SAFETY: the leaf has exclusive writable custody under PM.
                            unsafe {
                                core::ptr::write_bytes(
                                    (offset + frame.start_address().as_u64()) as *mut u8,
                                    0,
                                    PAGE_SIZE as usize,
                                );
                            }
                        }
                    }
                }
            }
            cursor += PAGE_SIZE;
        }
        Ok(())
    })())
}

pub fn sys_mincore(addr: u64, length: u64, vector: u64) -> SyscallResult {
    result((|| {
        let (start, end) = range(addr, length, true)?;
        let thread = get_current_thread_id().ok_or(ENOMEM as u64)?;
        // One output page on the stack; each query holds PM for one input page.
        let mut bytes = [0u8; PAGE_SIZE as usize];
        let mut cursor = start;
        let mut written = 0u64;
        while cursor < end {
            let count = ((end - cursor) / PAGE_SIZE).min(PAGE_SIZE) as usize;
            for byte in &mut bytes[..count] {
                let mut guard = crate::process::manager();
                let (_, p) = guard
                    .as_mut()
                    .and_then(|m| m.find_address_space_by_thread_mut(thread))
                    .ok_or(ENOMEM as u64)?;
                if !covered(p, cursor, cursor + PAGE_SIZE) {
                    return Err(ENOMEM as u64);
                }
                let table = p.page_table.as_deref().ok_or(ENOMEM as u64)?;
                let resident = table.translate(VirtAddr::new(cursor)).is_some()
                    || p.vmas
                        .iter()
                        .find(|v| v.contains(VirtAddr::new(cursor)))
                        .and_then(|v| v.backing.as_ref())
                        .is_some_and(|b| b.resident(cursor));
                *byte = u8::from(resident);
                cursor += PAGE_SIZE;
            }
            let output = vector
                .checked_add(written)
                .ok_or(super::errno::EFAULT as u64)?;
            super::userptr::write_user_bytes(output, bytes.as_ptr(), count)?;
            written += count as u64;
        }
        Ok(())
    })())
}
