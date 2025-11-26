use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

/// Maximum number of usable memory regions we support
/// Increased from 32 to 128 to handle UEFI's fragmented memory map
const MAX_REGIONS: usize = 128;

/// Low memory floor - we never allocate frames below 1MiB
/// This avoids issues with:
/// - Frame 0x0 (null pointer confusion)
/// - BIOS/firmware reserved areas
/// - Legacy device memory (VGA, etc)
const LOW_MEMORY_FLOOR: u64 = 0x100000; // 1 MiB

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

                    // CRITICAL: Assert we never return frame 0x0
                    debug_assert!(
                        frame_addr >= LOW_MEMORY_FLOOR,
                        "Attempting to allocate frame below low memory floor: {:#x}",
                        frame_addr
                    );
                    
                    // Log problematic frame allocations
                    if frame_addr == 0x62f000 {
                        log::warn!("Allocating problematic frame 0x62f000 (frame #{}, region {}, offset {})", 
                                  n, i, frame_offset);
                    }
                    
                    // Production safety: Never return frames below the floor
                    if frame_addr < LOW_MEMORY_FLOOR {
                        log::error!(
                            "CRITICAL: Attempted to allocate frame {:#x} below low memory floor {:#x}",
                            frame_addr, LOW_MEMORY_FLOOR
                        );
                        return None;
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
        log::trace!("Frame allocator: Attempting to allocate frame #{}", current);
        let frame = Self::get_usable_frame(current);
        if let Some(f) = frame {
            log::trace!(
                "Frame allocator: Allocated frame {:#x} (allocation #{})",
                f.start_address().as_u64(),
                current
            );
        }
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

    // Extract usable regions, excluding low memory below the floor
    for region in memory_regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            // Skip regions entirely below the low memory floor
            if region.end <= LOW_MEMORY_FLOOR {
                log::debug!(
                    "Skipping low memory region {:#x}..{:#x} (below floor {:#x})",
                    region.start, region.end, LOW_MEMORY_FLOOR
                );
                ignored_regions += 1;
                ignored_memory += region.end - region.start;
                continue;
            }
            
            if region_count < MAX_REGIONS {
                // Adjust region start if it begins below the floor
                let adjusted_start = if region.start < LOW_MEMORY_FLOOR {
                    log::info!(
                        "Adjusting region start from {:#x} to {:#x} (low memory floor)",
                        region.start, LOW_MEMORY_FLOOR
                    );
                    LOW_MEMORY_FLOOR
                } else {
                    region.start
                };
                
                regions[region_count] = Some(UsableRegion {
                    start: adjusted_start,
                    end: region.end,
                });
                region_count += 1;
                total_memory += region.end - adjusted_start;
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

    log::info!(
        "Frame allocator initialized with {} MiB of usable memory in {} regions (floor={:#x})",
        total_memory / (1024 * 1024),
        region_count,
        LOW_MEMORY_FLOOR
    );

    if ignored_regions > 0 {
        log::warn!(
            "Ignored {} memory regions ({} MiB) due to MAX_REGIONS limit",
            ignored_regions,
            ignored_memory / (1024 * 1024)
        );
    }
}

/// Allocate a physical frame
pub fn allocate_frame() -> Option<PhysFrame> {
    let mut allocator = BootInfoFrameAllocator::new();
    allocator.allocate_frame()
}

/// Deallocate a physical frame (currently a no-op)
/// TODO: Implement proper frame deallocation
#[allow(dead_code)]
pub fn deallocate_frame(_frame: PhysFrame) {
    // For now, we don't reclaim frames
    // A proper implementation would add the frame back to a free list
}

/// A wrapper that allows using the global frame allocator with the mapper
pub struct GlobalFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        allocate_frame()
    }
}
