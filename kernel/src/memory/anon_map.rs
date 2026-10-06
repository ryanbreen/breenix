//! First-touch backing for private anonymous VMAs, including kernel user copies.
#[cfg(target_arch = "aarch64")]
use super::arch_stub::{Page, PageTableFlags, Size4KiB, VirtAddr};
use super::file_map::{Access, FaultOutcome};
use super::vma::{MmapFlags, Protection};
use crate::process::Process;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, Size4KiB},
    VirtAddr,
};

pub(crate) fn resolve_fault(process: &mut Process, address: u64, access: Access) -> FaultOutcome {
    let Some(vma) = process.vmas.iter().find(|v| {
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
    let Some(table) = process.page_table.as_mut() else {
        return FaultOutcome::NotFile;
    };
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    if table.translate(page.start_address()).is_some() {
        return FaultOutcome::NotFile;
    }
    let Some(frame) = super::frame_allocator::allocate_frame() else {
        return FaultOutcome::Signal(crate::signal::constants::SIGSEGV);
    };
    let offset = super::physical_memory_offset();
    // SAFETY: the new, exclusively owned frame is reachable via the direct map.
    unsafe {
        core::ptr::write_bytes(
            (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
            0,
            4096,
        );
    }
    if table.map_page(page, frame, flags).is_err() {
        let _ = super::frame_allocator::deallocate_leaf_frame(frame);
        return FaultOutcome::Signal(crate::signal::constants::SIGSEGV);
    }
    crate::syscall::memory_common::flush_tlb(page.start_address());
    FaultOutcome::Resolved
}

/// Install the VMA's execute permission as well as its write permission.
pub(crate) fn page_flags(prot: Protection) -> PageTableFlags {
    let mut flags = crate::syscall::memory_common::prot_to_page_flags(prot);
    if !prot.contains(Protection::EXEC) {
        flags.insert(PageTableFlags::NO_EXECUTE);
    }
    flags
}
