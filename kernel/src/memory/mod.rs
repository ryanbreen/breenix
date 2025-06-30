pub mod frame_allocator;
pub mod paging;
pub mod heap;
pub mod stack;

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
    
    // Initialize heap
    log::info!("Initializing heap allocator...");
    heap::init(&mapper).expect("heap initialization failed");
    
    // Initialize stack allocation system
    log::info!("Initializing stack allocation system...");
    stack::init();
    
    log::info!("Memory management initialized");
}

/// Get the physical memory offset
pub fn physical_memory_offset() -> VirtAddr {
    *PHYSICAL_MEMORY_OFFSET.get().expect("physical memory offset not initialized")
}

/// Convert a physical address to a virtual address using the offset mapping
#[allow(dead_code)]
pub fn phys_to_virt(phys: PhysAddr, offset: VirtAddr) -> VirtAddr {
    VirtAddr::new(phys.as_u64() + offset.as_u64())
}