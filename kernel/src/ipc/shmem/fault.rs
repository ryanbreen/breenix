//! Shm first-touch and private CoW resolution; called with PM held.
use crate::memory::anon_map::page_flags;
#[cfg(target_arch = "aarch64")]
use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
use crate::memory::file_map::{Access, FaultOutcome};
use crate::memory::vma::{MmapFlags, Protection};
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, Size4KiB},
    VirtAddr,
};
pub(crate) fn resolve_page(
    table: &mut crate::memory::process_memory::ProcessPageTable,
    vmas: &[crate::memory::vma::Vma],
    address: u64,
    access: Access,
) -> FaultOutcome {
    let Some(vma) = vmas
        .iter()
        .find(|v| v.shm.is_some() && v.contains(VirtAddr::new(address)))
    else {
        return FaultOutcome::NotFile;
    };
    if !permitted(vma.prot, access) {
        return FaultOutcome::Signal(crate::signal::constants::SIGSEGV);
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    let mapping = vma.shm.as_ref().expect("shm VMA selected");
    if mapping.object().resizing() {
        return FaultOutcome::Resolved;
    }
    let offset = vma.shm_offset + page.start_address().as_u64() - vma.start.as_u64();
    if offset / 4096 >= mapping.object().size().div_ceil(4096) {
        return FaultOutcome::Signal(crate::signal::constants::SIGBUS);
    }
    if let Some((_, flags)) = table.get_page_info(page) {
        if vma.flags.contains(MmapFlags::PRIVATE)
            && matches!(access, Access::Write)
            && crate::memory::process_memory::is_cow_page(flags)
        {
            return if table.resolve_cow_write(address, 0) {
                FaultOutcome::Resolved
            } else {
                FaultOutcome::Signal(crate::signal::constants::SIGKILL)
            };
        }
        return FaultOutcome::NotFile;
    }
    let Some(frame) = mapping.object().frame_at(offset) else {
        // First-touch backing is prepared outside PM by handle_fault/populate.
        return FaultOutcome::Resolved;
    };
    let flags = if vma.flags.contains(MmapFlags::PRIVATE) && vma.prot.contains(Protection::WRITE) {
        crate::memory::process_memory::make_cow_flags(page_flags(vma.prot))
    } else {
        page_flags(vma.prot)
    };
    let mapped = table.map_page(page, frame, flags).is_ok();
    crate::ipc::shmem::release_frame(frame);
    if !mapped {
        return FaultOutcome::Signal(crate::signal::constants::SIGKILL);
    }
    if vma.flags.contains(MmapFlags::PRIVATE)
        && matches!(access, Access::Write)
        && !table.resolve_cow_write(address, 0)
    {
        return FaultOutcome::Signal(crate::signal::constants::SIGKILL);
    }

    crate::syscall::memory_common::flush_new_mapping(page.start_address());
    FaultOutcome::Resolved
}

fn permitted(prot: Protection, access: Access) -> bool {
    match access {
        Access::Read => prot.contains(Protection::READ) || prot.contains(Protection::WRITE),
        Access::Write => prot.contains(Protection::WRITE),
        Access::Execute => prot.contains(Protection::EXEC),
    }
}
