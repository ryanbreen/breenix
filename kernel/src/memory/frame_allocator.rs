use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
#[cfg(feature = "testing")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::PhysAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{FrameAllocator, PhysFrame, Size4KiB, PhysAddr};

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

/// Free list for deallocated frames
/// When frames are deallocated (e.g., after CoW copy reduces refcount to 0),
/// they are added to this list for reuse
static FREE_FRAMES: Mutex<Vec<PhysFrame>> = Mutex::new(Vec::new());

/// Test-only flag to simulate OOM conditions
///
/// When set to true, allocate_frame() will return None to simulate out-of-memory.
/// This is used to test that CoW fault handling gracefully terminates processes
/// when memory allocation fails.
///
/// # Safety
/// Only enable this flag briefly during testing. The flag affects ALL frame
/// allocations, so enabling it for too long will crash the kernel.
#[cfg(feature = "testing")]
static SIMULATE_OOM: AtomicBool = AtomicBool::new(false);

/// Enable OOM simulation for testing
///
/// After calling this, all frame allocations will return None until
/// `disable_oom_simulation()` is called.
///
/// # Warning
/// Only use this for brief tests! Extended OOM simulation will crash the kernel.
#[cfg(feature = "testing")]
pub fn enable_oom_simulation() {
    log::warn!("OOM simulation ENABLED - all frame allocations will fail");
    SIMULATE_OOM.store(true, Ordering::SeqCst);
}

/// Disable OOM simulation
#[cfg(feature = "testing")]
pub fn disable_oom_simulation() {
    SIMULATE_OOM.store(false, Ordering::SeqCst);
    log::info!("OOM simulation disabled - frame allocations restored");
}

/// Check if OOM simulation is currently active
#[cfg(feature = "testing")]
#[allow(dead_code)] // May be useful for future diagnostic output
pub fn is_oom_simulation_active() -> bool {
    SIMULATE_OOM.load(Ordering::SeqCst)
}

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
        // Use compare-exchange loop to avoid wasting frame slots on failure
        loop {
            let current = NEXT_FREE_FRAME.load(Ordering::SeqCst);
            log::trace!("Frame allocator: Attempting to allocate frame #{}", current);

            // Try to get the frame at this index
            let frame = Self::get_usable_frame(current);
            if frame.is_none() {
                // No more frames available - don't increment counter
                return None;
            }

            // Try to claim this frame atomically
            match NEXT_FREE_FRAME.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    // Successfully claimed the frame
                    if let Some(f) = &frame {
                        log::trace!(
                            "Frame allocator: Allocated frame {:#x} (allocation #{})",
                            f.start_address().as_u64(),
                            current
                        );
                    }
                    return frame;
                }
                Err(_) => {
                    // Another thread got there first, retry
                    continue;
                }
            }
        }
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

/// Initialize the frame allocator for ARM64 with a simple memory range
/// This is used during ARM64 boot where we don't have bootloader memory info.
///
/// # Arguments
/// * `start` - Start address of usable memory (must be page-aligned)
/// * `end` - End address of usable memory (exclusive)
#[cfg(target_arch = "aarch64")]
pub fn init_aarch64(start: u64, end: u64) {
    let mut regions = [None; MAX_REGIONS];

    // Page-align the start address (round up)
    let aligned_start = (start + 0xFFF) & !0xFFF;

    regions[0] = Some(UsableRegion {
        start: aligned_start,
        end,
    });

    let total_memory = end - aligned_start;

    *MEMORY_INFO.lock() = Some(MemoryInfo {
        regions,
        region_count: 1,
    });

    log::info!(
        "ARM64 frame allocator initialized: {:#x}..{:#x} ({} MiB)",
        aligned_start,
        end,
        total_memory / (1024 * 1024)
    );
}

/// Allocate a physical frame
///
/// First checks the free list for previously deallocated frames,
/// then falls back to sequential allocation from the memory map.
///
/// # OOM Behavior
///
/// When memory is exhausted (or OOM simulation is active in test builds),
/// this function returns `None`. Callers must handle this gracefully:
///
/// - **CoW fault handler**: Returns `false`, causing the page fault handler
///   to terminate the process with SIGSEGV (exit code -11). This is the
///   correct POSIX behavior for processes that cannot allocate memory
///   during page faults.
///
/// - **Other kernel code**: Should propagate the error or use fallback paths.
pub fn allocate_frame() -> Option<PhysFrame> {
    // Test-only: simulate OOM if flag is set
    #[cfg(feature = "testing")]
    if SIMULATE_OOM.load(Ordering::SeqCst) {
        log::trace!("Frame allocator: OOM simulation active, returning None");
        return None;
    }

    // Try to reuse a frame from the free list (all architectures).
    // Uses try_lock() to avoid deadlock if called from interrupt context.
    if let Some(mut free_list) = FREE_FRAMES.try_lock() {
        if let Some(frame) = free_list.pop() {
            log::trace!(
                "Frame allocator: Reused frame {:#x} from free list ({} remaining)",
                frame.start_address().as_u64(),
                free_list.len()
            );
            return Some(frame);
        }
    }

    // Fall back to sequential allocation from memory map
    let mut allocator = BootInfoFrameAllocator::new();
    allocator.allocate_frame()
}

/// Deallocate a physical frame, returning it to the free pool
///
/// The frame will be available for reuse by future allocations.
/// This is called when a CoW page's reference count drops to zero.
pub fn deallocate_frame(frame: PhysFrame) {
    // Don't deallocate frames below the low memory floor
    if frame.start_address().as_u64() < LOW_MEMORY_FLOOR {
        log::warn!(
            "Refusing to deallocate frame {:#x} below low memory floor",
            frame.start_address().as_u64()
        );
        return;
    }

    if let Some(mut free_list) = FREE_FRAMES.try_lock() {
        log::trace!(
            "Frame allocator: Deallocated frame {:#x} (free list size: {})",
            frame.start_address().as_u64(),
            free_list.len() + 1
        );
        free_list.push(frame);
    } else {
        // If we can't get the lock (e.g., called from interrupt context),
        // we lose this frame. This is a memory leak but prevents deadlock.
        log::warn!(
            "Frame allocator: Could not deallocate frame {:#x} - lock contention",
            frame.start_address().as_u64()
        );
    }
}

/// Memory statistics for procfs reporting
pub struct MemoryStats {
    /// Total usable memory in bytes
    pub total_bytes: u64,
    /// Number of frames allocated (sequential allocator index)
    pub allocated_frames: usize,
    /// Number of frames in the free list (available for reuse)
    pub free_list_frames: usize,
}

/// Get current memory statistics for procfs /proc/meminfo
///
/// Returns total usable memory, allocated frame count, and free list size.
/// These can be used to compute total, used, and free memory.
pub fn memory_stats() -> MemoryStats {
    // Calculate total memory from MEMORY_INFO regions
    let total_bytes = if let Some(info_guard) = MEMORY_INFO.try_lock() {
        if let Some(ref info) = *info_guard {
            let mut total = 0u64;
            for i in 0..info.region_count {
                if let Some(region) = info.regions[i] {
                    total += region.end - region.start;
                }
            }
            total
        } else {
            0
        }
    } else {
        0
    };

    let allocated_frames = NEXT_FREE_FRAME.load(Ordering::Relaxed);

    let free_list_frames = if let Some(free_list) = FREE_FRAMES.try_lock() {
        free_list.len()
    } else {
        0
    };

    MemoryStats {
        total_bytes,
        allocated_frames,
        free_list_frames,
    }
}

/// A wrapper that allows using the global frame allocator with the mapper
pub struct GlobalFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        allocate_frame()
    }
}
