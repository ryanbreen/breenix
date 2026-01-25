pub mod frame_allocator;
pub mod frame_metadata;
pub mod heap;
pub mod kernel_page_table;
pub mod kernel_stack;
pub mod layout;
pub mod paging;
pub mod per_cpu_stack;
pub mod process_memory;
pub mod stack;
pub mod tlb;
pub mod vma;
#[cfg(not(target_arch = "x86_64"))]
pub mod arch_stub;

use bootloader_api::info::MemoryRegions;
use conquer_once::spin::OnceCell;
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, Page, PageTableFlags, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::{PhysAddr, VirtAddr};
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{Mapper, Page, PageTableFlags, PhysFrame, Size4KiB, PhysAddr, VirtAddr};

/// Global physical memory offset for use throughout the kernel
static PHYSICAL_MEMORY_OFFSET: OnceCell<VirtAddr> = OnceCell::uninit();

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
        log::info!("CRITICAL: Updating kernel_cr3 to master PML4: {:#x}", master_cr3);
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
    let _mapper = unsafe { paging::init(physical_memory_offset) };

    // Initialize heap
    // For ARM64, the heap is initialized in main_aarch64.rs with a simple bump allocator
    #[cfg(target_arch = "x86_64")]
    {
        log::info!("Initializing heap allocator...");
        heap::init(&mapper).expect("heap initialization failed");
    }
    #[cfg(target_arch = "aarch64")]
    {
        log::info!("ARM64: Using bump allocator from main_aarch64.rs");
    }

    // Initialize stack allocation system
    log::info!("Initializing stack allocation system...");
    stack::init();

    // Initialize kernel stack allocator
    log::info!("Initializing kernel stack allocator...");
    kernel_stack::init();

    // Initialize per-CPU emergency stacks
    log::info!("Initializing per-CPU emergency stacks...");
    // For now, assume single CPU. In SMP systems, this would be the actual CPU count
    let _emergency_stacks =
        per_cpu_stack::init_per_cpu_stacks(1).expect("Failed to initialize per-CPU stacks");

    log::info!("Memory management initialized");
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
            mapper
                .map_to(
                    page,
                    frame,
                    flags,
                    &mut frame_allocator::GlobalFrameAllocator,
                )
                .map_err(|_| "Failed to map MMIO page")?
                .flush();
        }
    }

    Ok(virt_addr as usize)
}

/// Wrapper for PhysAddr operations that converts kernel virtual addresses
/// to physical addresses
pub struct PhysAddrWrapper;

impl PhysAddrWrapper {
    /// Convert a kernel virtual address to a physical address
    ///
    /// This function handles several types of kernel addresses:
    /// 1. Direct physical memory mappings (starting at phys_offset) - subtract offset
    /// 2. Heap and other mapped regions - walk page tables to find physical address
    ///
    /// NOTE: The direct physical memory map starts at phys_offset and extends for
    /// the size of physical RAM (~500MB in QEMU). Addresses like the heap (0x4444_4444_0000)
    /// are NOT in this region even though they may be numerically greater than phys_offset.
    pub fn from_kernel_virt(virt: usize) -> u64 {
        let phys_offset = physical_memory_offset();
        let heap_start = crate::memory::heap::HEAP_START;
        let heap_end = heap_start + crate::memory::heap::HEAP_SIZE;

        // Check if this is a heap address - these are mapped, not direct
        let is_heap = (virt as u64) >= heap_start && (virt as u64) < heap_end;

        // The direct physical memory map starts at phys_offset.
        // We can detect if an address is truly in the direct map by checking:
        // 1. It's >= phys_offset
        // 2. Subtracting phys_offset gives a reasonable physical address (< 4GB typically)
        // 3. It's NOT in a known non-direct-mapped region (heap, MMIO, stack, etc.)
        if !is_heap && (virt as u64) >= phys_offset.as_u64() {
            let candidate_phys = (virt as u64) - phys_offset.as_u64();
            // Physical RAM is typically < 4GB in our QEMU setup (512MB max)
            // If the result is reasonable, it's likely a direct map address
            if candidate_phys < 0x1_0000_0000 {
                return candidate_phys;
            }
        }

        // For heap and other mapped regions, use the page table entry directly
        // We can't use translate_addr because it adds the offset back when reading PTEs
        #[cfg(target_arch = "x86_64")]
        use x86_64::registers::control::Cr3;
        #[cfg(not(target_arch = "x86_64"))]
        use crate::memory::arch_stub::Cr3;
        #[cfg(target_arch = "x86_64")]
        use x86_64::structures::paging::PageTable;
        #[cfg(not(target_arch = "x86_64"))]
        use crate::memory::arch_stub::PageTable;

        let virt_addr = VirtAddr::new(virt as u64);

        // Get page table indices for this virtual address
        let p4_idx = (virt_addr.as_u64() >> 39) & 0x1FF;
        let p3_idx = (virt_addr.as_u64() >> 30) & 0x1FF;
        let p2_idx = (virt_addr.as_u64() >> 21) & 0x1FF;
        let p1_idx = (virt_addr.as_u64() >> 12) & 0x1FF;
        let page_offset = virt_addr.as_u64() & 0xFFF;

        // Get the CR3 (PML4 physical address)
        let (pml4_frame, _) = Cr3::read();
        let pml4_phys = pml4_frame.start_address().as_u64();

        // Access PML4 through physical memory mapping
        let pml4_virt = phys_offset.as_u64() + pml4_phys;
        let pml4 = unsafe { &*(pml4_virt as *const PageTable) };

        let p4_entry = &pml4[p4_idx as usize];
        if !p4_entry.flags().contains(PageTableFlags::PRESENT) {
            log::error!("PhysAddrWrapper: PML4[{}] not present for virt {:#x}", p4_idx, virt);
            return virt as u64;
        }

        // Get PDPT
        let pdpt_phys = p4_entry.addr().as_u64();
        let pdpt_virt = phys_offset.as_u64() + pdpt_phys;
        let pdpt = unsafe { &*(pdpt_virt as *const PageTable) };

        let p3_entry = &pdpt[p3_idx as usize];
        if !p3_entry.flags().contains(PageTableFlags::PRESENT) {
            log::error!("PhysAddrWrapper: PDPT[{}] not present for virt {:#x}", p3_idx, virt);
            return virt as u64;
        }

        // Check for 1GB huge page
        if p3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            let phys = p3_entry.addr().as_u64() + (virt_addr.as_u64() & 0x3FFFFFFF);
            return phys;
        }

        // Get PD
        let pd_phys = p3_entry.addr().as_u64();
        let pd_virt = phys_offset.as_u64() + pd_phys;
        let pd = unsafe { &*(pd_virt as *const PageTable) };

        let p2_entry = &pd[p2_idx as usize];
        if !p2_entry.flags().contains(PageTableFlags::PRESENT) {
            log::error!("PhysAddrWrapper: PD[{}] not present for virt {:#x}", p2_idx, virt);
            return virt as u64;
        }

        // Check for 2MB huge page
        if p2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            let phys = p2_entry.addr().as_u64() + (virt_addr.as_u64() & 0x1FFFFF);
            return phys;
        }

        // Get PT
        let pt_phys = p2_entry.addr().as_u64();
        let pt_virt = phys_offset.as_u64() + pt_phys;
        let pt = unsafe { &*(pt_virt as *const PageTable) };

        let p1_entry = &pt[p1_idx as usize];
        if !p1_entry.flags().contains(PageTableFlags::PRESENT) {
            log::error!("PhysAddrWrapper: PT[{}] not present for virt {:#x}", p1_idx, virt);
            return virt as u64;
        }

        // Final physical address
        let frame_phys = p1_entry.addr().as_u64();
        let phys = frame_phys + page_offset;

        phys
    }
}
