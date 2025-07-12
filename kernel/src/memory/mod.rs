pub mod frame_allocator;
pub mod paging;
pub mod heap;
pub mod stack;
pub mod process_memory;
pub mod tlb;
pub mod kernel_page_table;
pub mod kernel_stack;
pub mod per_cpu_stack;
pub mod fork_helpers;

use bootloader_api::info::MemoryRegions;
use x86_64::{PhysAddr, VirtAddr};
use conquer_once::spin::OnceCell;

/// Global physical memory offset for use throughout the kernel
static PHYSICAL_MEMORY_OFFSET: OnceCell<VirtAddr> = OnceCell::uninit();

/// Initialize the memory subsystem
pub fn init(
    physical_memory_offset: VirtAddr,
    memory_regions: &'static MemoryRegions,
) {
    log::info!("Initializing memory management...");
    log::info!("Physical memory offset: {:?}", physical_memory_offset);
    
    // Store the physical memory offset globally
    PHYSICAL_MEMORY_OFFSET.init_once(|| physical_memory_offset);
    
    // Initialize frame allocator
    log::info!("Initializing frame allocator...");
    frame_allocator::init(memory_regions);
    
    // Initialize paging
    log::info!("Initializing paging...");
    let mapper = unsafe { paging::init(physical_memory_offset) };
    
    // Save the kernel page table for later switching
    process_memory::init_kernel_page_table();
    
    // Initialize global kernel page table system
    log::info!("Initializing global kernel page tables...");
    kernel_page_table::init(physical_memory_offset);
    
    // Migrate any existing processes (though there shouldn't be any yet)
    kernel_page_table::migrate_existing_processes();
    
    // Initialize heap
    log::info!("Initializing heap allocator...");
    heap::init(&mapper).expect("heap initialization failed");
    
    // Initialize stack allocation system
    log::info!("Initializing stack allocation system...");
    stack::init();
    
    // Initialize kernel stack allocator
    log::info!("Initializing kernel stack allocator...");
    kernel_stack::init();
    
    // Initialize per-CPU emergency stacks
    log::info!("Initializing per-CPU emergency stacks...");
    // For now, assume single CPU. In SMP systems, this would be the actual CPU count
    let _emergency_stacks = per_cpu_stack::init_per_cpu_stacks(1)
        .expect("Failed to initialize per-CPU stacks");
    
    log::info!("Memory management initialized");
}

/// Get the physical memory offset
pub fn physical_memory_offset() -> VirtAddr {
    *PHYSICAL_MEMORY_OFFSET.get().expect("physical memory offset not initialized")
}

/// Convert a physical address to a virtual address using the offset mapping
pub fn phys_to_virt(phys: PhysAddr, offset: VirtAddr) -> VirtAddr {
    VirtAddr::new(phys.as_u64() + offset.as_u64())
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
        log::info!("  - Test frame allocation successful: {:#x}", frame.start_address());
        log::info!("  - Frame allocator is operational");
    } else {
        log::error!("  - Frame allocator returned None!");
    }
    
    // Test stack allocation
    log::info!("\nTesting stack allocation...");
    match stack::allocate_stack(16 * 1024) { // 16 KiB stack
        Ok(stack) => {
            log::info!("✓ Successfully allocated 16 KiB guarded stack");
            log::info!("  - Stack top: {:#x}", stack.top());
            log::info!("  - Stack bottom: {:#x}", stack.bottom());
            log::info!("  - Guard page: {:#x}", stack.guard_page());
            log::info!("  - Stack size: {} bytes", stack.size());
            
            // Test address containment
            let test_addr = stack.top() - 100u64;
            log::info!("  - Contains {:#x}? {}", test_addr, stack.contains(test_addr));
            let outside_addr = stack.guard_page();
            log::info!("  - Contains {:#x} (guard)? {}", outside_addr, stack.contains(outside_addr));
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
    log::info!("  - USER_STACK_ALLOC_START: {:#x}", stack::USER_STACK_ALLOC_START);
    log::info!("  - KERNEL_STACK_ALLOC_START: {:#x}", stack::KERNEL_STACK_ALLOC_START);
    
    log::info!("=============================");
}