//! Shared x86 kernel address space.
//!
//! The boot, master and process PML4 tables point each upper-half slot at the
//! same boot-owned PDPT for that slot. Mapping changes stay below PML4.

use crate::memory::frame_allocator::allocate_frame;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::{
    instructions::tlb,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{PageTable, PageTableFlags, PhysFrame},
    PhysAddr, VirtAddr,
};

#[path = "x86_kernel_half.rs"]
mod x86_kernel_half;

/// The master kernel PML4 frame (Phase 2), published as a physical address in a
/// lock-free cell rather than as a `Mutex<Option<PhysFrame>>`.
///
/// It is written exactly once, by `build_master_kernel_pml4()` during
/// `memory::init()`, and is read-only for the rest of the boot.
///
/// It has to be lock-free because one of its readers is the context-switch
/// path: `interrupts::context_switch::setup_kernel_thread_return` calls
/// `process_memory::switch_to_kernel_page_table`, which reads this value with
/// interrupts disabled, while `map_kernel_page`/`unmap_kernel_page` read it
/// from ordinary thread context with interrupts enabled. Behind a spin lock
/// those two readers deadlock a single-CPU machine: a timer preemption inside
/// the thread-context reader leaves the lock held, and the dispatch that same
/// interrupt performs then spins on it forever with IF=0, so no later
/// interrupt can run to resume the holder.
/// claim-lint:ok: that deadlock is the GDB specimen recorded in
/// docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md.
///
/// `0` means "not built yet". The frame this cell holds comes from
/// `allocate_frame()` in `build_master_kernel_pml4()`, and the boot frame
/// allocator does not hand out physical address 0, so 0 is unambiguous as the
/// sentinel.
/// claim-lint:ok: 1 of 1 store to this cell is the one in
/// `build_master_kernel_pml4()` below, pinned by
/// tests/dispatch_path_lock_free_structure.rs.
static MASTER_KERNEL_PML4_PHYS: AtomicU64 = AtomicU64::new(0);

fn table_ptr(phys_mem_offset: VirtAddr, frame: PhysFrame) -> *mut PageTable {
    (phys_mem_offset + frame.start_address().as_u64()).as_mut_ptr()
}

/// Install a PDPT for every kernel slot before the master and process roots
/// copy the boot PML4. The boot direct map remains usable through the migration.
pub fn init(phys_mem_offset: VirtAddr) {
    let (boot_frame, _) = Cr3::read();
    // SAFETY: only the boot CPU runs here, and the frame allocator returns
    // distinct, unused frames covered by the boot direct map.
    unsafe {
        let boot = &mut *table_ptr(phys_mem_offset, boot_frame);
        x86_kernel_half::migrate_kernel_half(boot, phys_mem_offset, || {
            allocate_frame().expect("Failed to allocate frame for kernel PDPT")
        });
    }
    log::info!("Global kernel page table initialized successfully");
}

/// The table and newly allocated frame must be mapped by the physical offset.
/// The caller must serialize creation of children in the same entry.
unsafe fn ensure_child_table(
    table: &mut PageTable,
    index: usize,
    phys_mem_offset: VirtAddr,
) -> Result<PhysFrame, &'static str> {
    let entry = &mut table[index];
    if !entry.is_unused() {
        return entry
            .frame()
            .map_err(|_| "Kernel mapping needs a child table");
    }

    let frame = allocate_frame().ok_or("Out of memory for kernel page table")?;
    (*table_ptr(phys_mem_offset, frame)).zero();
    entry.set_frame(frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
    Ok(frame)
}

/// Map a page in the global kernel address space
///
/// This function maps a virtual page to a physical frame in the shared kernel
/// page tables. The mapping becomes immediately visible to all processes.
///
/// # Safety
/// The virtual address must be in kernel space. Callers must serialize table
/// creation for mappings that can share an unpopulated intermediate entry.
pub unsafe fn map_kernel_page(
    virt: VirtAddr,
    phys: PhysAddr,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    // Verify this is a kernel address
    if virt.as_u64() < 0xFFFF_8000_0000_0000 {
        return Err("map_kernel_page called with non-kernel address");
    }

    let pml4_frame = master_kernel_pml4().ok_or("Master kernel PML4 not initialized")?;
    let phys_mem_offset = crate::memory::physical_memory_offset();
    let pml4 = &*table_ptr(phys_mem_offset, pml4_frame);
    let pdpt_frame = pml4[virt.p4_index()]
        .frame()
        .map_err(|_| "Kernel PDPT not installed")?;

    let pdpt_index = usize::from(virt.p3_index());
    let pd_index = usize::from(virt.p2_index());
    let pt_index = usize::from(virt.p1_index());

    let pdpt = &mut *table_ptr(phys_mem_offset, pdpt_frame);

    let pd_frame = ensure_child_table(pdpt, pdpt_index, phys_mem_offset)?;
    let pd = &mut *table_ptr(phys_mem_offset, pd_frame);
    let pt_frame = ensure_child_table(pd, pd_index, phys_mem_offset)?;
    let pt = &mut *table_ptr(phys_mem_offset, pt_frame);

    if crate::memory::kernel_stack::is_kernel_stack_va(virt.as_u64())
        && pt[pt_index].flags().contains(PageTableFlags::PRESENT)
    {
        crate::memory::kernel_stack::note_kernel_stack_pte_overwrite_refused();
        return Err("map_kernel_page: refusing to overwrite a present kernel-stack PTE");
    }

    // Map the page
    let page_frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
    pt[pt_index].set_frame(page_frame, flags);

    tlb::flush(virt);
    Ok(())
}

/// Remove a page from the global kernel address space.
///
/// Intermediate page-table frames remain installed so a later occupant can
/// reuse the hierarchy without allocating it again.
///
/// The translation is not flushed here: the caller flushes once on every CPU
/// (`tlb::flush_all`) after its last unmap and before it releases any frame
/// this returned, so a range costs one shootdown rather than one per page.
///
/// # Safety
/// Caller must ensure the virtual address is in kernel space and that removing
/// the mapping cannot race an architectural user of the page, and must flush
/// as above before releasing the returned frame.
pub unsafe fn unmap_kernel_page(virt: VirtAddr) -> Result<Option<PhysFrame>, &'static str> {
    if virt.as_u64() < 0xFFFF_8000_0000_0000 {
        return Err("unmap_kernel_page called with non-kernel address");
    }

    let pml4_frame = master_kernel_pml4().ok_or("Master kernel PML4 not initialized")?;
    let phys_mem_offset = crate::memory::physical_memory_offset();
    let pml4 = &*table_ptr(phys_mem_offset, pml4_frame);
    let pdpt_frame = pml4[virt.p4_index()]
        .frame()
        .map_err(|_| "Kernel PDPT not installed")?;

    let pdpt_index = usize::from(virt.p3_index());
    let pd_index = usize::from(virt.p2_index());
    let pt_index = usize::from(virt.p1_index());

    let pdpt = &mut *table_ptr(phys_mem_offset, pdpt_frame);
    if pdpt[pdpt_index].is_unused() {
        return Ok(None);
    }
    let pd_frame = pdpt[pdpt_index]
        .frame()
        .map_err(|_| "Cannot unmap a page through a non-table PDPTE")?;

    let pd = &mut *table_ptr(phys_mem_offset, pd_frame);
    if pd[pd_index].is_unused() {
        return Ok(None);
    }
    let pt_frame = pd[pd_index]
        .frame()
        .map_err(|_| "Cannot unmap a page through a non-table PDE")?;

    let pt = &mut *table_ptr(phys_mem_offset, pt_frame);
    let entry = &mut pt[pt_index];
    if entry.is_unused() {
        return Ok(None);
    }

    let frame = PhysFrame::containing_address(entry.addr());
    entry.set_unused();

    Ok(Some(frame))
}

/// Copy the boot root and switch to the permanent kernel root. Process roots
/// inherit its kernel half before they can be scheduled.
pub fn build_master_kernel_pml4() {
    let phys_mem_offset = crate::memory::physical_memory_offset();
    let (boot_frame, _) = Cr3::read();
    let master_pml4_frame = allocate_frame().expect("Failed to allocate master PML4");

    // SAFETY: the boot CPU is still the only CPU accessing these tables.
    unsafe {
        let boot = table_ptr(phys_mem_offset, boot_frame);
        let master = table_ptr(phys_mem_offset, master_pml4_frame);
        core::ptr::copy_nonoverlapping(boot, master, 1);
        let master = &*master;

        // Prebuild the existing 128 MiB kernel-stack and 16 MiB IST-stack
        // ranges so their runtime mappers never race to install a PD or PT.
        // Use their boot-owned PDPTs and leave any existing entries intact.
        for (slot, pt_count) in [(402, 64), (403, 8)] {
            let pdpt_frame = master[slot].frame().expect("Stack PDPT not installed");
            let pdpt = &mut *table_ptr(phys_mem_offset, pdpt_frame);
            let pd_frame = ensure_child_table(pdpt, 0, phys_mem_offset)
                .expect("Failed to prepare the stack PD");
            let pd = &mut *table_ptr(phys_mem_offset, pd_frame);
            for index in 0..pt_count {
                ensure_child_table(pd, index, phys_mem_offset)
                    .expect("Failed to prepare the stack PT");
            }
        }
    }

    // Publish only after the child tables are ready. The context-switch
    // accessor pairs its Acquire load with this Release store.
    MASTER_KERNEL_PML4_PHYS.store(
        master_pml4_frame.start_address().as_u64(),
        Ordering::Release,
    );

    crate::memory::tlb::note_root_load(master_pml4_frame.start_address().as_u64());
    unsafe {
        Cr3::write(master_pml4_frame, Cr3Flags::empty());
        tlb::flush_all();
    }
    log::info!("CR3 switched to master PML4");
}

/// Get the master kernel PML4 frame for process creation (Phase 2)
///
/// IMPORTANT: This function is called from the context switch path (via
/// switch_to_kernel_page_table), so it must not do any logging, and it must
/// not take a lock. Logging from interrupt context can deadlock on the logger
/// lock; a lock here deadlocks on the ordinary thread-context readers, which
/// hold it with interrupts enabled. One relaxed-cost Acquire load, no lock, no
/// allocation, no formatting.
pub fn master_kernel_pml4() -> Option<PhysFrame> {
    let phys = MASTER_KERNEL_PML4_PHYS.load(Ordering::Acquire);
    if phys == 0 {
        None
    } else {
        Some(PhysFrame::containing_address(PhysAddr::new(phys)))
    }
}
