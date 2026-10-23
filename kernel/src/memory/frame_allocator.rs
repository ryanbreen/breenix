#[path = "contiguous.rs"]
mod contiguous;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{FrameAllocator, PhysAddr, PhysFrame, Size4KiB};
use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::panic::Location;
#[cfg(feature = "testing")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicPtr, AtomicU16, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::PhysAddr;

/// Maximum number of usable memory regions we support
/// Increased from 32 to 128 to handle UEFI's fragmented memory map
const MAX_REGIONS: usize = 128;

/// Whether a physical address belongs to allocator-managed RAM.
#[cfg(target_arch = "x86_64")]
pub(crate) fn is_usable_address(address: u64) -> bool {
    MEMORY_INFO.get().is_some_and(|info| {
        info.regions[..info.region_count]
            .iter()
            .flatten()
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
struct FrameLedgerSlot {
    allocation: AtomicU32,
    leaf_refs: AtomicU32,
    /// The allocation site of the current generation (`FRAME_SITES` index + 1),
    /// or 0 when unknown: frames allocated before the ledger, or a full table.
    site: AtomicU16,
}

impl FrameLedgerSlot {
    fn new(allocation: u32) -> Self {
        Self {
            allocation: AtomicU32::new(allocation),
            leaf_refs: AtomicU32::new(0),
            site: AtomicU16::new(0),
        }
    }

    fn load(&self, order: Ordering) -> u32 {
        self.allocation.load(order)
    }

    fn store(&self, value: u32, order: Ordering) {
        self.allocation.store(value, order);
    }

    fn compare_exchange(
        &self,
        current: u32,
        new: u32,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u32, u32> {
        self.allocation
            .compare_exchange(current, new, success, failure)
    }
}

struct FrameLedger {
    chunks: &'static [spin::Once<&'static [FrameLedgerSlot]>],
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

    fn ensure_chunk(&self, index: usize) -> Result<&'static [FrameLedgerSlot], ()> {
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
            FrameLedgerSlot::new(state)
        });

        let mut prepared = Some(slots);
        let published = self.chunks[chunk_index].call_once(|| {
            Vec::leak(prepared.take().expect("prepared frame-ledger chunk"))
                as &'static [FrameLedgerSlot]
        });
        Ok(*published)
    }

    fn get(&self, index: usize) -> Option<&FrameLedgerSlot> {
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
pub(crate) struct FrameLease {
    frame: PhysFrame,
    index: u32,
    generation: u32,
}

impl FrameLease {
    pub(crate) fn frame(self) -> PhysFrame {
        self.frame
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReturnOutcome {
    Returned,
    LostContended,
    RefusedDoubleRelease,
    RefusedStale,
    RefusedNeverAllocated,
    RefusedUntracked,
    RefusedLiveLeaf,
}

const LEAF_EXTERNAL: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeafMappingClass {
    Owned,
    External,
}

#[derive(Clone, Copy)]
struct ExternalLeafSpan {
    start: u64,
    end: u64,
}

const EMPTY_EXTERNAL_LEAF_SPAN: ExternalLeafSpan = ExternalLeafSpan { start: 0, end: 0 };
const MAX_EXTERNAL_LEAF_SPANS: usize = 16;
static EXTERNAL_LEAF_SPANS: Mutex<[ExternalLeafSpan; MAX_EXTERNAL_LEAF_SPANS]> =
    Mutex::new([EMPTY_EXTERNAL_LEAF_SPAN; MAX_EXTERNAL_LEAF_SPANS]);

/// Free list for deallocated frames
/// When frames are deallocated (e.g., after CoW copy reduces refcount to 0),
/// they are added to this list for reuse. Once the ledger is published, its
/// capacity is prepared before sequential frames become visible and returns
/// refuse at the exact boundary rather than reallocating.
static FREE_FRAMES: Mutex<Vec<PhysFrame>> = Mutex::new(Vec::new());

/// The logical CPU holding `FREE_FRAMES` through `with_free_frames`, or
/// `usize::MAX`.
static FREE_FRAMES_OWNER: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Run `f` on the free list. A holder on another CPU is waited for; `f` is
/// handed the list unless this CPU already holds it, which happens when the
/// capacity reservation below grows the heap and the heap allocates a frame.
///
/// The list used to be try-locked. With one CPU a busy lock could only mean
/// such nesting, and the fallbacks fit it: an allocation took the next frame
/// past the frontier, a return was dropped. With several CPUs it is usually
/// held by another CPU for a push or a pop, and a frame returned while it was
/// held was lost: free in the ledger but missing from the list. Interrupts are
/// masked for the hold, so a holder is not interrupted or switched out with it
/// held.
fn with_free_frames<R>(f: impl FnOnce(Option<&mut Vec<PhysFrame>>) -> R) -> R {
    crate::arch_without_interrupts(|| {
        let me = crate::task::scheduler::current_cpu();
        loop {
            if let Some(mut free_list) = FREE_FRAMES.try_lock() {
                FREE_FRAMES_OWNER.store(me, Ordering::Relaxed);
                let result = f(Some(&mut free_list));
                FREE_FRAMES_OWNER.store(usize::MAX, Ordering::Relaxed);
                return result;
            }
            if FREE_FRAMES_OWNER.load(Ordering::Relaxed) == me {
                return f(None);
            }
            core::hint::spin_loop();
        }
    })
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

fn frame_in_external_leaf_span(frame: PhysFrame) -> bool {
    let address = frame.start_address().as_u64();
    EXTERNAL_LEAF_SPANS
        .lock()
        .iter()
        .any(|span| span.start != span.end && (span.start..span.end).contains(&address))
}

fn mark_external_leaf_slot(frame: PhysFrame) -> Result<bool, &'static str> {
    let Some(index) = frame_ordinal(frame) else {
        return Ok(false);
    };
    let Some(ledger) = FRAME_LEDGER.get() else {
        return Err("frame ledger not initialized");
    };
    let Some(slot) = ledger.get(index) else {
        return Err("external leaf frame has no realized allocator slot");
    };
    if slot.load(Ordering::Acquire) & STATE_MASK != ST_ALLOCATED {
        return Err("external leaf frame is not allocator-owned and live");
    }
    loop {
        let refs = slot.leaf_refs.load(Ordering::Acquire);
        if refs == LEAF_EXTERNAL {
            return Ok(true);
        }
        if refs != 0 {
            return Err("external leaf frame already has owned mappings");
        }
        if slot
            .leaf_refs
            .compare_exchange(0, LEAF_EXTERNAL, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(true);
        }
    }
}

/// Register allocator-backed memory that remains owned outside process page
/// tables. Mapping code does not choose this class: the owning allocator site
/// marks the frame before any user descriptor can be published.
pub(crate) fn register_external_leaf_frame(frame: PhysFrame) -> Result<(), &'static str> {
    if mark_external_leaf_slot(frame)? {
        Ok(())
    } else {
        Err("external leaf frame is outside allocator memory")
    }
}

/// Return a frame `register_external_leaf_frame` marked. Its owner calls this
/// once the process page tables that mapped the frame are gone: a table drops
/// an external leaf's mapping without returning the frame.
pub(crate) fn release_external_leaf_frame(frame: PhysFrame) -> ReturnOutcome {
    let Some(slot) = frame_ordinal(frame).and_then(|index| FRAME_LEDGER.get()?.get(index)) else {
        return counted(ReturnOutcome::RefusedUntracked);
    };
    if slot
        .leaf_refs
        .compare_exchange(LEAF_EXTERNAL, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return counted(ReturnOutcome::RefusedLiveLeaf);
    }
    return_lease(current_lease_for_frame(frame))
}

/// Derive one mapping reference from allocator state before its descriptor is
/// published. The caller must roll the reference back if mapping fails.
pub(crate) fn acquire_leaf_mapping(frame: PhysFrame) -> Result<LeafMappingClass, &'static str> {
    if frame_in_external_leaf_span(frame) {
        return Ok(LeafMappingClass::External);
    }
    let Some(index) = frame_ordinal(frame) else {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
        return Err("leaf frame is neither allocator-owned nor registered external memory");
    };
    let Some(ledger) = FRAME_LEDGER.get() else {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
        return Err("frame ledger not initialized");
    };
    let Some(slot) = ledger.get(index) else {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
        return Err("leaf frame has no realized allocator slot");
    };
    if slot.load(Ordering::Acquire) & STATE_MASK != ST_ALLOCATED {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
        return Err("leaf frame is not allocator-owned and live");
    }

    loop {
        let refs = slot.leaf_refs.load(Ordering::Acquire);
        if refs == LEAF_EXTERNAL {
            return Ok(LeafMappingClass::External);
        }
        if refs == LEAF_EXTERNAL - 1 {
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
            return Err("leaf mapping reference count exhausted");
        }
        if slot
            .leaf_refs
            .compare_exchange(refs, refs + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            if slot.load(Ordering::Acquire) & STATE_MASK == ST_ALLOCATED {
                return Ok(LeafMappingClass::Owned);
            }
            slot.leaf_refs.fetch_sub(1, Ordering::AcqRel);
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
            return Err("leaf frame changed allocator state during classification");
        }
    }
}

/// Release one allocator-derived mapping reference. Unregistered, external,
/// or non-live frames fail closed and can never be returned by this operation.
pub(crate) fn decref_leaf_mapping(frame: PhysFrame) -> bool {
    let Some(index) = frame_ordinal(frame) else {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_DECREF_UNREGISTERED);
        return false;
    };
    let Some(slot) = FRAME_LEDGER.get().and_then(|ledger| ledger.get(index)) else {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_DECREF_UNREGISTERED);
        return false;
    };
    if slot.load(Ordering::Acquire) & STATE_MASK != ST_ALLOCATED {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_DECREF_UNREGISTERED);
        return false;
    }

    loop {
        let refs = slot.leaf_refs.load(Ordering::Acquire);
        if refs == LEAF_EXTERNAL {
            return false;
        }
        if refs == 0 {
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_DECREF_UNREGISTERED);
            return false;
        }
        if slot
            .leaf_refs
            .compare_exchange(refs, refs - 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return refs == 1;
        }
    }
}

pub(crate) fn leaf_mapping_refcount(frame: PhysFrame) -> u32 {
    if frame_in_external_leaf_span(frame) {
        return LEAF_EXTERNAL;
    }
    frame_ordinal(frame)
        .and_then(|index| FRAME_LEDGER.get()?.get(index))
        .map(|slot| slot.leaf_refs.load(Ordering::Acquire))
        .unwrap_or(0)
}

pub(crate) fn leaf_mapping_stats() -> (usize, u64) {
    let Some(ledger) = FRAME_LEDGER.get() else {
        return (0, 0);
    };
    let mut tracked = 0usize;
    let mut total_refs = 0u64;
    for chunk in ledger.chunks.iter().filter_map(spin::Once::get) {
        for slot in chunk.iter() {
            let refs = slot.leaf_refs.load(Ordering::Relaxed);
            if refs != 0 && refs != LEAF_EXTERNAL {
                tracked += 1;
                total_refs += refs as u64;
            }
        }
    }
    (tracked, total_refs)
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
    assert_eq!(NEXT_FREE_FRAME.load(Ordering::Acquire), frontier_snapshot);
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
    let reserve = |free_list: &mut Vec<PhysFrame>| {
        if free_list.capacity() < required {
            let additional = required.saturating_sub(free_list.len());
            if free_list.try_reserve(additional).is_err() {
                return PrepareFrame::Exhausted;
            }
        }
        FREE_FRAME_CAPACITY.store(free_list.capacity(), Ordering::Release);
        PrepareFrame::Ready
    };
    with_free_frames(|free_list| free_list.map_or(PrepareFrame::Contended, reserve))
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
        // A usable run below the VGA hole that holds the AP startup
        // trampoline; page 0 (real-mode interrupt vectors) is never used.
        #[cfg(target_arch = "x86_64")]
        if region.kind == MemoryRegionKind::Usable {
            let start = region.start.max(0x1000).next_multiple_of(4096);
            let end = region.end.min(0xA0000) & !0xfff;
            if end >= start + AP_TRAMPOLINE_PAGES * 4096 {
                AP_TRAMPOLINE_BASE.store(end - AP_TRAMPOLINE_PAGES * 4096, Ordering::Release);
            }
        }
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

/// Initialize the frame allocator for ARM64 from the frame pool's ranges:
/// page-aligned, ascending and disjoint (see `platform_config::frame_pool`).
#[cfg(target_arch = "aarch64")]
pub fn init_aarch64(ranges: &[arm64_boot_contract::phys_memory::PhysRange]) {
    let mut regions = [None; MAX_REGIONS];
    let mut region_count = 0;
    for range in ranges.iter().take(MAX_REGIONS) {
        debug_assert_eq!(range.start % 4096, 0);
        debug_assert_eq!(range.end % 4096, 0);
        regions[region_count] = Some(UsableRegion {
            start: range.start,
            end: range.end,
        });
        region_count += 1;
    }

    MEMORY_INFO.call_once(|| MemoryInfo {
        regions,
        region_count,
    });
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
    if let Some(frame) = with_free_frames(|free_list| free_list.and_then(|list| list.pop())) {
        return Some(frame);
    }

    // Fall back to sequential allocation from memory map
    let mut allocator = BootInfoFrameAllocator::new();
    allocator.allocate_frame()
}

enum ClaimError {
    Duplicate,
    Untracked,
}

#[track_caller]
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
                if slot.leaf_refs.load(Ordering::Acquire) != 0 {
                    crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
                    return Err(ClaimError::Duplicate);
                }
                let generation = (observed >> 2).wrapping_add(1) & 0x3fff_ffff;
                let allocated = (generation << 2) | ST_ALLOCATED;
                if slot
                    .compare_exchange(observed, allocated, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    slot.site
                        .store(frame_site(Location::caller()), Ordering::Relaxed);
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

#[track_caller]
fn allocate_claimed() -> Option<(PhysFrame, Option<FrameLease>)> {
    loop {
        let frame = allocate_candidate()?;
        match claim_frame(frame) {
            Ok(lease) => return Some((frame, lease)),
            Err(ClaimError::Duplicate | ClaimError::Untracked) => continue,
        }
    }
}

#[track_caller]
pub fn allocate_frame() -> Option<PhysFrame> {
    allocate_claimed().map(|(frame, _)| frame)
}

/// Reserve `count` physically contiguous frames, writing them in ascending
/// address order into `out[..count]`, and return the base frame.
///
/// DMA rings (VirtIO virtqueues) are published to the device as a single
/// physical address covering the whole ring, so their frames must be genuinely
/// adjacent in physical memory. The free list recycles frames in arbitrary
/// order, so adjacency can only be established at the sequential frontier:
/// this reserves a run of consecutive allocator ordinals whose physical
/// addresses are checked adjacent, and claims every one of them in the ledger
/// exactly as a single-frame allocation does. Frames reserved but not usable
/// (a run cut short by a usable-region boundary) are handed back through the
/// normal fail-closed return path, never dropped on the floor.
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

    // The claim runs in a closure, where `#[track_caller]` stops: record the
    // caller as each frame's site after its claim.
    let site = frame_site(Location::caller());
    contiguous::allocate(
        count,
        out,
        &NEXT_FREE_FRAME,
        BootInfoFrameAllocator::get_usable_frame,
        |frame| frame.start_address().as_u64(),
        prepare_contiguous_run,
        |frame| {
            let claimed = claim_frame(frame).is_ok();
            if claimed {
                set_frame_site(frame, site);
            }
            claimed
        },
        deallocate_frame,
    )
}

/// Record `site` as the allocation site of `frame`'s current generation.
fn set_frame_site(frame: PhysFrame, site: u16) {
    if let Some(slot) = frame_ordinal(frame).and_then(|index| FRAME_LEDGER.get()?.get(index)) {
        slot.site.store(site, Ordering::Relaxed);
    }
}

/// Prepare ledger and free-list capacity for every ordinal in a contiguous run.
///
/// Returns false only when capacity is genuinely exhausted. As on the
/// single-frame path, a transient free-list lock holder must not manufacture
/// OOM: after bounded retries the run is published anyway and a future
/// exact-boundary return is refused as a counted loss instead of reallocating.
fn prepare_contiguous_run(base: usize, run: usize) -> bool {
    for i in 0..run {
        let mut capacity_contentions = 0u8;
        loop {
            match prepare_frame_for_allocation(base + i) {
                PrepareFrame::Ready => break,
                PrepareFrame::Contended if capacity_contentions < 8 => {
                    capacity_contentions += 1;
                    core::hint::spin_loop();
                }
                PrepareFrame::Contended => break,
                PrepareFrame::Exhausted => return false,
            }
        }
    }
    true
}

#[track_caller]
pub(crate) fn allocate_frame_leased() -> Option<FrameLease> {
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
        ReturnOutcome::RefusedLiveLeaf => teardown::FRAME_RETURN_REFUSED_LIVE_LEAF.increment(),
        ReturnOutcome::Returned => {}
    }
    outcome
}

pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome {
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
                if slot.leaf_refs.load(Ordering::Acquire) != 0 {
                    return counted(ReturnOutcome::RefusedLiveLeaf);
                }
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

    let push = |free_list: &mut Vec<PhysFrame>| {
        // This explicit boundary check is the release-build proof that push
        // cannot grow the Vec on any ledger-backed return path.
        if free_list.len() == free_list.capacity() {
            return counted(ReturnOutcome::LostContended);
        }
        free_list.push(lease.frame);
        ReturnOutcome::Returned
    };
    with_free_frames(|free_list| {
        free_list.map_or_else(|| counted(ReturnOutcome::LostContended), push)
    })
}

fn current_lease_for_frame(frame: PhysFrame) -> FrameLease {
    let index = frame_ordinal(frame).unwrap_or(usize::MAX);
    let generation = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(index))
        .map(|slot| slot.load(Ordering::Acquire) >> 2)
        .unwrap_or(0);
    FrameLease {
        frame,
        index: u32::try_from(index).unwrap_or(u32::MAX),
        generation,
    }
}

/// Return a leaf whose mapping refcount reached zero without adding logging,
/// formatting, allocation, or blocking lock acquisition to the drain path.
pub(crate) fn deallocate_leaf_frame(frame: PhysFrame) -> ReturnOutcome {
    return_lease(current_lease_for_frame(frame))
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

    match return_lease(current_lease_for_frame(frame)) {
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
#[cfg(feature = "boot_tests")]
pub(crate) use boot_tests::republish_frame_for_gate;
#[cfg(feature = "boot_tests")]
pub(crate) use boot_tests::retire_with_free_list_contended;
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

    let free_list_frames = with_free_frames(|free_list| match free_list {
        Some(list) => list.len(),
        None => 0,
    });

    MemoryStats {
        total_bytes,
        allocated_frames,
        free_list_frames,
    }
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
    /// it) that a process page table maps (`mapped`) or does not. A site
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
}

/// The owner `allocate_frame_for` gave each site (`FRAME_SITES` index), or 0.
static FRAME_SITE_OWNERS: [AtomicU8; FRAME_SITE_SLOTS] =
    [const { AtomicU8::new(0) }; FRAME_SITE_SLOTS];

/// `allocate_frame`, recording what the caller's frames are for where the
/// census cannot tell it from the caller's source file.
#[track_caller]
pub fn allocate_frame_for(owner: FrameOwner) -> Option<PhysFrame> {
    if let Some(slot) = (frame_site(Location::caller()) as usize).checked_sub(1) {
        FRAME_SITE_OWNERS[slot].store(owner as u8, Ordering::Relaxed);
    }
    allocate_frame()
}

/// Allocated frames by the code that allocated them and by owner, and frames
/// lost to the allocator, read from the ledger (`/proc/breenix/frames`).
///
/// `used` is what `/proc/meminfo` counts as in use: the frames the sequential
/// allocator has handed out, less those on the free list. It is `allocated`
/// plus `lost`, frames the ledger records as free that no free list holds (a
/// return that found the list locked or full). The walk is not atomic against
/// concurrent allocation, so the parts can disagree by the frames allocated or
/// returned while it ran.
pub fn frame_census() -> Result<alloc::string::String, i32> {
    use core::fmt::Write;
    const ENOMEM: i32 = -12;

    let Some(ledger) = FRAME_LEDGER.get() else {
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
        let tagged = FRAME_SITE_OWNERS[slot - 1].load(Ordering::Relaxed);
        for (mapped, owner) in owners.iter_mut().enumerate() {
            *owner = if tagged != 0 { tagged } else { FrameOwner::of_site(file, mapped == 1) as u8 };
        }
    }
    let mut by_owner = [0usize; FrameOwner::ALL.len() + 1];
    let (mut allocated, mut free) = (0usize, 0usize);
    for chunk in ledger.chunks.iter().filter_map(spin::Once::get) {
        for slot in chunk.iter() {
            match slot.load(Ordering::Relaxed) & STATE_MASK {
                ST_ALLOCATED => {
                    allocated += 1;
                    let index = slot.site.load(Ordering::Relaxed) as usize;
                    let site = &mut frames[index];
                    site[0] += 1;
                    let refs = slot.leaf_refs.load(Ordering::Relaxed);
                    match refs {
                        0 => {}
                        LEAF_EXTERNAL => site[2] += 1,
                        _ => site[1] += 1,
                    }
                    by_owner[site_owners[index][usize::from(refs != 0)] as usize] += 1;
                }
                ST_FREE => free += 1,
                _ => {}
            }
        }
    }
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
