pub mod frame_allocator;
pub mod paging;
pub mod heap;

use bootloader_api::info::MemoryRegions;
use x86_64::{PhysAddr, VirtAddr};

/// Initialize the memory subsystem
pub fn init(
    physical_memory_offset: VirtAddr,
    memory_regions: &'static MemoryRegions,
) {
    log::info!("Initializing memory management...");
    log::info!("Physical memory offset: {:?}", physical_memory_offset);
    
    // Initialize frame allocator
    log::info!("Initializing frame allocator...");
    frame_allocator::init(memory_regions);
    
    // Initialize paging
    log::info!("Initializing paging...");
    let mapper = unsafe { paging::init(physical_memory_offset) };
    
    // Initialize heap
    log::info!("Initializing heap allocator...");
    heap::init(&mapper).expect("heap initialization failed");
    
    log::info!("Memory management initialized");
}

/// Convert a physical address to a virtual address using the offset mapping
#[allow(dead_code)]
pub fn phys_to_virt(phys: PhysAddr, offset: VirtAddr) -> VirtAddr {
    VirtAddr::new(phys.as_u64() + offset.as_u64())
}