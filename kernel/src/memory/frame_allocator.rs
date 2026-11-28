#[path = "contiguous.rs"]
mod contiguous;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{FrameAllocator, PhysAddr, PhysFrame, Size4KiB};
use alloc::vec::Vec;
use super::boot_ranges::RangeSet;
use super::frame_meta::{ClaimError, Decref, FrameMeta, FrameMetaMap, FreeList, LEAF_EXTERNAL, ST_ALLOCATED, ST_FREE};
pub(crate) use super::frame_meta::{LeafMappingClass, ReturnOutcome};
#[cfg(target_arch = "x86_64")]
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::panic::Location;
#[cfg(feature = "testing")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::PhysAddr;

/// Whether a physical address belongs to allocator-managed RAM.
#[cfg(target_arch = "x86_64")]
pub(crate) fn is_usable_address(address: u64) -> bool {
    MEMORY_INFO.get().is_some_and(|info| {
        info.iter()
            .any(|region| address >= region.start && address < region.end)
    })
}

/// Low memory floor - we never allocate frames below 1MiB
/// This avoids issues with:
/// - Frame 0x0 (null pointer confusion)
/// - BIOS/firmware reserved areas
/// - Legacy device memory (VGA, etc)
const LOW_MEMORY_FLOOR: u64 = 0x100000; // 1 MiB

/// Pages the x86 application-processor startup trampoline needs below 1 MiB:
/// its code page and the three page-table pages of its identity map.
#[cfg(target_arch = "x86_64")]
pub const AP_TRAMPOLINE_PAGES: u64 = 4;

/// Base of a run of `AP_TRAMPOLINE_PAGES` usable pages below 1 MiB, or 0 when
/// the firmware reported none. A startup IPI can only start a processor at a
/// page below 1 MiB, and the allocator never hands out pages below its floor,
/// so nothing else in the kernel uses this run.
#[cfg(target_arch = "x86_64")]
static AP_TRAMPOLINE_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The low run recorded for the x86 application-processor trampoline.
#[cfg(target_arch = "x86_64")]
pub fn ap_trampoline_base() -> Option<u64> {
    match AP_TRAMPOLINE_BASE.load(Ordering::Acquire) {
        0 => None,
        base => Some(base),
    }
}

/// Immutable boot ranges, published before the first frame allocation.
static MEMORY_INFO: spin::Once<&'static RangeSet> = spin::Once::new();
#[cfg(target_arch = "x86_64")]
static X86_BOOT_RANGES: spin::Once<(RangeSet, FrameMetaMap)> = spin::Once::new();
static NEXT_FREE_FRAME: AtomicUsize = AtomicUsize::new(0);

/// Per-frame metadata, carved from the boot ranges and published with them,
/// before the first frame allocation and before the heap exists.
static FRAME_META: spin::Once<&'static FrameMetaMap> = spin::Once::new();

#[inline]
fn frame_meta() -> Option<&'static FrameMetaMap> {
    FRAME_META.get().copied()
}

#[inline]
fn pfn(frame: PhysFrame) -> u64 {
    frame.start_address().as_u64() / 4096
}

#[inline]
fn frame_at(pfn: u64) -> PhysFrame {
    PhysFrame::containing_address(PhysAddr::new(pfn * 4096))
}

/// The code that allocated each frame: one entry per caller location of the
/// allocation functions (`#[track_caller]`), so the census in
/// `/proc/breenix/frames` can name the code that owns an allocated frame.
const FRAME_SITE_SLOTS: usize = 256;
static FRAME_SITES: [AtomicPtr<Location<'static>>; FRAME_SITE_SLOTS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; FRAME_SITE_SLOTS];

/// The site number of `location`, entered in `FRAME_SITES` on first use; 0
/// when the table is full. Lock-free, so any allocation context may call it.
fn frame_site(location: &'static Location<'static>) -> u16 {
    let wanted = location as *const Location<'static> as *mut Location<'static>;
    let mut index = ((wanted as usize as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as usize;
    for _ in 0..FRAME_SITE_SLOTS {
        let entry = &FRAME_SITES[index];
        let current = entry.load(Ordering::Acquire);
        if current == wanted {
            return index as u16 + 1;
        }
        if current.is_null() {
            match entry.compare_exchange(
                core::ptr::null_mut(),
                wanted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return index as u16 + 1,
                Err(other) if other == wanted => return index as u16 + 1,
                Err(_) => {}
            }
        }
        index = (index + 1) % FRAME_SITE_SLOTS;
    }
    0
}

/// Unforgeable authority to return one allocator-issued generation.
#[derive(Clone, Copy)]
pub(crate) struct FrameLease {
    frame: PhysFrame,
    generation: u32,
}

impl FrameLease {
    pub(crate) fn frame(self) -> PhysFrame {
        self.frame
    }
}

/// Free list for deallocated frames, threaded through their metadata. A
/// return needs no capacity, so a returned frame is linked, not dropped
/// (`a_return_at_exact_free_list_capacity_is_recorded`). Interrupts are masked
/// for the hold, so a holder is not interrupted or switched out with it held.
static FREE_FRAMES: Mutex<FreeList> = Mutex::new(FreeList::new());

fn with_free_frames<R>(f: impl FnOnce(&mut FreeList) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut FREE_FRAMES.lock()))
}

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
        for region in info.iter() {
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
        None
    }
}

/// Register allocator-backed memory that remains owned outside process page
/// tables. Mapping code does not choose this class: the owning allocator site
/// marks the frame before any user descriptor can be published.
pub(crate) fn register_external_leaf_frame(frame: PhysFrame) -> Result<(), &'static str> {
    let Some(meta) = frame_meta() else {
        return Err("frame metadata not initialized");
    };
    if meta.mark_external(pfn(frame))? {
        Ok(())
    } else {
        Err("external leaf frame is outside allocator memory")
    }
}

/// Return a frame `register_external_leaf_frame` marked. Its owner calls this
/// once the process page tables that mapped the frame are gone: a table drops
/// an external leaf's mapping without returning the frame.
pub(crate) fn release_external_leaf_frame(frame: PhysFrame) -> ReturnOutcome {
    let cleared = frame_meta().map_or(Err(ReturnOutcome::RefusedUntracked), |meta| {
        meta.clear_external(pfn(frame))
    });
    if let Err(outcome) = cleared {
        return counted(outcome);
    }
    return_lease(current_lease_for_frame(frame))
}

/// Derive one mapping reference from allocator state before its descriptor is
/// published. The caller must roll the reference back if mapping fails.
pub(crate) fn acquire_leaf_mapping(frame: PhysFrame) -> Result<LeafMappingClass, &'static str> {
    let result = frame_meta()
        .ok_or("frame metadata not initialized")
        .and_then(|meta| meta.acquire_leaf(pfn(frame)));
    if result.is_err() {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
    }
    result
}

/// Release one allocator-derived mapping reference. Unregistered, external,
/// or non-live frames fail closed and can never be returned by this operation.
pub(crate) fn decref_leaf_mapping(frame: PhysFrame) -> bool {
    match frame_meta().map_or(Decref::Refused, |meta| meta.decref_leaf(pfn(frame))) {
        Decref::Last => true,
        Decref::Remaining | Decref::External => false,
        Decref::Refused => {
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_DECREF_UNREGISTERED);
            false
        }
    }
}

pub(crate) fn leaf_mapping_refcount(frame: PhysFrame) -> u32 {
    frame_meta().map_or(0, |meta| meta.leaf_refs(pfn(frame)))
}

pub(crate) fn leaf_mapping_stats() -> (usize, u64) {
    let Some(meta) = frame_meta() else {
        return (0, 0);
    };
    let mut tracked = 0usize;
    let mut total_refs = 0u64;
    meta.for_each(|_, entry| {
        let refs = entry.leaf_refs();
        if refs != 0 && refs != LEAF_EXTERNAL {
            tracked += 1;
            total_refs += refs as u64;
        }
    });
    (tracked, total_refs)
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        // Use compare-exchange loop to avoid wasting frame slots on failure
        loop {
            let current = NEXT_FREE_FRAME.load(Ordering::SeqCst);
            log::trace!("Frame allocator: Attempting to allocate frame #{}", current);

            // Try to get the frame at this index
            let Some(frame) = Self::get_usable_frame(current) else {
                // No more frames available - don't increment counter
                return None;
            };

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

/// Initialize the global frame allocator from the x86 bootloader's complete
/// map, carving the frame metadata out of it before anything can allocate.
#[cfg(target_arch = "x86_64")]
pub fn init(memory_regions: &'static MemoryRegions) {
    use super::boot_ranges::{from_x86, RangeStorage};
    use arm64_boot_contract::phys_memory::PhysRange;
    use core::ptr::NonNull;

    struct DirectMap(u64);

    // The bootloader maps the usable ranges writable, and the builders
    // reserve each page before the allocator can see it.
    unsafe impl RangeStorage for DirectMap {
        fn storage_page(&self, phys: u64) -> Option<NonNull<u8>> {
            NonNull::new(self.0.checked_add(phys)? as *mut u8)
        }
    }

    let usable = || memory_regions.iter().filter(|r| r.kind == MemoryRegionKind::Usable);
    for region in usable() {
        let Some(start) = region.start.max(0x1000).checked_next_multiple_of(4096) else {
            continue;
        };
        let end = region.end.min(0xA0000) & !0xfff;
        if start <= end && end - start >= AP_TRAMPOLINE_PAGES * 4096 {
            AP_TRAMPOLINE_BASE.store(end - AP_TRAMPOLINE_PAGES * 4096, Ordering::Release);
        }
    }

    let (ranges, meta) = X86_BOOT_RANGES.call_once(|| {
        let storage = DirectMap(super::physical_memory_offset().as_u64());
        let mut ranges = from_x86(&storage, usable().map(|r| PhysRange::new(r.start, r.end)))
            .unwrap_or_else(|error| panic!("boot: {}", error.as_str()));
        let meta = super::frame_meta::build(&mut ranges, &storage)
            .unwrap_or_else(|error| panic!("boot: frame metadata: {}", error.as_str()));
        (ranges, meta)
    });
    publish(ranges, meta);
    let stats = meta.stats();
    log::info!(
        "Frame allocator initialized with {} MiB of usable memory in {} regions (floor={:#x})",
        ranges.total_bytes() >> 20, ranges.len(), LOW_MEMORY_FLOOR
    );
    log::info!(
        "Frame metadata: metadata_pages={} sections={} descriptor_pages={} leaf_pages={} root_pages={} spare_pages={}",
        stats.metadata_pages, stats.sections, stats.descriptor_pages, stats.leaf_pages,
        stats.root_pages, stats.spare_pages
    );
}

/// Use the same immutable ranges and metadata the ARM64 boot path built
/// before the heap.
#[cfg(target_arch = "aarch64")]
pub fn init_aarch64(ranges: &'static RangeSet, meta: &'static FrameMetaMap) {
    publish(ranges, meta);
}

fn publish(ranges: &'static RangeSet, meta: &'static FrameMetaMap) {
    FRAME_META.call_once(|| meta);
    MEMORY_INFO.call_once(|| ranges);
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
///
/// The slab layer's pages pass `simulated_oom_applies` false (see
/// `allocate_slab_frame`).
fn allocate_candidate(simulated_oom_applies: bool) -> Option<PhysFrame> {
    // Test-only: simulate OOM if flag is set
    if simulated_oom_applies && simulated_oom() {
        log::trace!("Frame allocator: OOM simulation active, returning None");
        return None;
    }

    // Try to reuse a frame from the free list (all architectures).
    if let Some(meta) = frame_meta() {
        if let Some(pfn) = with_free_frames(|free_list| free_list.pop(meta)) {
            return Some(frame_at(pfn));
        }
    }

    // Fall back to sequential allocation from memory map
    let mut allocator = BootInfoFrameAllocator::new();
    allocator.allocate_frame()
}

/// Claim `frame`'s next generation for an allocation at `site`.
fn claim_frame(frame: PhysFrame, site: u16, owner: u8) -> Option<FrameLease> {
    use crate::tracing::providers::teardown;
    let Some(meta) = frame_meta() else {
        teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
        return None;
    };
    match meta.claim(pfn(frame), site, owner) {
        Ok(generation) => Some(FrameLease { frame, generation }),
        Err(ClaimError::LiveLeaf) => {
            crate::trace_count!(teardown::LEAF_CUSTODY_REFUSED);
            None
        }
        Err(ClaimError::Allocated) => {
            teardown::FRAME_DUPLICATE_ALLOC_REFUSED.increment();
            None
        }
        Err(ClaimError::Untracked) => {
            teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment();
            None
        }
    }
}

/// Whether the testing build's simulated frame exhaustion is on.
fn simulated_oom() -> bool {
    #[cfg(feature = "testing")]
    return SIMULATE_OOM.load(Ordering::SeqCst);
    #[cfg(not(feature = "testing"))]
    return false;
}

/// A candidate the metadata refuses to claim is skipped: it is live, mapped
/// or untracked, so handing it out could give it two owners.
fn allocate_claimed(site: u16, owner: u8) -> Option<FrameLease> {
    claimed_from(true, site, owner)
}

fn claimed_from(simulated_oom_applies: bool, site: u16, owner: u8) -> Option<FrameLease> {
    loop {
        let frame = allocate_candidate(simulated_oom_applies)?;
        if let Some(lease) = claim_frame(frame, site, owner) {
            return Some(lease);
        }
    }
}

/// A frame for a slab page. The testing build's simulated frame exhaustion
/// (`enable_oom_simulation`) models user pages running out, and the fixed
/// heap the slab layer replaces did not see it, so this path does not either.
/// No logging, formatting or allocation: the slab layer calls it with its
/// cache lock held and interrupts masked.
#[track_caller]
pub(crate) fn allocate_slab_frame() -> Option<PhysFrame> {
    claimed_from(false, frame_site(Location::caller()), 0).map(FrameLease::frame)
}

#[track_caller]
pub fn allocate_frame() -> Option<PhysFrame> {
    allocate_claimed(frame_site(Location::caller()), 0).map(FrameLease::frame)
}

/// Reserve `count` physically contiguous frames, writing them in ascending
/// address order into `out[..count]`, and return the base frame.
///
/// DMA rings (VirtIO virtqueues) are published to the device as a single
/// physical address covering the whole ring, so their frames must be genuinely
/// adjacent in physical memory. The free list recycles frames in arbitrary
/// order, so adjacency can only be established at the sequential frontier:
/// this reserves a run of consecutive allocator ordinals whose physical
/// addresses are checked adjacent, and claims each of them in the metadata
/// exactly as a single-frame allocation does. Frames reserved but not
/// usable (a run cut short by a usable-region boundary) are handed back
/// through the normal fail-closed return path, never dropped on the floor.
/// claim-lint:ok: tests/frame_allocator_contiguous.rs releases the claimed prefix in 2 of its 3 cases.
///
/// Fail-closed: returns `None` rather than a run that is not contiguous. A
/// caller that cannot get real contiguity must fail its own initialization.
#[track_caller]
pub fn allocate_contiguous_frames(
    count: usize,
    out: &mut [Option<PhysFrame>],
) -> Option<PhysFrame> {
    if count == 0 || out.len() < count {
        return None;
    }

    // Test-only: simulate OOM if flag is set
    #[cfg(feature = "testing")]
    if SIMULATE_OOM.load(Ordering::SeqCst) {
        return None;
    }

    let site = frame_site(Location::caller());
    contiguous::allocate(
        count,
        out,
        &NEXT_FREE_FRAME,
        BootInfoFrameAllocator::get_usable_frame,
        |frame| frame.start_address().as_u64(),
        // The metadata of the usable frames is built at boot, before the
        // first allocation, so a run has no capacity to prepare.
        |_, _| true,
        |frame| claim_frame(frame, site, 0).is_some(),
        deallocate_frame,
    )
}

#[track_caller]
pub(crate) fn allocate_frame_leased() -> Option<FrameLease> {
    allocate_claimed(frame_site(Location::caller()), 0)
}

fn counted(outcome: ReturnOutcome) -> ReturnOutcome {
    use crate::tracing::providers::teardown;
    match outcome {
        ReturnOutcome::RefusedDoubleRelease => teardown::FRAME_RETURN_REFUSED_DOUBLE.increment(),
        ReturnOutcome::RefusedStale => teardown::FRAME_RETURN_REFUSED_STALE.increment(),
        ReturnOutcome::RefusedNeverAllocated => {
            teardown::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.increment()
        }
        ReturnOutcome::RefusedUntracked => teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment(),
        ReturnOutcome::RefusedLiveLeaf => teardown::FRAME_RETURN_REFUSED_LIVE_LEAF.increment(),
        ReturnOutcome::Returned => {}
    }
    outcome
}

pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome {
    let Some(meta) = frame_meta() else {
        return counted(ReturnOutcome::RefusedUntracked);
    };
    let frame = pfn(lease.frame);
    let outcome = meta.retire(frame, lease.generation);
    if outcome == ReturnOutcome::Returned {
        // The transition succeeded, so the frame has an entry to link.
        with_free_frames(|free_list| free_list.push(meta, frame));
    }
    counted(outcome)
}

fn current_lease_for_frame(frame: PhysFrame) -> FrameLease {
    FrameLease {
        frame,
        generation: frame_meta().map_or(0, |meta| meta.current_generation(pfn(frame))),
    }
}

/// The entry of an allocated frame the slab layer holds, for its slab record.
pub(crate) fn frame_entry(frame: PhysFrame) -> Option<&'static FrameMeta> {
    frame_meta()?.entry(pfn(frame))
}

/// Return a slab page's frame. No logging, formatting or allocation: the
/// slab layer returns pages with its cache lock held and interrupts masked.
pub(crate) fn deallocate_slab_frame(frame: PhysFrame) -> ReturnOutcome {
    return_lease(current_lease_for_frame(frame))
}

/// Return a leaf whose mapping refcount reached zero without adding logging,
/// formatting, allocation, or blocking lock acquisition to the drain path.
pub(crate) fn deallocate_leaf_frame(frame: PhysFrame) -> ReturnOutcome {
    return_lease(current_lease_for_frame(frame))
}

/// Deallocate a physical frame, returning it to the free pool
///
/// A return goes through the generation transition and fails closed; a
/// refusal leaks rather than risking reuse by two owners.
pub fn deallocate_frame(frame: PhysFrame) {
    // Don't deallocate frames below the low memory floor
    if frame.start_address().as_u64() < LOW_MEMORY_FLOOR {
        log::warn!(
            "Refusing to deallocate frame {:#x} below low memory floor",
            frame.start_address().as_u64()
        );
        return;
    }

    if return_lease(current_lease_for_frame(frame)) == ReturnOutcome::Returned {
        log::trace!(
            "Frame allocator: Deallocated frame {:#x}",
            frame.start_address().as_u64()
        );
    }
}

#[cfg(feature = "boot_tests")]
#[path = "frame_allocator_tests.rs"]
mod boot_tests;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub use boot_tests::run_x86_frame_custody_gate;
#[cfg(feature = "boot_tests")]
pub use boot_tests::{
    frame_custody_healthy_counters_test, frame_custody_refusal_gate_test, free_list_len_for_gate,
};

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
    let total_bytes = MEMORY_INFO.get().map_or(0, |info| info.total_bytes());

    let allocated_frames = NEXT_FREE_FRAME.load(Ordering::Relaxed);

    let free_list_frames = with_free_frames(|free_list| free_list.len());

    MemoryStats {
        total_bytes,
        allocated_frames,
        free_list_frames,
    }
}

/// The frames the frontier has handed out, read without the free list's lock,
/// for the OOM record, which may run while this CPU or another holds it.
pub fn frontier_frames() -> usize {
    NEXT_FREE_FRAME.load(Ordering::Relaxed)
}

/// What holds an allocated frame, for the census's `owner` rows: the owners a
/// suite's memory record breaks its frames down by (docs/suites/README.md,
/// "Memory record").
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameOwner {
    /// A process's stack pages, as exec and process creation map them.
    UserStacks = 1,
    /// Other pages mapped into processes: program, heap, anonymous and shared
    /// memory, copy-on-write copies.
    UserPages,
    /// Page-table frames, a process's and the kernel's.
    PageTables,
    /// File pages the per-inode cache holds (`memory::file_map`).
    PageCache,
    /// Kernel and per-CPU stacks.
    KernelStacks,
    /// Device and DMA buffers, and the display's.
    Buffers,
    /// Everything else, the kernel heap's growth among it.
    Other,
}

impl FrameOwner {
    const ALL: [FrameOwner; 7] = [
        FrameOwner::UserStacks,
        FrameOwner::UserPages,
        FrameOwner::PageTables,
        FrameOwner::PageCache,
        FrameOwner::KernelStacks,
        FrameOwner::Buffers,
        FrameOwner::Other,
    ];

    fn name(self) -> &'static str {
        match self {
            FrameOwner::UserStacks => "user_stacks",
            FrameOwner::UserPages => "user_pages",
            FrameOwner::PageTables => "page_tables",
            FrameOwner::PageCache => "page_cache",
            FrameOwner::KernelStacks => "kernel_stacks",
            FrameOwner::Buffers => "buffers",
            FrameOwner::Other => "other",
        }
    }

    /// The owner of a frame allocated at `file` (as `short_source_path` names
    /// it) that a process page table maps (`mapped`) or does not. A frame
    /// `allocate_frame_for` tagged is that owner instead.
    fn of_site(file: &str, mapped: bool) -> FrameOwner {
        const STACKS: [&str; 3] = ["memory/kernel_stack.rs", "memory/per_cpu_stack.rs", "memory/stack.rs"];
        const BUFFERS: [&str; 4] = ["drivers/", "gpu/", "console/", "syscall/graphics.rs"];
        const TABLES: [&str; 3] = ["memory/process_memory.rs", "memory/kernel_page_table.rs", "/structures/paging/"];
        const USER: [&str; 7] = [
            "memory/anon_map.rs", "elf.rs", "process/fork.rs", "process/manager.rs", "syscall/memory",
            "ipc/shmem/", "interrupts.rs",
        ];
        if file == "memory/file_map.rs" {
            FrameOwner::PageCache
        } else if STACKS.contains(&file) {
            FrameOwner::KernelStacks
        } else if BUFFERS.iter().any(|prefix| file.starts_with(prefix)) {
            FrameOwner::Buffers
        } else if mapped {
            FrameOwner::UserPages
        } else if TABLES.iter().any(|part| file.contains(part)) {
            FrameOwner::PageTables
        } else if USER.iter().any(|part| file.contains(part)) {
            FrameOwner::UserPages
        } else {
            FrameOwner::Other
        }
    }

    fn from_tag(tag: u8) -> Option<FrameOwner> {
        FrameOwner::ALL.into_iter().find(|owner| *owner as u8 == tag)
    }
}

/// Classify a tracked resident mapping using the same allocation site as the
/// global census. The caller holds the address space stable for the walk.
pub(crate) fn resident_frame_owner(address: PhysAddr) -> Option<FrameOwner> {
    let entry = frame_meta()?.entry(address.as_u64() / 4096)?;
    if entry.state() != ST_ALLOCATED {
        return None;
    }
    if let Some(owner) = FrameOwner::from_tag(entry.owner()) {
        return Some(owner);
    }
    let site = (entry.site() as usize).checked_sub(1)?;
    // SAFETY: non-null site entries point to static source locations.
    let location = unsafe { FRAME_SITES[site].load(Ordering::Acquire).as_ref() }?;
    Some(FrameOwner::of_site(short_source_path(location.file()), true))
}

/// `allocate_frame`, recording what the caller's frames are for where the
/// census cannot tell it from the caller's source file.
#[track_caller]
pub fn allocate_frame_for(owner: FrameOwner) -> Option<PhysFrame> {
    allocate_claimed(frame_site(Location::caller()), owner as u8).map(FrameLease::frame)
}

/// Live allocator frames above the bootstrap direct-map window.
#[cfg(target_arch = "aarch64")]
pub(crate) fn allocated_high_frames() -> usize {
    use arm64_boot_contract::phys_memory::{DIRECT_MAP_RAM_END, PAGE_SIZE};

    let Some(meta) = frame_meta() else {
        return 0;
    };
    let mut allocated = 0;
    meta.for_each(|pfn, entry| {
        if pfn >= DIRECT_MAP_RAM_END / PAGE_SIZE && entry.state() == ST_ALLOCATED {
            allocated += 1;
        }
    });
    allocated
}

/// Allocated frames by the code that allocated them and by owner, and frames
/// lost to the allocator, read from the frame metadata (`/proc/breenix/frames`).
///
/// `used` is what `/proc/meminfo` counts as in use: the frames the sequential
/// allocator has handed out, less those on the free list. It is `allocated`
/// plus `lost`, frames the metadata records as free that the free list does
/// not hold yet (a return between its transition and its link). The walk is
/// not atomic against concurrent allocation, so the parts can disagree by the
/// frames allocated or returned while it ran.
pub fn frame_census() -> Result<alloc::string::String, i32> {
    use core::fmt::Write;
    const ENOMEM: i32 = -12;

    let Some(meta) = frame_meta() else {
        return Ok(alloc::string::String::from("ledger 0\n"));
    };
    // Per site (index 0 = unknown): frames, of which mapped by a process page
    // table (owned leaf references) and registered external leaves.
    let mut frames: Vec<[usize; 3]> = Vec::new();
    frames
        .try_reserve_exact(FRAME_SITE_SLOTS + 1)
        .map_err(|_| ENOMEM)?;
    frames.resize(FRAME_SITE_SLOTS + 1, [0; 3]);
    // Per site, the owner of its frames that no process page table maps and
    // of those one does, as `FrameOwner` discriminants.
    let mut site_owners = [[FrameOwner::Other as u8; 2]; FRAME_SITE_SLOTS + 1];
    for (slot, owners) in site_owners.iter_mut().enumerate().skip(1) {
        let location = FRAME_SITES[slot - 1].load(Ordering::Acquire);
        // SAFETY: entries are only ever set to `&'static Location`s.
        let file = unsafe { location.as_ref() }.map_or("unknown", |location| short_source_path(location.file()));
        for (mapped, owner) in owners.iter_mut().enumerate() {
            *owner = FrameOwner::of_site(file, mapped == 1) as u8;
        }
    }
    let mut by_owner = [0usize; FrameOwner::ALL.len() + 1];
    let (mut allocated, mut free) = (0usize, 0usize);
    meta.for_each(|_, entry| match entry.state() {
        ST_ALLOCATED => {
            allocated += 1;
            let index = entry.site() as usize;
            let site = &mut frames[index];
            site[0] += 1;
            let refs = entry.leaf_refs();
            match refs {
                0 => {}
                LEAF_EXTERNAL => site[2] += 1,
                _ => site[1] += 1,
            }
            let owner = match entry.owner() {
                0 => site_owners[index][usize::from(refs != 0)],
                tagged => tagged,
            };
            by_owner[owner as usize] += 1;
        }
        ST_FREE => free += 1,
        _ => {}
    });
    let free_list = memory_stats().free_list_frames;
    let frontier = NEXT_FREE_FRAME.load(Ordering::Relaxed);

    // One row per source location; a location's statics may be duplicated
    // across codegen units, so rows merge by file and line.
    let mut rows: Vec<(&'static str, u32, [usize; 3])> = Vec::new();
    rows.try_reserve_exact(FRAME_SITE_SLOTS + 1)
        .map_err(|_| ENOMEM)?;
    for (index, counts) in frames.iter().enumerate() {
        if counts[0] == 0 {
            continue;
        }
        let (file, line) = match index.checked_sub(1) {
            None => ("unknown", 0),
            Some(slot) => {
                let location = FRAME_SITES[slot].load(Ordering::Acquire);
                // SAFETY: entries are only ever set to `&'static Location`s.
                match unsafe { location.as_ref() } {
                    Some(location) => (location.file(), location.line()),
                    None => ("unknown", 0),
                }
            }
        };
        match rows.iter_mut().find(|row| row.0 == file && row.1 == line) {
            Some(row) => (0..3).for_each(|i| row.2[i] += counts[i]),
            None => rows.push((file, line, *counts)),
        }
    }
    rows.sort_unstable_by(|a, b| b.2[0].cmp(&a.2[0]));

    let mut text = alloc::string::String::new();
    text.try_reserve(64 + rows.len() * 80).map_err(|_| ENOMEM)?;
    let _ = write!(
        text,
        "used {}\nallocated {}\nlost {}\nfree_list {}\n",
        frontier.saturating_sub(free_list),
        allocated,
        free.saturating_sub(free_list),
        free_list
    );
    for owner in FrameOwner::ALL {
        let _ = writeln!(text, "owner {} {}", owner.name(), by_owner[owner as usize]);
    }
    for (file, line, counts) in rows {
        let _ = writeln!(
            text,
            "site {} mapped={} external={} {}:{}",
            counts[0],
            counts[1],
            counts[2],
            short_source_path(file),
            line
        );
    }
    Ok(text)
}

/// A kernel source path without `kernel/src/`, and a dependency's (absolute,
/// on the build host) as `<crate directory>/src/...`.
fn short_source_path(file: &'static str) -> &'static str {
    if let Some(kernel) = file.strip_prefix("kernel/src/") {
        return kernel;
    }
    if !file.starts_with('/') {
        return file;
    }
    let Some(src) = file.rfind("/src/") else {
        return file.rsplit('/').next().unwrap_or(file);
    };
    let crate_start = file[..src].rfind('/').map_or(0, |slash| slash + 1);
    &file[crate_start..]
}

/// A wrapper that allows using the global frame allocator with the mapper
pub struct GlobalFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocator {
    #[track_caller]
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        allocate_frame()
    }
}

/// Heap metadata retained by the physical allocator, sampled without
/// allocation: the frontier, then the free list's and the frame metadata's
/// heap bytes. Both are 0: the metadata and its free-list links are carved
/// from RAM at boot, outside the heap.
#[cfg(target_arch="x86_64")]
pub(crate) fn heap_footprint() -> [usize; 3] {
    [NEXT_FREE_FRAME.load(Ordering::Acquire), 0, 0]
}
