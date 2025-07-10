use bootloader_api::info::{MemoryRegions, MemoryRegionKind};
use x86_64::structures::paging::{PhysFrame, Size4KiB, FrameAllocator};
use x86_64::PhysAddr;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Maximum number of usable memory regions we support
/// Increased from 32 to 128 to handle UEFI's fragmented memory map
const MAX_REGIONS: usize = 128;

/// A memory region descriptor
#[derive(Debug, Clone, Copy)]
struct UsableRegion {
    start: u64,
    end: u64,
}

/// Stores extracted memory information
struct MemoryInfo {
    regions: [Option<UsableRegion>; MAX_REGIONS],
    region_count: usize,
}

static MEMORY_INFO: Mutex<Option<MemoryInfo>> = Mutex::new(None);
static NEXT_FREE_FRAME: AtomicUsize = AtomicUsize::new(0);

/// A simple frame allocator that returns usable frames from the bootloader's memory map
pub struct BootInfoFrameAllocator;

impl BootInfoFrameAllocator {
    /// Create a new frame allocator
    pub fn new() -> Self {
        Self
    }
    
    /// Get the nth usable frame
    fn get_usable_frame(n: usize) -> Option<PhysFrame> {
        // Check if we're in a problematic allocation
        if n > 1500 && n < 1600 {
            log::debug!("get_usable_frame: Allocating frame number {}", n);
        }
        
        // Try to detect potential deadlock
        let info = match MEMORY_INFO.try_lock() {
            Some(guard) => guard,
            None => {
                log::error!("MEMORY_INFO lock is already held - potential deadlock!");
                // Force a panic with more info
                panic!("Frame allocator deadlock detected during allocation #{}", n);
            }
        };
        let info = info.as_ref()?;
        
        let mut count = 0;
        for i in 0..info.region_count {
            if let Some(region) = info.regions[i] {
                let region_frames = (region.end - region.start) / 4096;
                
                if count + region_frames as usize > n {
                    let frame_offset = n - count;
                    let frame_addr = region.start + (frame_offset as u64 * 4096);
                    
                    // Log problematic frame allocations
                    if frame_addr == 0x62f000 {
                        log::warn!("Allocating problematic frame 0x62f000 (frame #{}, region {}, offset {})", 
                                  n, i, frame_offset);
                    }
                    
                    return Some(PhysFrame::containing_address(PhysAddr::new(frame_addr)));
                }
                count += region_frames as usize;
            }
        }
        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let current = NEXT_FREE_FRAME.fetch_add(1, Ordering::SeqCst);
        let frame = Self::get_usable_frame(current);
        frame
    }
}

/// Initialize the global frame allocator
pub fn init(memory_regions: &'static MemoryRegions) {
    let mut regions = [None; MAX_REGIONS];
    let mut region_count = 0;
    let mut total_memory = 0u64;
    let mut ignored_regions = 0;
    let mut ignored_memory = 0u64;
    
    // Extract usable regions
    for region in memory_regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            if region_count < MAX_REGIONS {
                regions[region_count] = Some(UsableRegion {
                    start: region.start,
                    end: region.end,
                });
                region_count += 1;
                total_memory += region.end - region.start;
            } else {
                // Count ignored regions instead of logging each one
                ignored_regions += 1;
                ignored_memory += region.end - region.start;
            }
        }
    }
    
    // Store the extracted information
    *MEMORY_INFO.lock() = Some(MemoryInfo {
        regions,
        region_count,
    });
    
    log::info!("Frame allocator initialized with {} MiB of usable memory in {} regions", 
               total_memory / (1024 * 1024), region_count);
    
    if ignored_regions > 0 {
        log::warn!("Ignored {} memory regions ({} MiB) due to MAX_REGIONS limit", 
                   ignored_regions, ignored_memory / (1024 * 1024));
    }
}

/// Allocate a physical frame
pub fn allocate_frame() -> Option<PhysFrame> {
    let mut allocator = BootInfoFrameAllocator::new();
    allocator.allocate_frame()
}


/// A wrapper that allows using the global frame allocator with the mapper
pub struct GlobalFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        allocate_frame()
    }
}