//! Shared object mappings and the POSIX shared-memory namespace.
#[path = "shmem/fault.rs"]
mod fault;
pub(crate) use fault::resolve_page;
#[path = "shmem/namespace.rs"]
mod namespace;
#[path = "shmem/object.rs"]
mod object;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::VirtAddr;
use crate::memory::vma::{MmapFlags, Protection, Vma};
use crate::syscall::errno::{EAGAIN, EINVAL, ENOMEM, ESRCH};
use crate::syscall::memory_common::{self, PAGE_SIZE};
use alloc::sync::Arc;
pub use namespace::{access_path, mmap_fd, open_path, stat_path, unlink_path, virtual_entry};
pub(crate) use object::release_frame;
pub use object::{page_count, MappingOrigin, ShmMapping, ShmObject};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

pub fn map_object(
    obj: &Arc<ShmObject>,
    addr: u64,
    len: u64,
    prot: Protection,
    fixed: bool,
) -> Result<u64, u64> {
    let flags = MmapFlags::from_bits_truncate(
        MmapFlags::SHARED.bits() | if fixed { MmapFlags::FIXED.bits() } else { 0 },
    );
    map_range(
        obj,
        addr,
        len,
        prot,
        flags,
        0,
        MappingOrigin::SysV,
        prot.contains(Protection::WRITE),
    )
}

/// Atomically refuse an occupied fixed address, for shmat without SHM_REMAP.
/// The original map_object signature remains available to existing callers.
pub fn map_object_noreplace(
    obj: &Arc<ShmObject>,
    addr: u64,
    len: u64,
    prot: Protection,
) -> Result<u64, u64> {
    let flags =
        MmapFlags::from_bits_truncate(MmapFlags::SHARED.bits() | MmapFlags::FIXED_NOREPLACE.bits());
    map_range(
        obj,
        addr,
        len,
        prot,
        flags,
        0,
        MappingOrigin::SysV,
        prot.contains(Protection::WRITE),
    )
}

fn map_range(
    obj: &Arc<ShmObject>,
    addr: u64,
    len: u64,
    prot: Protection,
    flags: MmapFlags,
    offset: u64,
    origin: MappingOrigin,
    may_write: bool,
) -> Result<u64, u64> {
    if len == 0 || !memory_common::is_page_aligned(offset) {
        return Err(EINVAL as u64);
    }
    let len = len.checked_add(PAGE_SIZE - 1).ok_or(ENOMEM as u64)? & !(PAGE_SIZE - 1);
    offset
        .checked_add(len)
        .filter(|end| *end <= i64::MAX as u64)
        .ok_or(EINVAL as u64)?;
    let tid = memory_common::get_current_thread_id().ok_or(ESRCH as u64)?;
    // Select and publish the sparse VMA in the SAME PM section. No page
    // allocation separates placement from reservation; a racing mmap cannot
    // claim the chosen range. clear_for_mapping owns fixed-map/limit checks.
    let (start, populate) = {
        let mut guard = crate::process::manager_for_address_space(tid);
        let (_, process) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(tid))
            .ok_or(ESRCH as u64)?;
        let fixed = flags.contains(MmapFlags::FIXED) || flags.contains(MmapFlags::FIXED_NOREPLACE);
        let in_range = |start: u64| {
            memory_common::is_page_aligned(start)
                && start >= crate::memory::vma::MMAP_REGION_START
                && start
                    .checked_add(len)
                    .is_some_and(|end| end <= crate::memory::vma::MMAP_REGION_END)
        };
        if process.page_table.is_none() {
            return Err(ENOMEM as u64);
        }
        let hint = process.mmap_hint;
        let start = if fixed {
            if !in_range(addr) {
                return Err(EINVAL as u64);
            }
            addr
        } else {
            let requested = memory_common::round_down_to_page(addr);
            if in_range(requested)
                && !process
                    .vmas
                    .iter()
                    .any(|v| v.start.as_u64() < requested + len && requested < v.end.as_u64())
            {
                requested
            } else {
                memory_common::place_below_hint(process, len).ok_or(ENOMEM as u64)?
            }
        };
        if let Err(e) =
            crate::syscall::mmap::clear_for_mapping(process, start, start + len, prot, flags)
        {
            process.mmap_hint = hint;
            return Err(e);
        }
        let mut vma = Vma::new(
            VirtAddr::new(start),
            VirtAddr::new(start + len),
            prot,
            flags,
        );
        vma.shm = Some(ShmMapping::with_origin(obj, origin, may_write));
        vma.shm_offset = offset;
        process.vmas.push(vma);
        crate::syscall::memory_advice::record_future(process, start, start + len);
        (
            start,
            process.memory_locks.future && !process.memory_locks.onfault,
        )
    };
    if populate || flags.contains(MmapFlags::POPULATE) {
        // MAP_POPULATE is best effort, as for ordinary mmap; mlockall's
        // reservation is retained even if part of the mapping hits EOF/OOM.
        let _ = crate::syscall::memory_advice::populate(tid, start, start + len);
    }
    Ok(start)
}

pub fn object_at(addr: u64) -> Option<Arc<ShmObject>> {
    let tid = memory_common::get_current_thread_id()?;
    let mut guard = crate::process::manager_for_address_space(tid);
    let (_, process) = guard.as_mut()?.find_address_space_by_thread_mut(tid)?;
    process.vmas.iter().find_map(|v| {
        let m = v.shm.as_ref()?;
        m.matches_attachment(v.start.as_u64(), v.shm_offset, addr)
            .then(|| m.object().clone())
    })
}

/// Allocate backing outside PM, then callers revalidate their VMA under PM.
/// ENOMEM is allocation failure; EAGAIN means a concurrent size transition.
pub fn prepare_population(thread: u64, address: u64) -> Result<(), u64> {
    let snapshot = {
        let mut guard = crate::process::manager_for_address_space(thread);
        let (_, process) = guard
            .as_mut()
            .and_then(|m| m.find_address_space_by_thread_mut(thread))
            .ok_or(ESRCH as u64)?;
        population_snapshot(&process.vmas, address)
    };
    if let Some((object, offset, _)) = snapshot {
        match object.ensure_page(offset) {
            Err(e) if e == EAGAIN as u64 => {}
            result => result?,
        }
    }
    Ok(())
}
pub(crate) fn population_snapshot(
    vmas: &[Vma],
    address: u64,
) -> Option<(Arc<ShmObject>, u64, Protection)> {
    let v = vmas
        .iter()
        .find(|v| v.shm.is_some() && v.contains(VirtAddr::new(address)))?;
    let m = v.shm.as_ref()?;
    Some((
        m.object().clone(),
        v.shm_offset + memory_common::round_down_to_page(address) - v.start.as_u64(),
        v.prot,
    ))
}

/// Invalidate an object's resident pages beyond the new rounded EOF,
/// including MAP_PRIVATE copies. VMAs retain their offsets and size so later
/// touches get SIGBUS and regrowth faults onto the new canonical object page.
fn invalidate_object(object: &ShmObject, len: u64) -> Result<(), u64> {
    let first = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
    loop {
        let mut guard = crate::process::manager();
        if crate::memory::fork_copy::any_forming() {
            drop(guard);
            crate::task::scheduler::yield_current();
            continue;
        }
        let Some(manager) = guard.as_mut() else {
            return Ok(());
        };
        let mut removed = 0;
        'processes: for (_, process) in manager.iter_processes_mut() {
            let Some(table) = process.page_table.as_deref_mut() else {
                continue;
            };
            for vma in &process.vmas {
                if !vma
                    .shm
                    .as_ref()
                    .is_some_and(|m| core::ptr::eq(m.object().as_ref(), object))
                {
                    continue;
                }
                let mut cursor = vma
                    .start
                    .as_u64()
                    .saturating_add(first.saturating_sub(vma.shm_offset));
                while let Some(page) = table.next_mapped_page(cursor, vma.end.as_u64()) {
                    cursor = page.start_address().as_u64() + PAGE_SIZE;
                    table
                        .unmap_page_deferred(page)
                        .map_err(|_| ENOMEM as u64)?
                        .flush()
                        .release();
                    removed += 1;
                    if removed == 64 {
                        break 'processes;
                    }
                }
            }
        }
        if removed < 64 {
            return Ok(());
        }
        // First-touch publication is blocked by object.resizing. Rescan live
        // VMAs after releasing PM so a newly published fork is included.
        drop(guard);
        crate::task::scheduler::yield_current();
    }
}

/// Maximum mapping rights are captured at mmap/shmat, independent of later chmod.
pub fn mprotect_allowed(vma: &Vma, prot: Protection) -> bool {
    !prot.contains(Protection::WRITE) || vma.shm.as_ref().is_none_or(|m| m.may_write())
}
/// Population runs under PM, after prepare_population allocated outside PM.
pub fn populate_page(
    process: &mut crate::process::Process,
    address: u64,
) -> Option<Result<bool, u64>> {
    if !process
        .vmas
        .iter()
        .any(|v| v.shm.is_some() && v.contains(VirtAddr::new(address)))
    {
        return None;
    }
    Some((|| {
        use crate::memory::file_map::{Access, FaultOutcome};
        let table = process.page_table.as_deref_mut().ok_or(ENOMEM as u64)?;
        let vma = process
            .vmas
            .iter()
            .find(|v| v.contains(VirtAddr::new(address)))
            .ok_or(ENOMEM as u64)?;
        if vma.prot == Protection::NONE {
            return Err(ENOMEM as u64);
        }
        let access = if vma.prot.contains(Protection::WRITE) {
            Access::Write
        } else if vma.prot.contains(Protection::READ) {
            Access::Read
        } else {
            Access::Execute
        };
        match resolve_page(table, &process.vmas, address, access) {
            FaultOutcome::Resolved | FaultOutcome::NotFile => {
                Ok(table.translate(VirtAddr::new(address)).is_some())
            }
            _ => Err(ENOMEM as u64),
        }
    })())
}

/// Preserve lazy private CoW on a WRITE upgrade; removing WRITE also removes
/// the CoW marker, which generic fault handlers interpret as permission to write.
/// true tells mprotect that this page's flags are handled here.
pub fn protect_page(
    table: &mut crate::memory::process_memory::ProcessPageTable,
    vma: &Vma,
    address: u64,
    prot: Protection,
) -> Result<bool, u64> {
    if vma.shm.is_none() || !vma.flags.contains(MmapFlags::PRIVATE) {
        return Ok(false);
    }
    #[cfg(target_arch = "aarch64")]
    use crate::memory::arch_stub::{Page, Size4KiB};
    #[cfg(target_arch = "x86_64")]
    use x86_64::structures::paging::{Page, Size4KiB};
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    let Some((frame, _)) = table.get_page_info(page) else {
        return Ok(true);
    };
    let mut flags = crate::memory::anon_map::page_flags(prot);
    if prot.contains(Protection::WRITE) && crate::memory::frame_metadata::frame_is_shared(frame) {
        flags = crate::memory::process_memory::make_cow_flags(flags);
    }
    table
        .update_page_flags(page, flags)
        .map_err(|_| ENOMEM as u64)?;
    Ok(true)
}
