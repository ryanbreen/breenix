#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{FrameAllocator, PhysAddr, PhysFrame, Size4KiB};
use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
#[cfg(feature = "testing")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
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

/// Stores extracted memory information (immutable after initialization)
struct MemoryInfo {
    regions: [Option<UsableRegion>; MAX_REGIONS],
    region_count: usize,
}

/// Memory region info, initialized once during boot and never modified.
/// Using spin::Once instead of Mutex eliminates lock contention on the
/// frame allocation hot path - get() is a single atomic load after init.
static MEMORY_INFO: spin::Once<MemoryInfo> = spin::Once::new();
static NEXT_FREE_FRAME: AtomicUsize = AtomicUsize::new(0);

const LEDGER_CHUNK_FRAMES: usize = 64 * 1024;
const MAX_TRACKED_FRAMES: usize = u32::MAX as usize;
const ST_NEVER: u32 = 0;
const ST_ALLOCATED: u32 = 1;
const ST_FREE: u32 = 2;
const STATE_MASK: u32 = 0b11;

/// Demand-backed frame state indexed by the allocator's usable-frame ordinal.
/// Only the directory and chunks below the boot frontier are allocated at init;
/// later chunks are prepared before the sequential frontier publishes a frame.
struct FrameLedger {
    chunks: &'static [spin::Once<&'static [AtomicU32]>],
    total_frames: usize,
    initial_frontier: usize,
}

impl FrameLedger {
    fn try_new(total_frames: usize, initial_frontier: usize) -> Result<Self, ()> {
        let chunk_count = total_frames.div_ceil(LEDGER_CHUNK_FRAMES);
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(chunk_count).map_err(|_| ())?;
        chunks.resize_with(chunk_count, spin::Once::new);
        Ok(Self {
            chunks: Vec::leak(chunks),
            total_frames,
            initial_frontier,
        })
    }

    fn ensure_chunk(&self, index: usize) -> Result<&'static [AtomicU32], ()> {
        if index >= self.total_frames {
            return Err(());
        }

        let chunk_index = index / LEDGER_CHUNK_FRAMES;
        if let Some(slots) = self.chunks[chunk_index].get() {
            return Ok(*slots);
        }

        // Build outside spin::Once. A nested allocation can therefore never
        // spin on an initializer that the interrupted context must finish.
        let start = chunk_index * LEDGER_CHUNK_FRAMES;
        let end = (start + LEDGER_CHUNK_FRAMES).min(self.total_frames);
        let mut slots = Vec::new();
        slots.try_reserve_exact(end - start).map_err(|_| ())?;
        let mut slot_index = start;
        slots.resize_with(end - start, || {
            let state = if slot_index < self.initial_frontier {
                (1 << 2) | ST_ALLOCATED
            } else {
                ST_NEVER
            };
            slot_index += 1;
            AtomicU32::new(state)
        });

        let mut prepared = Some(slots);
        let published = self.chunks[chunk_index].call_once(|| {
            Vec::leak(prepared.take().expect("prepared frame-ledger chunk"))
                as &'static [AtomicU32]
        });
        Ok(*published)
    }

    fn get(&self, index: usize) -> Option<&AtomicU32> {
        if index >= self.total_frames {
            return None;
        }
        self.chunks[index / LEDGER_CHUNK_FRAMES]
            .get()
            .and_then(|slots| slots.get(index % LEDGER_CHUNK_FRAMES))
    }
}

static FRAME_LEDGER: spin::Once<FrameLedger> = spin::Once::new();
static FREE_FRAME_CAPACITY: AtomicUsize = AtomicUsize::new(0);
const BOOTSTRAP_FREE_CAPACITY: usize = 64;

struct BootstrapFreeFrames {
    addresses: [u64; BOOTSTRAP_FREE_CAPACITY],
    len: usize,
}

static BOOTSTRAP_FREE_FRAMES: Mutex<BootstrapFreeFrames> = Mutex::new(BootstrapFreeFrames {
    addresses: [0; BOOTSTRAP_FREE_CAPACITY],
    len: 0,
});

/// Unforgeable authority to return one allocator-issued generation.
#[derive(Clone, Copy)]
struct FrameLease {
    frame: PhysFrame,
    index: u32,
    generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReturnOutcome {
    Returned,
    LostContended,
    RefusedDoubleRelease,
    RefusedStale,
    RefusedNeverAllocated,
    RefusedUntracked,
}

/// Free list for deallocated frames
/// When frames are deallocated (e.g., after CoW copy reduces refcount to 0),
/// they are added to this list for reuse. Once the ledger is published, its
/// capacity is prepared before sequential frames become visible and returns
/// refuse at the exact boundary rather than reallocating.
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

    /// Get the nth usable frame (lock-free after initialization)
    fn get_usable_frame(n: usize) -> Option<PhysFrame> {
        let info = MEMORY_INFO.get()?;

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

/// Inverse of `get_usable_frame`: map an aligned frame in a usable region to
/// the allocator ordinal used by the ledger.
fn frame_ordinal(frame: PhysFrame) -> Option<usize> {
    let info = MEMORY_INFO.get()?;
    let address = frame.start_address().as_u64();
    let mut ordinal = 0usize;
    for region in info.regions[..info.region_count].iter().flatten() {
        if (region.start..region.end).contains(&address) {
            return Some(ordinal + ((address - region.start) / 4096) as usize);
        }
        ordinal += ((region.end - region.start) / 4096) as usize;
    }
    None
}

fn seed_free_frame(ledger: &FrameLedger, frame: PhysFrame) -> bool {
    let Some(index) = frame_ordinal(frame) else {
        crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
        return false;
    };
    let Ok(slots) = ledger.ensure_chunk(index) else {
        crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
        return false;
    };
    let slot = &slots[index % LEDGER_CHUNK_FRAMES];
    if slot.load(Ordering::Relaxed) & STATE_MASK == ST_FREE {
        crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_DOUBLE.increment();
        return false;
    }
    slot.store((1 << 2) | ST_FREE, Ordering::Relaxed);
    true
}

/// Build and seed the generation ledger after the heap exists but before any
/// process page table can be constructed. This is a quiescent boot operation.
pub fn init_frame_ledger() {
    if FRAME_LEDGER.get().is_some() {
        return;
    }

    let info = MEMORY_INFO.get().expect("frame allocator not initialized");
    let advertised_frames = info.regions[..info.region_count]
        .iter()
        .flatten()
        .map(|region| ((region.end - region.start) / 4096) as usize)
        .sum::<usize>();
    // u32::MAX is reserved as the untracked sentinel. Oversized firmware maps
    // stop exposing frames at this ceiling instead of aborting boot.
    let total_frames = advertised_frames.min(MAX_TRACKED_FRAMES);
    let frontier_snapshot = NEXT_FREE_FRAME.load(Ordering::Acquire);
    let frontier = frontier_snapshot.min(total_frames);
    let ledger = FrameLedger::try_new(total_frames, frontier)
        .expect("frame ledger chunk directory allocation failed");

    let mut chunk_start = 0;
    while chunk_start < frontier {
        ledger
            .ensure_chunk(chunk_start)
            .expect("frame ledger bootstrap chunk allocation failed");
        chunk_start += LEDGER_CHUNK_FRAMES;
    }

    // The heap exists now, so reserve the entire published frontier before the
    // ledger makes allocation-free returns mandatory.
    {
        let mut bootstrap = BOOTSTRAP_FREE_FRAMES.lock();
        let mut free_list = FREE_FRAMES.lock();
        let required = frontier.saturating_add(bootstrap.len);
        if free_list.capacity() < required {
            let additional = required.saturating_sub(free_list.len());
            free_list
                .try_reserve(additional)
                .expect("frame return capacity allocation failed");
        }
        // Frontier slots were initialized as allocated. Bootstrap returns are
        // applied afterwards so an already-free frame is not overwritten.
        let mut free_index = 0;
        while free_index < free_list.len() {
            let frame = free_list[free_index];
            if seed_free_frame(&ledger, frame) {
                free_index += 1;
            } else {
                free_list.swap_remove(free_index);
            }
        }
        for address in bootstrap.addresses[..bootstrap.len].iter().copied() {
            let frame = PhysFrame::containing_address(PhysAddr::new(address));
            if seed_free_frame(&ledger, frame) {
                free_list.push(frame);
            }
        }
        bootstrap.len = 0;
        FREE_FRAME_CAPACITY.store(free_list.capacity(), Ordering::Release);
    }

    // No allocation may race the snapshot used to seed ST_ALLOCATED/ST_NEVER.
    assert_eq!(
        NEXT_FREE_FRAME.load(Ordering::Acquire),
        frontier_snapshot
    );
    FRAME_LEDGER.call_once(|| ledger);
}

enum PrepareFrame {
    Ready,
    Contended,
    Exhausted,
}

fn ensure_free_frame_capacity(required: usize) -> PrepareFrame {
    if FREE_FRAME_CAPACITY.load(Ordering::Acquire) >= required {
        return PrepareFrame::Ready;
    }
    let Some(mut free_list) = FREE_FRAMES.try_lock() else {
        return PrepareFrame::Contended;
    };
    if free_list.capacity() < required {
        let additional = required.saturating_sub(free_list.len());
        if free_list.try_reserve(additional).is_err() {
            return PrepareFrame::Exhausted;
        }
    }
    FREE_FRAME_CAPACITY.store(free_list.capacity(), Ordering::Release);
    PrepareFrame::Ready
}

fn prepare_frame_for_allocation(index: usize) -> PrepareFrame {
    let Some(ledger) = FRAME_LEDGER.get() else {
        return PrepareFrame::Ready;
    };
    if ledger.ensure_chunk(index).is_err() {
        return PrepareFrame::Exhausted;
    }
    ensure_free_frame_capacity(index + 1)
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        // Use compare-exchange loop to avoid wasting frame slots on failure
        let mut capacity_contentions = 0u8;
        loop {
            let current = NEXT_FREE_FRAME.load(Ordering::SeqCst);
            log::trace!("Frame allocator: Attempting to allocate frame #{}", current);

            // Try to get the frame at this index
            let Some(frame) = Self::get_usable_frame(current) else {
                // No more frames available - don't increment counter
                return None;
            };

            match prepare_frame_for_allocation(current) {
                PrepareFrame::Ready => capacity_contentions = 0,
                PrepareFrame::Contended if capacity_contentions < 8 => {
                    capacity_contentions += 1;
                    core::hint::spin_loop();
                    continue;
                }
                // A transient free-list lock holder must not manufacture OOM.
                // Publish after bounded retries; a future exact-boundary return
                // is refused as a counted loss instead of reallocating.
                PrepareFrame::Contended => {}
                PrepareFrame::Exhausted => return None,
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
                    log::trace!(
                        "Frame allocator: Allocated frame {:#x} (allocation #{})",
                        frame.start_address().as_u64(),
                        current
                    );
                    return Some(frame);
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
                    region.start,
                    region.end,
                    LOW_MEMORY_FLOOR
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
                        region.start,
                        LOW_MEMORY_FLOOR
                    );
                    LOW_MEMORY_FLOOR
                } else {
                    region.start
                };

                debug_assert_eq!(adjusted_start % 4096, 0);
                debug_assert_eq!(region.end % 4096, 0);

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

    // Store the extracted information (once, immutable)
    MEMORY_INFO.call_once(|| MemoryInfo {
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
    debug_assert_eq!(aligned_start % 4096, 0);
    debug_assert_eq!(end % 4096, 0);

    regions[0] = Some(UsableRegion {
        start: aligned_start,
        end,
    });

    let total_memory = end - aligned_start;

    MEMORY_INFO.call_once(|| MemoryInfo {
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
fn allocate_candidate() -> Option<PhysFrame> {
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

enum ClaimError {
    Duplicate,
    Untracked,
}

fn claim_frame(frame: PhysFrame) -> Result<Option<FrameLease>, ClaimError> {
    let Some(ledger) = FRAME_LEDGER.get() else {
        return Ok(None);
    };
    let Some(index) = frame_ordinal(frame) else {
        crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
        return Err(ClaimError::Untracked);
    };
    let Some(slot) = ledger.get(index) else {
        crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
        return Err(ClaimError::Untracked);
    };

    loop {
        let observed = slot.load(Ordering::Acquire);
        match observed & STATE_MASK {
            ST_NEVER | ST_FREE => {
                let generation = (observed >> 2).wrapping_add(1) & 0x3fff_ffff;
                let allocated = (generation << 2) | ST_ALLOCATED;
                if slot
                    .compare_exchange(observed, allocated, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(Some(FrameLease {
                        frame,
                        index: index as u32,
                        generation,
                    }));
                }
            }
            ST_ALLOCATED => {
                crate::tracing::providers::teardown::FRAME_DUPLICATE_ALLOC_REFUSED.increment();
                return Err(ClaimError::Duplicate);
            }
            _ => {
                crate::tracing::providers::teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
                return Err(ClaimError::Untracked);
            }
        }
    }
}

fn allocate_claimed() -> Option<(PhysFrame, Option<FrameLease>)> {
    loop {
        let frame = allocate_candidate()?;
        match claim_frame(frame) {
            Ok(lease) => return Some((frame, lease)),
            Err(ClaimError::Duplicate | ClaimError::Untracked) => continue,
        }
    }
}

pub fn allocate_frame() -> Option<PhysFrame> {
    allocate_claimed().map(|(frame, _)| frame)
}

#[cfg(feature = "boot_tests")]
fn allocate_frame_leased() -> Option<FrameLease> {
    assert!(FRAME_LEDGER.get().is_some(), "frame ledger not initialized");
    allocate_claimed().and_then(|(_, lease)| lease)
}

fn counted(outcome: ReturnOutcome) -> ReturnOutcome {
    use crate::tracing::providers::teardown;
    match outcome {
        ReturnOutcome::LostContended => teardown::FRAME_LOST_CONTENDED.increment(),
        ReturnOutcome::RefusedDoubleRelease => teardown::FRAME_RETURN_REFUSED_DOUBLE.increment(),
        ReturnOutcome::RefusedStale => teardown::FRAME_RETURN_REFUSED_STALE.increment(),
        ReturnOutcome::RefusedNeverAllocated => {
            teardown::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.increment()
        }
        ReturnOutcome::RefusedUntracked => teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment(),
        ReturnOutcome::Returned => {}
    }
    outcome
}

fn return_lease(lease: FrameLease) -> ReturnOutcome {
    let Some(ledger) = FRAME_LEDGER.get() else {
        let Some(mut bootstrap) = BOOTSTRAP_FREE_FRAMES.try_lock() else {
            return counted(ReturnOutcome::LostContended);
        };
        if bootstrap.len == BOOTSTRAP_FREE_CAPACITY {
            return counted(ReturnOutcome::LostContended);
        }
        let index = bootstrap.len;
        bootstrap.addresses[index] = lease.frame.start_address().as_u64();
        bootstrap.len += 1;
        return ReturnOutcome::Returned;
    };
    let lease_index = lease.index as usize;
    if frame_ordinal(lease.frame) != Some(lease_index) {
        return counted(ReturnOutcome::RefusedUntracked);
    }
    let Some(slot) = ledger.get(lease_index) else {
        return counted(if lease_index < ledger.total_frames {
            ReturnOutcome::RefusedNeverAllocated
        } else {
            ReturnOutcome::RefusedUntracked
        });
    };

    loop {
        let observed = slot.load(Ordering::Acquire);
        if observed & STATE_MASK == ST_NEVER {
            return counted(ReturnOutcome::RefusedNeverAllocated);
        }
        if observed >> 2 != lease.generation {
            return counted(ReturnOutcome::RefusedStale);
        }
        match observed & STATE_MASK {
            ST_ALLOCATED => {
                let free = (observed & !STATE_MASK) | ST_FREE;
                if slot
                    .compare_exchange(observed, free, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
            ST_FREE => return counted(ReturnOutcome::RefusedDoubleRelease),
            _ => return counted(ReturnOutcome::RefusedUntracked),
        }
    }

    if let Some(mut free_list) = FREE_FRAMES.try_lock() {
        // This explicit boundary check is the release-build proof that push
        // cannot grow the Vec on any ledger-backed return path.
        if free_list.len() == free_list.capacity() {
            return counted(ReturnOutcome::LostContended);
        }
        free_list.push(lease.frame);
        ReturnOutcome::Returned
    } else {
        counted(ReturnOutcome::LostContended)
    }
}

/// Deallocate a physical frame, returning it to the free pool
///
/// Before ledger publication bootstrap callers use a fixed, allocation-free
/// staging array that is transferred into the free list during ledger init.
/// Afterwards every return is fail-closed through the generation transition;
/// a refusal leaks rather than risking reuse by two owners.
pub fn deallocate_frame(frame: PhysFrame) {
    // Don't deallocate frames below the low memory floor
    if frame.start_address().as_u64() < LOW_MEMORY_FLOOR {
        log::warn!(
            "Refusing to deallocate frame {:#x} below low memory floor",
            frame.start_address().as_u64()
        );
        return;
    }

    let index = frame_ordinal(frame).unwrap_or(usize::MAX);
    let generation = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(index))
        .map(|slot| slot.load(Ordering::Acquire) >> 2)
        .unwrap_or(0);
    let lease = FrameLease {
        frame,
        index: u32::try_from(index).unwrap_or(u32::MAX),
        generation,
    };
    match return_lease(lease) {
        ReturnOutcome::Returned => log::trace!(
            "Frame allocator: Deallocated frame {:#x}",
            frame.start_address().as_u64()
        ),
        ReturnOutcome::LostContended => log::warn!(
            "Frame allocator: Could not deallocate frame {:#x} - lock contention",
            frame.start_address().as_u64()
        ),
        _ => {}
    }
}

#[cfg(feature = "boot_tests")]
#[path = "frame_allocator_tests.rs"]
mod boot_tests;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub use boot_tests::run_x86_frame_custody_gate;
#[cfg(feature = "boot_tests")]
pub use boot_tests::{frame_custody_healthy_counters_test, frame_custody_refusal_gate_test};

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
    // Calculate total memory from MEMORY_INFO regions (lock-free read)
    let total_bytes = if let Some(info) = MEMORY_INFO.get() {
        let mut total = 0u64;
        for i in 0..info.region_count {
            if let Some(region) = info.regions[i] {
                total += region.end - region.start;
            }
        }
        total
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
