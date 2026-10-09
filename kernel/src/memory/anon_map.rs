//! First-touch backing for private anonymous VMAs, including kernel user copies.
#[cfg(target_arch = "aarch64")]
use super::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};
use super::file_map::{Access, FaultOutcome};
use super::vma::{MmapFlags, Protection};
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

/// Allocate and zero outside PM, then revalidate the reservation before publishing.
pub(crate) fn handle_fault(
    root: u64,
    address: u64,
    access: Access,
    user_thread: Option<u64>,
) -> FaultOutcome {
    if crate::process::process_manager_held_on_current_cpu() {
        return FaultOutcome::NotFile;
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    let snapshot = {
        let guard = crate::process::manager();
        let Some((pid, owner)) = guard.as_ref().and_then(|m| m.find_process_by_cr3(root)) else {
            return FaultOutcome::NotFile;
        };
        let Some(prot) = anonymous_protection(&owner.vmas, address) else {
            return FaultOutcome::NotFile;
        };
        let Some(table) = owner.page_table.as_deref() else {
            return FaultOutcome::NotFile;
        };
        if table.translate(page.start_address()).is_some() && permitted(prot, access) {
            return FaultOutcome::NotFile;
        }
        (pid, table.address_space(), prot)
    };
    let frame = if permitted(snapshot.2, access) {
        zeroed_frame()
    } else {
        None
    };
    let outcome = {
        let mut guard = crate::process::manager();
        let Some(manager) = guard.as_mut() else {
            if let Some(frame) = frame {
                let _ = super::frame_allocator::deallocate_leaf_frame(frame);
            }
            return FaultOutcome::NotFile;
        };
        let outcome = match manager.get_process_mut(snapshot.0) {
            Some(owner) => {
                let prot = anonymous_protection(&owner.vmas, address);
                match owner.page_table.as_deref_mut() {
                    Some(table) if table.address_space() == snapshot.1 => match prot {
                        Some(prot) if !permitted(prot, access) => {
                            FaultOutcome::Signal(crate::signal::constants::SIGSEGV)
                        }
                        Some(prot) => {
                            if table.translate(page.start_address()).is_some() {
                                FaultOutcome::Resolved
                            } else if let Some(frame) = frame {
                                if table.map_page(page, frame, page_flags(prot)).is_ok() {
                                    crate::syscall::memory_common::flush_new_mapping(
                                        page.start_address(),
                                    );
                                    return FaultOutcome::Resolved;
                                }
                                FaultOutcome::Signal(crate::signal::constants::SIGKILL)
                            } else {
                                FaultOutcome::Signal(crate::signal::constants::SIGKILL)
                            }
                        }
                        None => FaultOutcome::NotFile,
                    },
                    _ => FaultOutcome::NotFile,
                }
            }
            None => FaultOutcome::NotFile,
        };
        if let (FaultOutcome::Signal(signal), Some(tid)) = (outcome, user_thread) {
            if let Some((_, process)) = manager.find_process_by_thread_mut(tid) {
                // SIGSEGV is a mapping that forbids the access; SIGKILL is no
                // memory to back it.
                let info = if signal == crate::signal::constants::SIGSEGV {
                    crate::signal::types::SigInfo::fault(
                        crate::signal::constants::SEGV_ACCERR,
                        address,
                    )
                } else {
                    crate::signal::types::SigInfo::kernel()
                };
                process.signals.force_signal(signal, info);
            }
        }
        outcome
    };
    if let Some(frame) = frame {
        let _ = super::frame_allocator::deallocate_leaf_frame(frame);
    }
    outcome
}

fn anonymous_protection(vmas: &[super::vma::Vma], address: u64) -> Option<Protection> {
    vmas.iter()
        .find(|v| {
            v.contains(VirtAddr::new(address))
                && v.flags.contains(MmapFlags::ANONYMOUS)
                && v.flags.contains(MmapFlags::PRIVATE)
        })
        .map(|v| v.prot)
}

fn permitted(prot: Protection, access: Access) -> bool {
    match access {
        Access::Read => prot.contains(Protection::READ) || prot.contains(Protection::WRITE),
        Access::Write => prot.contains(Protection::WRITE),
        Access::Execute => prot.contains(Protection::EXEC),
    }
}

fn zeroed_frame() -> Option<PhysFrame<Size4KiB>> {
    let frame = super::frame_allocator::allocate_frame()?;
    let offset = super::physical_memory_offset();
    // SAFETY: the frame is exclusively owned and accessible through the direct map.
    unsafe {
        core::ptr::write_bytes(
            (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
            0,
            4096,
        );
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
    }
    Some(frame)
}

fn resolve_page(
    table: &mut super::process_memory::ProcessPageTable,
    vmas: &[super::vma::Vma],
    address: u64,
    access: Access,
) -> FaultOutcome {
    let Some(vma) = vmas.iter().find(|v| {
        v.contains(VirtAddr::new(address))
            && v.flags.contains(MmapFlags::ANONYMOUS)
            && v.flags.contains(MmapFlags::PRIVATE)
    }) else {
        return FaultOutcome::NotFile;
    };
    let permitted = match access {
        Access::Read => vma.prot.contains(Protection::READ) || vma.prot.contains(Protection::WRITE),
        Access::Write => vma.prot.contains(Protection::WRITE),
        Access::Execute => vma.prot.contains(Protection::EXEC),
    };
    if !permitted {
        return FaultOutcome::Signal(crate::signal::constants::SIGSEGV);
    }
    let flags = page_flags(vma.prot);
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    if table.translate(page.start_address()).is_some() {
        return FaultOutcome::NotFile;
    }
    let Some(frame) = zeroed_frame() else {
        return FaultOutcome::Signal(crate::signal::constants::SIGKILL);
    };
    if table.map_page(page, frame, flags).is_err() {
        let _ = super::frame_allocator::deallocate_leaf_frame(frame);
        return FaultOutcome::Signal(crate::signal::constants::SIGKILL);
    }
    crate::syscall::memory_common::flush_new_mapping(page.start_address());
    FaultOutcome::Resolved
}

/// Install the VMA's execute permission as well as its write permission.
pub(crate) fn page_flags(prot: Protection) -> PageTableFlags {
    let mut flags = crate::syscall::memory_common::prot_to_page_flags(prot);
    if prot == Protection::NONE {
        flags.remove(PageTableFlags::USER_ACCESSIBLE);
    }
    if !prot.contains(Protection::EXEC) {
        flags.insert(PageTableFlags::NO_EXECUTE);
    }
    flags
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrepareWriteError {
    Fault,
    /// No leaf yet: defer until the cache/size transition can make progress.
    Retry,
}

/// A signal frame is copied through the direct map and cannot take a user fault.
/// Back missing anonymous or file pages before the permission-checked copy,
/// which resolves resident CoW pages through the same owned table. Neither
/// preparation nor copying touches a user VA with PROCESS_MANAGER held.
pub(crate) fn prepare_write(
    table: &mut super::process_memory::ProcessPageTable,
    vmas: &[super::vma::Vma],
    start: u64,
    length: usize,
) -> Result<(), PrepareWriteError> {
    if !crate::memory::layout::is_valid_user_range(start, length) {
        return Err(PrepareWriteError::Fault);
    }
    let Some(end) = start.checked_add(length as u64) else {
        return Err(PrepareWriteError::Fault);
    };
    let mut address = start & !4095;
    while address < end {
        if table.translate(VirtAddr::new(address)).is_none() {
            let outcome = match resolve_page(table, vmas, address, Access::Write) {
                FaultOutcome::NotFile => {
                    super::file_map::resolve_page(table, vmas, address, Access::Write)
                }
                outcome => outcome,
            };
            if !matches!(outcome, FaultOutcome::Resolved) {
                return Err(PrepareWriteError::Fault);
            }
            // A file-size transition may request a retry without installing a
            // leaf. Release PM by deferring delivery; do not copy or kill the
            // process, and do not spin while the transition needs that lock.
            if table.translate(VirtAddr::new(address)).is_none() {
                return Err(PrepareWriteError::Retry);
            }
        }
        address += 4096;
    }
    Ok(())
}
