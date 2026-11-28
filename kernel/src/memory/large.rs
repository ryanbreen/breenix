//! The kernel side of the larger-request layers (E1 plan, 6.1 rows 2 and 3,
//! 6.4 and 6.5; PR E1-7): span arenas running `heap_index` in the span
//! window, and vmap allocations of order-0 frames in the vmap window. The
//! algorithms are `span_arena.rs` and `vmap.rs`, which the host tests
//! include; this file supplies the locks, the windows (`kva.rs`) and the
//! frames.
//!
//! In E1-7 `GlobalAlloc` does not route here yet: only `self_check` does,
//! and E1-9 routes requests above a page (6.1). The fixed heap still serves
//! them until then, so no device buffer changes here.
//!
//! - Each CPU the firmware describes has a span arena, installed with the
//!   magazine slot table (5.4a), and a CPU that cannot name itself, or has
//!   no arena, uses the shared one. A free goes to the block's own arena,
//!   which its span's record names.
//! - A vmap allocation's record is a slab object, found from the
//!   allocation's first frame: the second word of that frame's `FrameMeta`,
//!   which a slab page uses for its descriptor (4.3's `use` union).
//! - 7.2 step 2 gives back each arena's kept empty span through a
//!   registered drain, with `try_lock` on the arena and on the span window.

use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use super::frame_allocator::{self, FrameOwner};
use super::kmem::{self, AllocError, Ctx, Layer};
use super::kva::{self, WindowId};
use super::kva_extent::Allocation;
use super::span_arena::{self, Arena, Miss, Unmapped, SPAN_BYTES};
use super::vmap::{self, Block, Space, PAGE};

type SpanArena = Arena<Allocation>;

/// The arena number of the shared arena, in its spans' records.
const SHARED: usize = usize::MAX;
static SHARED_ARENA: Mutex<SpanArena> = Mutex::new(Arena::new(SHARED));
/// The per-CPU arenas, one per described CPU, installed once.
static ARENAS: AtomicPtr<Mutex<SpanArena>> = AtomicPtr::new(core::ptr::null_mut());
static ARENA_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Spans and vmap allocations made and given back, for `/proc/breenix/kmem`.
static SPANS_MAPPED: AtomicU64 = AtomicU64::new(0);
static SPANS_UNMAPPED: AtomicU64 = AtomicU64::new(0);
static VMAP_LIVE: AtomicU64 = AtomicU64::new(0);
static VMAP_MOVES: AtomicU64 = AtomicU64::new(0);

static SPAN_DRAIN: kmem::Drain = kmem::Drain::new("span-empty", drain_retained);

/// Register the span arenas' step-2 drain; called once, after the windows.
pub fn init() {
    kmem::register_drain(&SPAN_DRAIN);
}

/// Install one span arena for each of the `cpus` CPUs the firmware describes
/// (5.4a), from the shared path. With no table, each CPU uses the shared
/// arena, which is correct and slower.
pub(super) fn init_cpu_arenas(cpus: usize) {
    let Ok(layout) = Layout::array::<Mutex<SpanArena>>(cpus) else { return };
    if cpus == 0 {
        return;
    }
    let Ok(table) = kmem::try_alloc(layout, Ctx::Normal) else { return };
    let table = table.cast::<Mutex<SpanArena>>().as_ptr();
    // SAFETY: the block holds `cpus` arenas, and no code frees it.
    unsafe {
        for cpu in 0..cpus {
            table.add(cpu).write(Mutex::new(Arena::new(cpu)));
        }
    }
    ARENA_COUNT.store(cpus, Ordering::Relaxed);
    ARENAS.store(table, Ordering::Release);
}

/// Arena `id`, the shared one for an id with no per-CPU arena.
fn arena(id: usize) -> &'static Mutex<SpanArena> {
    let table = ARENAS.load(Ordering::Acquire);
    if table.is_null() || id >= ARENA_COUNT.load(Ordering::Relaxed) {
        return &SHARED_ARENA;
    }
    // SAFETY: an installed table holds `ARENA_COUNT` arenas for good.
    unsafe { &*table.add(id) }
}

/// This CPU's arena number. Read with interrupts masked by the caller, so
/// the CPU cannot change under it.
fn local_arena() -> usize {
    match kmem::named_cpu() {
        Some(cpu) if cpu < ARENA_COUNT.load(Ordering::Relaxed) => cpu,
        _ => SHARED,
    }
}

/// Run `f` on arena `id` with this CPU's interrupts masked. `wait` false
/// takes the lock with one `try_lock`, and a busy lock gives an empty `Option`.
fn with_arena<R>(id: usize, wait: bool, f: impl FnOnce(&mut SpanArena) -> R) -> Option<R> {
    crate::arch_without_interrupts(|| {
        let mutex = arena(id);
        if wait {
            Some(f(&mut mutex.lock()))
        } else {
            mutex.try_lock().map(|mut arena| f(&mut arena))
        }
    })
}

/// A list of frames threaded through their own first word, through the
/// direct map, so taking 512 of them needs no other memory.
struct Frames {
    head: u64,
    count: u64,
}

impl Frames {
    fn direct(pa: u64) -> *mut u64 {
        (super::physical_memory_offset().as_u64() + pa) as *mut u64
    }

    /// `count` frames, or `NoFrames` with each one taken given back.
    fn take(count: u64) -> Result<Frames, AllocError> {
        let mut frames = Frames { head: 0, count: 0 };
        while frames.count < count {
            let Some(frame) = frame_allocator::allocate_frame_for(FrameOwner::Other) else {
                frames.give_back();
                return Err(AllocError::NoFrames);
            };
            let pa = frame.start_address().as_u64();
            // SAFETY: a new frame, which the direct map covers.
            unsafe { Self::direct(pa).write(frames.head) };
            frames.head = pa;
            frames.count += 1;
        }
        Ok(frames)
    }

    fn pop(&mut self) -> u64 {
        assert!(self.count != 0, "a mapping took more frames than it reserved");
        let pa = self.head;
        // SAFETY: a listed frame holds the next one's address.
        self.head = unsafe { Self::direct(pa).read() };
        self.count -= 1;
        pa
    }

    fn give_back(&mut self) {
        while self.count != 0 {
            let pa = self.pop();
            frame_allocator::deallocate_frame(super::PhysFrame::containing_address(super::PhysAddr::new(pa)));
        }
    }
}

/// Map a new span: 2 MiB of the span window, backed by 512 order-0 frames
/// mapped 4 KiB at a time (6.4; a single 2 MiB block once E1-8's buddy can
/// give one).
fn map_span(ctx: Ctx) -> Result<Unmapped<Allocation>, AllocError> {
    let span = SPAN_BYTES as u64;
    let allocation = kva::reserve(WindowId::Span, span, span, ctx)?;
    let mut frames = match Frames::take(span / PAGE) {
        Ok(frames) => frames,
        Err(error) => {
            kva::release(WindowId::Span, allocation, ctx);
            return Err(error);
        }
    };
    if let Err(error) = kva::map(&allocation, span, |_| frames.pop(), ctx) {
        frames.give_back();
        kva::release(WindowId::Span, allocation, ctx);
        return Err(error);
    }
    SPANS_MAPPED.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the span window hands out 2 MiB-aligned spans.
    let base = unsafe { NonNull::new_unchecked(allocation.start as *mut u8) };
    Ok(Unmapped { base, token: allocation })
}

/// Give a span back to the span window; its frames return after the flush.
fn unmap_span(span: Unmapped<Allocation>, ctx: Ctx) {
    SPANS_UNMAPPED.fetch_add(1, Ordering::Relaxed);
    kva::release(WindowId::Span, span.token, ctx);
}

/// 7.2 step 2 for the span arenas: each arena's kept empty span back to the
/// span window, with `try_lock` on the arena and the window, so a busy one
/// is skipped. Returns the frames free now.
fn drain_retained() -> usize {
    let count = if ARENAS.load(Ordering::Acquire).is_null() { 0 } else { ARENA_COUNT.load(Ordering::Relaxed) };
    let mut freed = 0;
    for id in (0..count).chain(core::iter::once(SHARED)) {
        freed += with_arena(id, false, |arena| {
            let Some(span) = arena.take_retained() else { return 0 };
            let base = span.base;
            match kva::try_release(WindowId::Span, span.token) {
                Ok(frames) => {
                    SPANS_UNMAPPED.fetch_add(1, Ordering::Relaxed);
                    frames
                }
                Err(token) => {
                    // SAFETY: the span is still mapped and its record intact.
                    unsafe { arena.restore(Unmapped { base, token }) };
                    0
                }
            }
        })
        .unwrap_or(0);
    }
    freed
}

/// The vmap window's size, the alignment bound of 6.1 row 3.
fn vmap_bytes() -> u64 {
    kva::placement(WindowId::Vmap).map_or(0, |placement| placement.bytes)
}

/// Allocate `layout`, which routes to a span arena or to vmap (6.1 rows 2
/// and 3), in `ctx`. A layout of row 1 is the slab layer's and is refused.
pub fn allocate(layout: Layout, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    match kmem::route(layout, vmap_bytes())? {
        Layer::Slab => Err(AllocError::Refused),
        Layer::Span => span_allocate(layout, ctx),
        Layer::Vmap => vmap_allocate(layout, ctx),
    }
}

fn span_allocate(layout: Layout, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    let ctx = kva::lowered(ctx);
    let wait = ctx.may_wait();
    let id = crate::arch_without_interrupts(local_arena);
    match with_arena(id, wait, |arena| arena.allocate(layout)) {
        Some(Ok(ptr)) => return Ok(ptr),
        Some(Err(Miss::TooLarge)) => return Err(AllocError::Refused),
        Some(Err(Miss::NeedSpan)) => {}
        None => return Err(kmem::STATS.refuse(ctx)),
    }
    let span = map_span(ctx)?;
    // SAFETY: a new span, mapped and owned by this arena from here on.
    match with_arena(id, true, |arena| unsafe { arena.adopt(span.base, span.token, layout) }) {
        Some(Ok(ptr)) => Ok(ptr),
        Some(Err(span)) => {
            unmap_span(span, ctx);
            Err(AllocError::Refused)
        }
        None => unreachable!("a waiting arena lock is always taken"),
    }
}

/// Whether `ptr` is in the span window or the vmap window.
pub fn contains(ptr: NonNull<u8>) -> bool {
    matches!(kva::window_of(ptr.as_ptr() as u64), Some(WindowId::Span | WindowId::Vmap))
}

/// Free `ptr`, allocated or last resized for `layout`, to the layer whose
/// window holds it (6.1's routing by address). A layout that does not match
/// the layer's record of the block is memory corruption and panics with both
/// values, holding no allocator lock.
///
/// # Safety
///
/// `ptr` came from `allocate` or `reallocate` and is not freed twice.
pub unsafe fn deallocate(ptr: NonNull<u8>, layout: Layout) {
    match kva::window_of(ptr.as_ptr() as u64) {
        Some(WindowId::Span) => unsafe { span_free(ptr, layout) },
        Some(WindowId::Vmap) => unsafe { vmap_free(ptr, layout) },
        _ => panic!("large: free of {:#x}, which is in neither the span nor the vmap window", ptr.as_ptr() as usize),
    }
}

unsafe fn span_free(ptr: NonNull<u8>, layout: Layout) {
    let id = unsafe { span_arena::owner::<Allocation>(ptr) };
    let result = with_arena(id, true, |arena| unsafe { arena.free(ptr, layout.size()) })
        .expect("a waiting arena lock is always taken");
    match result {
        Ok(Some(span)) => unmap_span(span, Ctx::Normal),
        Ok(None) => {}
        Err(mismatch) => panic!(
            "span arena: block {:#x} holds {} bytes, freed as {} bytes",
            ptr.as_ptr() as usize,
            mismatch.usable,
            layout.size()
        ),
    }
}

/// `ptr`, allocated or last resized for `layout`, as a block of `new_size`
/// bytes (6.6): in place where its layer allows, moving a vmap block's pages
/// without copying them, and otherwise a new block in the new layout's layer
/// with the bytes both sizes hold copied. The block keeps its layer while it
/// stays in place, and a later free with the new size finds its record.
///
/// # Safety
///
/// As for `deallocate`; `new_size` rounded up to `layout.align()` does not
/// overflow `isize`.
pub unsafe fn reallocate(ptr: NonNull<u8>, layout: Layout, new_size: usize, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    match kva::window_of(ptr.as_ptr() as u64) {
        Some(WindowId::Span) => {
            let id = unsafe { span_arena::owner::<Allocation>(ptr) };
            let resized = with_arena(id, true, |arena| unsafe { arena.resize(ptr, layout.size(), new_size) })
                .expect("a waiting arena lock is always taken");
            match resized {
                Ok(true) => return Ok(ptr),
                Ok(false) => {}
                Err(mismatch) => panic!(
                    "span arena: block {:#x} holds {} bytes, resized from {} bytes",
                    ptr.as_ptr() as usize,
                    mismatch.usable,
                    layout.size()
                ),
            }
        }
        Some(WindowId::Vmap) => return unsafe { vmap_resize(ptr, layout, new_size, ctx) },
        _ => panic!("large: resize of {:#x}, which is in neither the span nor the vmap window", ptr.as_ptr() as usize),
    }
    let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
    let new_ptr = match kmem::route(new_layout, vmap_bytes())? {
        Layer::Slab => kmem::try_alloc(new_layout, ctx)?,
        _ => allocate(new_layout, ctx)?,
    };
    // SAFETY: both blocks are live and distinct, each at least the length copied.
    unsafe {
        core::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), layout.size().min(new_size));
        deallocate(ptr, layout);
    }
    Ok(new_ptr)
}

/// The vmap window as `vmap.rs` reaches it, in one request's context.
struct KernelVmap {
    ctx: Ctx,
}

impl Space for KernelVmap {
    type Error = AllocError;

    fn reserve(&mut self, len: u64, align: u64) -> Result<Allocation, AllocError> {
        kva::reserve(WindowId::Vmap, len, align, self.ctx)
    }

    fn extend(&mut self, allocation: &Allocation, extra: u64) -> Option<Allocation> {
        kva::extend(WindowId::Vmap, allocation, extra)
    }

    fn map_fresh(&mut self, allocation: &Allocation, offset: u64, len: u64) -> Result<(), AllocError> {
        let mut frames = Frames::take(len / PAGE)?;
        kva::map_at(allocation, offset, len, |_| frames.pop(), self.ctx).inspect_err(|_| frames.give_back())
    }

    fn map_moved(&mut self, from: &Allocation, to: &Allocation, moved: u64) -> Result<(), AllocError> {
        let mut frames = Frames::take((to.len - moved) / PAGE)?;
        kva::remap(from, to, moved, |_| frames.pop(), self.ctx).inspect_err(|_| frames.give_back())
    }

    fn forward(&mut self, allocation: Allocation) {
        kva::forward(WindowId::Vmap, allocation, self.ctx);
    }

    fn release(&mut self, allocation: Allocation) {
        kva::release(WindowId::Vmap, allocation, self.ctx);
    }
}

/// The metadata entry of the frame a vmap block's first page maps, which
/// holds the block's record.
fn record_entry(start: u64) -> Option<&'static super::frame_meta::FrameMeta> {
    let pa = kva::frame_of(start)?;
    frame_allocator::frame_entry(super::PhysFrame::containing_address(super::PhysAddr::new(pa)))
}

/// The record of the vmap block at `ptr`, checked against `size`, the size
/// the caller says it was allocated or last resized for.
fn vmap_record(ptr: NonNull<u8>, size: usize) -> NonNull<Block> {
    let start = ptr.as_ptr() as u64;
    let record = record_entry(start)
        .and_then(|entry| NonNull::new(entry.vmap_record() as *mut Block))
        .unwrap_or_else(|| panic!("vmap: {start:#x} is not the start of a vmap block"));
    // SAFETY: a published record is live until its block is freed.
    let block = unsafe { record.as_ptr().read() };
    if block.allocation.start != start || block.size != size {
        panic!(
            "vmap: block {:#x} of {} bytes, freed or resized at {start:#x} as {size} bytes",
            block.allocation.start, block.size
        );
    }
    record
}

fn vmap_allocate(layout: Layout, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    let ctx = kva::lowered(ctx);
    if !ctx.may_map() {
        return Err(kmem::STATS.refuse(ctx));
    }
    let record = kmem::try_alloc(Layout::new::<Block>(), ctx)?.cast::<Block>();
    let mut space = KernelVmap { ctx };
    let block = match vmap::allocate(&mut space, layout.size(), layout.align()) {
        Ok(block) => block,
        Err(error) => {
            // SAFETY: the record was allocated above for this layout.
            unsafe { alloc::alloc::dealloc(record.as_ptr().cast(), Layout::new::<Block>()) };
            return Err(error);
        }
    };
    // SAFETY: the record is a new block sized and aligned for `Block`.
    unsafe { record.as_ptr().write(block) };
    record_entry(block.allocation.start)
        .expect("a vmap block's first page maps a tracked frame")
        .set_vmap_record(record.as_ptr() as u64);
    VMAP_LIVE.fetch_add(1, Ordering::Relaxed);
    // SAFETY: window VA lies in the kernel half, so it is not 0.
    Ok(unsafe { NonNull::new_unchecked(block.allocation.start as *mut u8) })
}

unsafe fn vmap_free(ptr: NonNull<u8>, layout: Layout) {
    let record = vmap_record(ptr, layout.size());
    // SAFETY: as in `vmap_record`; the record is this block's alone.
    let block = unsafe { record.as_ptr().read() };
    if let Some(entry) = record_entry(block.allocation.start) {
        entry.set_vmap_record(0);
    }
    vmap::free(&mut KernelVmap { ctx: Ctx::Normal }, block);
    VMAP_LIVE.fetch_sub(1, Ordering::Relaxed);
    // SAFETY: the record was allocated in `vmap_allocate` for this layout.
    unsafe { alloc::alloc::dealloc(record.as_ptr().cast(), Layout::new::<Block>()) };
}

unsafe fn vmap_resize(ptr: NonNull<u8>, layout: Layout, new_size: usize, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    let record = vmap_record(ptr, layout.size());
    // SAFETY: as in `vmap_free`.
    let mut block = unsafe { record.as_ptr().read() };
    let resized = vmap::resize(&mut KernelVmap { ctx }, &mut block, new_size);
    // The first frame moves with the pages, so its entry still names the record.
    // SAFETY: as above.
    unsafe { record.as_ptr().write(block) };
    if resized? == vmap::Resized::Moved {
        VMAP_MOVES.fetch_add(1, Ordering::Relaxed);
    }
    // SAFETY: window VA lies in the kernel half, so it is not 0.
    Ok(unsafe { NonNull::new_unchecked(block.allocation.start as *mut u8) })
}

/// What `self_check` did.
pub struct SelfCheck {
    pub spans: u64,
    pub vmap_moves: u64,
}

/// A byte pattern for offset `i` of check block `tag`.
fn pattern(tag: u8, i: usize) -> u8 {
    (i as u8).wrapping_mul(31).wrapping_add(tag)
}

/// Fill `len` bytes at `ptr` with block `tag`'s pattern.
///
/// # Safety
///
/// `[ptr, ptr + len)` is a live block's.
unsafe fn fill(ptr: NonNull<u8>, len: usize, tag: u8) {
    for i in 0..len {
        unsafe { ptr.as_ptr().add(i).write_volatile(pattern(tag, i)) };
    }
}

/// Whether `len` bytes at `ptr` hold block `tag`'s pattern.
///
/// # Safety
///
/// As for `fill`.
unsafe fn holds(ptr: NonNull<u8>, len: usize, tag: u8) -> bool {
    (0..len).all(|i| unsafe { ptr.as_ptr().add(i).read_volatile() } == pattern(tag, i))
}

/// The init self-check (E1-7): span blocks allocated, written, grown in
/// place and freed, with their span given back; a 4 MiB vmap block of
/// discontiguous frames grown in place and then moved without a copy; and
/// `dma::bus_address` refusing both windows' addresses and accepting a
/// direct-map frame. Each step is checked through the bytes the CPU reads.
pub fn self_check() -> Result<SelfCheck, &'static str> {
    let spans_before = SPANS_MAPPED.load(Ordering::Relaxed);
    let moves_before = VMAP_MOVES.load(Ordering::Relaxed);
    let small = Layout::from_size_align(16 << 10, 4096).map_err(|_| "layout")?;
    let a = allocate(small, Ctx::Normal).map_err(|_| "no span block")?;
    let b = allocate(small, Ctx::Normal).map_err(|_| "no second span block")?;
    if kva::window_of(a.as_ptr() as u64) != Some(WindowId::Span) || a.as_ptr() as usize % 4096 != 0 {
        return Err("a span block is outside the span window or misaligned");
    }
    // SAFETY: each block is live for the lengths named.
    unsafe {
        fill(a, small.size(), 1);
        fill(b, small.size(), 2);
        // `b` follows `a`, so `a` has no room after it; free `b` and grow `a`.
        deallocate(b, small);
        let grown = reallocate(a, small, 64 << 10, Ctx::Normal).map_err(|_| "a span block did not grow")?;
        if grown != a || !holds(a, small.size(), 1) {
            return Err("a span block with free space after it moved or lost bytes when it grew");
        }
        deallocate(a, Layout::from_size_align_unchecked(64 << 10, 4096));
    }
    if crate::memory::dma::bus_address(a.as_ptr(), 4096).is_ok() {
        return Err("dma::bus_address accepted a span address");
    }

    let big = Layout::from_size_align(4 << 20, 8).map_err(|_| "layout")?;
    let v = allocate(big, Ctx::Normal).map_err(|_| "no vmap block")?;
    if kva::window_of(v.as_ptr() as u64) != Some(WindowId::Vmap) {
        return Err("a vmap block is outside the vmap window");
    }
    // SAFETY: `v` is live for each length named, and each resize keeps its bytes.
    let result = unsafe {
        fill(v, big.size(), 3);
        let grown = reallocate(v, big, 6 << 20, Ctx::Normal).map_err(|_| "a vmap block did not grow")?;
        // A second block right after it blocks the next growth, which moves.
        let blocker = allocate(big, Ctx::Normal).map_err(|_| "no second vmap block")?;
        let six = Layout::from_size_align_unchecked(6 << 20, 8);
        let moved = reallocate(grown, six, 10 << 20, Ctx::Normal).map_err(|_| "a vmap block did not move")?;
        let ok = grown == v && moved != grown && holds(moved, big.size(), 3);
        let refused = crate::memory::dma::bus_address(moved.as_ptr(), 8192).is_err();
        deallocate(blocker, big);
        deallocate(moved, Layout::from_size_align_unchecked(10 << 20, 8));
        if !ok {
            Err("a vmap block lost bytes or did not stay in place when it grew")
        } else if !refused {
            Err("dma::bus_address accepted a vmap address")
        } else {
            Ok(())
        }
    };
    result?;

    let frame = frame_allocator::allocate_frame_for(FrameOwner::Other).ok_or("no frame for the DMA check")?;
    let pa = frame.start_address().as_u64();
    let direct = (super::physical_memory_offset().as_u64() + pa) as *const u8;
    let bus = crate::memory::dma::bus_address(direct, 4096);
    frame_allocator::deallocate_frame(frame);
    #[cfg(target_arch = "x86_64")]
    let expected = pa;
    #[cfg(target_arch = "aarch64")]
    let expected = crate::platform_config::kernel_va_to_ipa(direct as u64);
    if bus != Ok(expected) {
        return Err("dma::bus_address refused or mistranslated a direct-map frame");
    }
    // Give the checks' frames back now rather than at the next flush.
    kva::flush(WindowId::Span);
    kva::flush(WindowId::Vmap);
    Ok(SelfCheck {
        spans: SPANS_MAPPED.load(Ordering::Relaxed) - spans_before,
        vmap_moves: VMAP_MOVES.load(Ordering::Relaxed) - moves_before,
    })
}

/// The layers' counts as `name value` lines for `/proc/breenix/kmem`.
pub fn write_stats(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    writeln!(out, "span_spans_mapped {}", SPANS_MAPPED.load(Ordering::Relaxed))?;
    writeln!(out, "span_spans_unmapped {}", SPANS_UNMAPPED.load(Ordering::Relaxed))?;
    writeln!(out, "vmap_live {}", VMAP_LIVE.load(Ordering::Relaxed))?;
    writeln!(out, "vmap_moves {}", VMAP_MOVES.load(Ordering::Relaxed))
}
