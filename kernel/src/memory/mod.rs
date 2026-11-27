#[cfg(not(target_arch = "x86_64"))]
pub mod arch_stub;
pub mod anon_map;
pub mod cow_stats;
pub mod file_map;
pub mod fork_copy;
pub mod frame_allocator;
pub mod frame_metadata;
pub mod heap;
pub mod kernel_page_table;
pub mod kernel_walk;
pub mod kernel_stack;
pub mod layout;
pub mod paging;
pub mod per_cpu_stack;
pub mod process_memory;
pub mod slab;
pub mod stack;
pub(crate) mod stack_growth;
pub mod table_reclaim;
pub mod tlb;
pub mod vma;
pub mod locked;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{
    Mapper, Page, PageTableFlags, PhysAddr, PhysFrame, Size4KiB, VirtAddr,
};
use bootloader_api::info::MemoryRegions;
use conquer_once::spin::OnceCell;
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, Page, PageTableFlags, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::{PhysAddr, VirtAddr};

/// Global physical memory offset for use throughout the kernel
static PHYSICAL_MEMORY_OFFSET: OnceCell<VirtAddr> = OnceCell::uninit();

/// Initialize the physical memory offset for ARM64 (high-half direct map)
/// This must be called before any memory operations that need phys<->virt conversion
#[cfg(target_arch = "aarch64")]
pub fn init_physical_memory_offset_aarch64() {
    // ARM64 uses a higher-half direct map (HHDM)
    let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
    PHYSICAL_MEMORY_OFFSET.init_once(|| VirtAddr::new(hhdm_base));
    log::info!(
        "ARM64 physical memory offset initialized (HHDM at {:#x})",
        hhdm_base
    );
}

/// Initialize the ARM64 kernel heap (mapped virtual pages + global allocator)
#[cfg(target_arch = "aarch64")]
pub fn init_aarch64_heap() {
    log::info!("ARM64: Initializing heap allocator...");
    let phys_offset = physical_memory_offset();
    let mapper = unsafe { paging::init(phys_offset) };
    heap::init(&mapper).expect("ARM64 heap initialization failed");
    slab::init();
}

/// Next available MMIO virtual address
#[allow(dead_code)] // Used by map_mmio for device driver MMIO mappings
static MMIO_NEXT_ADDR: Mutex<u64> = Mutex::new(layout::MMIO_BASE);

/// Initialize the memory subsystem
pub fn init(physical_memory_offset: VirtAddr, memory_regions: &'static MemoryRegions) {
    log::info!("Initializing memory management...");
    log::info!("Physical memory offset: {:?}", physical_memory_offset);

    // Store the physical memory offset globally
    PHYSICAL_MEMORY_OFFSET.init_once(|| physical_memory_offset);

    // === STEP 1: Log canonical kernel layout ===
    log::info!("STEP 1: Establishing canonical kernel layout...");
    layout::log_layout();

    // Initialize frame allocator
    log::info!("Initializing frame allocator...");
    frame_allocator::init(memory_regions);

    // Initialize paging
    log::info!("Initializing paging...");
    let _mapper = unsafe { paging::init(physical_memory_offset) };

    // Save the kernel page table for later switching
    process_memory::init_kernel_page_table();

    // Initialize global kernel page table system
    log::info!("Initializing global kernel page tables...");
    kernel_page_table::init(physical_memory_offset);

    // PHASE 2: Build master kernel PML4 with upper-half mappings
    kernel_page_table::build_master_kernel_pml4();

    // CRITICAL: Update kernel_cr3 in per-CPU data to the new master PML4
    // per_cpu::init() already ran and set kernel_cr3 to the bootloader's CR3
    // Now that we've switched to the master PML4, we must update it
    #[cfg(target_arch = "x86_64")]
    {
        use x86_64::registers::control::Cr3;
        let (current_frame, _) = Cr3::read();
        let master_cr3 = current_frame.start_address().as_u64();
        log::info!(
            "CRITICAL: Updating kernel_cr3 to master PML4: {:#x}",
            master_cr3
        );
        crate::per_cpu::set_kernel_cr3(master_cr3);
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 uses TTBR0/TTBR1 for page tables; kernel pages are in TTBR1
        log::info!("ARM64: Page table setup handled by boot.S TTBR configuration");
    }

    // Migrate any existing processes (though there shouldn't be any yet)
    kernel_page_table::migrate_existing_processes();

    // PHASE 2: Enable global pages support (CR4.PGE)
    // This must be done after kernel page tables are set up but before userspace
    unsafe {
        paging::enable_global_pages();
    }

    // CRITICAL: Recreate mapper after CR3 switch to master PML4
    // The old mapper pointed to bootloader's PML4, which is now stale
    #[cfg(target_arch = "x86_64")]
    let mapper = unsafe { paging::init(physical_memory_offset) };
    #[cfg(not(target_arch = "x86_64"))]
    let mapper = unsafe { paging::init(physical_memory_offset) };

    // Initialize heap
    #[cfg(target_arch = "x86_64")]
    {
        log::info!("Initializing heap allocator...");
        heap::init(&mapper).expect("heap initialization failed");
    }
    #[cfg(target_arch = "aarch64")]
    {
        log::info!("Initializing heap allocator...");
        heap::init(&mapper).expect("heap initialization failed");
    }
    frame_allocator::init_frame_ledger();

    // Initialize slab caches (must be after heap)
    slab::init();

    // Initialize stack allocation system
    log::info!("Initializing stack allocation system...");
    stack::init();

    // Initialize kernel stack allocator
    log::info!("Initializing kernel stack allocator...");
    kernel_stack::init();

    // Initialize per-CPU emergency stacks
    log::info!("Initializing per-CPU emergency stacks...");
    // One slot for every CPU the per-CPU state can address, mapped now so a
    // secondary CPU's per-CPU init finds its IST stacks already present.
    let _emergency_stacks = per_cpu_stack::init_per_cpu_stacks(crate::task::scheduler::MAX_CPUS)
        .expect("Failed to initialize per-CPU stacks");

    log::info!("Memory management initialized");

    #[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
    frame_allocator::run_x86_frame_custody_gate();
    #[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
    process_memory::run_x86_page_table_custody_gate();
}

/// Get the physical memory offset
pub fn physical_memory_offset() -> VirtAddr {
    *PHYSICAL_MEMORY_OFFSET
        .get()
        .expect("physical memory offset not initialized")
}

/// Convert a physical address to a virtual address using the offset mapping
pub fn phys_to_virt(phys: PhysAddr, offset: VirtAddr) -> VirtAddr {
    VirtAddr::new(phys.as_u64() + offset.as_u64())
}

/// Allocate a kernel stack using the bitmap-based allocator
/// Note: size parameter is ignored - all kernel stacks are 8KB + 4KB guard
pub fn alloc_kernel_stack(_size: usize) -> Option<kernel_stack::KernelStack> {
    kernel_stack::allocate_kernel_stack().ok()
}

/// Display comprehensive memory debug information
pub fn debug_memory_info() {
    log::info!("=== Memory Debug Information ===");

    // Physical memory offset
    let phys_offset = physical_memory_offset();
    log::info!("Physical memory offset: {:#x}", phys_offset);

    // Frame allocator stats
    log::info!("Frame Allocator:");
    // Try to allocate a frame to see if allocator is working
    if let Some(frame) = frame_allocator::allocate_frame() {
        log::info!(
            "  - Test frame allocation successful: {:#x}",
            frame.start_address()
        );
        log::info!("  - Frame allocator is operational");
    } else {
        log::error!("  - Frame allocator returned None!");
    }

    // Test stack allocation
    log::info!("\nTesting stack allocation...");
    match stack::allocate_stack(16 * 1024) {
        // 16 KiB stack
        Ok(stack) => {
            log::info!("✓ Successfully allocated 16 KiB guarded stack");
            log::info!("  - Stack top: {:#x}", stack.top());
            log::info!("  - Stack bottom: {:#x}", stack.bottom());
            log::info!("  - Guard page: {:#x}", stack.guard_page());
            log::info!("  - Stack size: {} bytes", stack.size());

            // Test address containment
            let test_addr = stack.top() - 100u64;
            log::info!(
                "  - Contains {:#x}? {}",
                test_addr,
                stack.contains(test_addr)
            );
            let outside_addr = stack.guard_page();
            log::info!(
                "  - Contains {:#x} (guard)? {}",
                outside_addr,
                stack.contains(outside_addr)
            );
        }
        Err(e) => {
            log::error!("✗ Failed to allocate stack: {}", e);
        }
    }

    // Test phys_to_virt conversion
    log::info!("\nTesting physical to virtual conversion:");
    let test_phys = PhysAddr::new(0x1000);
    let test_virt = phys_to_virt(test_phys, phys_offset);
    log::info!("  - Physical {:#x} -> Virtual {:#x}", test_phys, test_virt);

    // Heap information
    log::info!("\nHeap Information:");
    use alloc::vec::Vec;
    let test_vec: Vec<u8> = Vec::with_capacity(1024);
    log::info!("  - Test vector capacity: {} bytes", test_vec.capacity());
    log::info!("  - Test vector ptr: {:p}", test_vec.as_ptr());

    // Stack allocation area info
    log::info!("\nStack Allocation Areas:");
    log::info!(
        "  - USER_STACK_REGION_START: {:#x}",
        layout::USER_STACK_REGION_START
    );
    log::info!(
        "  - KERNEL_STACK_ALLOC_START: {:#x}",
        stack::KERNEL_STACK_ALLOC_START
    );

    log::info!("=============================");
}

/// Map a physical MMIO region into kernel virtual address space
///
/// Allocates virtual address space from the MMIO region and creates page table
/// mappings for the given physical address range.
///
/// Returns the virtual address where the MMIO region is mapped.
pub fn map_mmio(phys_addr: u64, size: usize) -> Result<usize, &'static str> {
    let phys_offset = physical_memory_offset();

    // Align size up to page boundary
    let size_aligned = (size + 0xFFF) & !0xFFF;
    let num_pages = size_aligned / 4096;

    // Allocate virtual address space
    let virt_addr = {
        let mut next = MMIO_NEXT_ADDR.lock();
        let addr = *next;
        *next += size_aligned as u64;
        addr
    };

    log::info!(
        "MMIO: Mapping {:#x} -> {:#x} ({} pages)",
        phys_addr,
        virt_addr,
        num_pages
    );

    // Get mapper
    let mut mapper = unsafe { paging::get_mapper_with_offset(phys_offset) };

    // Map each page with uncacheable flags
    for i in 0..num_pages {
        let page_phys = phys_addr + (i * 4096) as u64;
        let page_virt = virt_addr + (i * 4096) as u64;

        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_virt));
        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(page_phys));

        // Use write-through, no-cache flags for MMIO
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::WRITE_THROUGH;

        unsafe {
            match mapper.map_to(
                page,
                frame,
                flags,
                &mut frame_allocator::GlobalFrameAllocator,
            ) {
                Ok(flush) => flush.flush(),
                Err(_) => {
                    for mapped_index in 0..i {
                        let mapped_page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                            virt_addr + (mapped_index * 4096) as u64,
                        ));
                        if let Ok((_, flush)) = mapper.unmap(mapped_page) {
                            flush.flush();
                        }
                    }
                    return Err("Failed to map MMIO page");
                }
            }
        }
    }

    Ok(virt_addr as usize)
}

pub use kernel_walk::WalkError;

/// Converts kernel virtual addresses to the physical addresses devices use.
pub struct PhysAddrWrapper;

impl PhysAddrWrapper {
    /// The physical address mapped at kernel virtual address `virt`, from the
    /// translation the CPU itself uses, or the reason there is none.
    ///
    /// On x86-64 this walks the active page tables (`kernel_walk::walk_x86_64`)
    /// for every address, the direct map included, so it assumes nothing about
    /// where RAM ends. On ARM64 the MMU translates it (`AT S1E1R`, read back
    /// from PAR_EL1), which covers both translation tables and the
    /// direct-map offset the loader chose.
    pub fn from_kernel_virt(virt: usize) -> Result<u64, WalkError> {
        #[cfg(target_arch = "x86_64")]
        {
            use x86_64::registers::control::Cr3;
            let phys_offset = physical_memory_offset().as_u64();
            let (pml4_frame, _) = Cr3::read();
            kernel_walk::walk_x86_64(pml4_frame.start_address().as_u64(), virt as u64, |table, index| {
                // SAFETY: page-table frames are RAM, which the direct map at
                // `phys_offset` covers, and `index` is below 512.
                unsafe { core::ptr::read_volatile((phys_offset + table + 8 * index as u64) as *const u64) }
            })
        }
        #[cfg(target_arch = "aarch64")]
        {
            // PAR_EL1 belongs to whoever ran the last AT on this CPU, so the
            // translation and its read stay together with interrupts masked.
            let par = crate::arch_without_interrupts(|| {
                let par: u64;
                // SAFETY: AT only translates; it does not access the address.
                unsafe {
                    core::arch::asm!(
                        "at s1e1r, {virt}",
                        "isb",
                        "mrs {par}, par_el1",
                        virt = in(reg) virt as u64,
                        par = out(reg) par,
                        options(nostack, preserves_flags),
                    );
                }
                par
            });
            kernel_walk::decode_par_el1(par, virt as u64)
        }
    }
}
