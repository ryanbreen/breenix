//! E1-7: span arenas, vmap allocations, the DMA bus-address check and the
//! layout routing (E1 plan 5.6, 6.1, 6.4, 6.5), driven through the kernel's
//! own `span_arena.rs`, `vmap.rs`, `dma_core.rs`, `kmem_core.rs` and E1-5's
//! window code over simulated page tables, frames and per-CPU TLBs.
//!
//! The CPUs are simulated under a fixed schedule: no threads, no timing.
use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

#[allow(dead_code)]
#[path = "../../../kernel/src/memory/dma_core.rs"]
mod dma_core;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/heap_index.rs"]
mod heap_index;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kmem_core.rs"]
mod kmem_core;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kva_extent.rs"]
mod kva_extent;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kva_map.rs"]
mod kva_map;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kva_window.rs"]
mod kva_window;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/span_arena.rs"]
mod span_arena;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/tlb_request.rs"]
mod tlb_request;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/vmap.rs"]
mod vmap;
#[path = "kmem_front/sim.rs"]
mod sim;

use kmem_core::{AllocError, Cpu, Drain, Front, Layer, Miss, Pool, Reclaim, ReclaimCtx, Registry, Shrinker, Small, Stats};
use kva_extent::Allocation;
use kva_map::{Format, TableMemory, TablePages, Walk, X86};
use kva_window::{Placement, Window, WindowId};
use sim::SimLock;
use span_arena::{Arena, Unmapped, SPAN_BYTES};

const PAGE: u64 = 4096;

#[repr(C, align(4096))]
struct Page([AtomicU64; 512]);

/// Physical pages, addressed by physical address in one host allocation:
/// table pages and data frames alike, so a frame's bytes can be read where
/// a CPU would read them. Page 0 is not handed out.
struct Phys {
    pages: Box<[Page]>,
    next: Cell<usize>,
}

impl Phys {
    fn new(pages: usize) -> Self {
        let pages = (0..pages).map(|_| Page([const { AtomicU64::new(0) }; 512])).collect();
        Phys { pages, next: Cell::new(1) }
    }

    fn page(&self) -> u64 {
        let index = self.next.get();
        assert!(index < self.pages.len(), "fixture out of pages");
        self.next.set(index + 1);
        index as u64 * PAGE
    }

    fn bytes(&self, pa: u64) -> *mut u8 {
        self.pages[(pa / PAGE) as usize].0.as_ptr() as *mut u8
    }
}

impl TableMemory for Phys {
    fn load(&self, table: u64, index: usize) -> u64 {
        self.pages[(table / PAGE) as usize].0[index].load(Ordering::Acquire)
    }
    fn store(&self, table: u64, index: usize, value: u64) {
        self.pages[(table / PAGE) as usize].0[index].store(value, Ordering::Release);
    }
    fn compare_exchange(&self, table: u64, index: usize, current: u64, new: u64) -> Result<u64, u64> {
        self.pages[(table / PAGE) as usize].0[index].compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
    }
    fn zero(&self, page: u64) {
        for entry in self.pages[(page / PAGE) as usize].0.iter() {
            entry.store(0, Ordering::Relaxed);
        }
    }
    fn publish_table(&self) {}
}

/// Table pages from the same physical pages.
struct TableSource<'a>(&'a Phys);

impl TablePages for TableSource<'_> {
    fn allocate(&mut self) -> Option<u64> {
        Some(self.0.page())
    }
    fn free(&mut self, _page: u64) {}
}

/// One CPU's TLB, following Intel SDM vol. 3A §4.10.4.1 with CR4.PGE set:
/// window leaves are global, so only `invlpg` or a PGE toggle drops them.
#[derive(Default)]
struct FakeTlb {
    entries: HashMap<u64, u64>,
}

impl tlb_request::TlbOps for FakeTlb {
    fn invlpg(&mut self, va: u64) {
        self.entries.remove(&(va & !(PAGE - 1)));
    }
    fn reload_cr3(&mut self) {}
    fn toggle_global_pages(&mut self) {
        self.entries.clear();
    }
    fn release_root(&mut self, _root: u64) {}
}

/// The CPUs as the x86 kernel-unmap shootdown reaches them.
struct FixtureCpus<'a> {
    tlbs: &'a mut [FakeTlb],
}

impl FixtureCpus<'_> {
    fn run(tlb: &mut FakeTlb, request: tlb_request::Request) {
        let (kind, arg) = tlb_request::encode(request);
        // SAFETY: the page list a request names outlives the flush call.
        unsafe { tlb_request::execute(kind, arg, tlb) };
    }
}

impl tlb_request::Shootdown for FixtureCpus<'_> {
    fn here(&mut self, request: tlb_request::Request) {
        Self::run(&mut self.tlbs[0], request);
    }
    fn others(&mut self, request: tlb_request::Request) {
        for tlb in self.tlbs[1..].iter_mut() {
            Self::run(tlb, request);
        }
    }
}

/// The frame `va` translates to through the tables, if any.
fn translate(phys: &Phys, root: u64, va: u64) -> Option<u64> {
    let Walk::Table(table) = kva_map::walk::<X86, _>(phys, root, va & !(PAGE - 1), 3) else {
        return None;
    };
    let leaf = phys.load(table, kva_map::index(va, 3));
    X86::is_valid(leaf).then(|| X86::address(leaf))
}

/// A kernel half with one window open in PML4 slot `slot`, over x86 tables
/// and 4 CPUs, and a frame allocator that hands out the frames in `pool` in
/// a fixed scattered order.
struct World {
    phys: Phys,
    root: u64,
    window: RefCell<Window>,
    _nodes: Vec<u64>,
    tlbs: RefCell<Vec<FakeTlb>>,
    pool: RefCell<Vec<u64>>,
    /// Frames handed out, and frames given back after a flush.
    taken: RefCell<Vec<u64>>,
    offered: RefCell<Vec<u64>>,
}

impl World {
    fn new(id: WindowId, slot: usize, frames: usize) -> World {
        let phys = Phys::new(frames + 64);
        let root = phys.page();
        let pdpt = phys.page();
        phys.store(root, slot, X86::table(pdpt));
        let placement = Placement { id, first_slot: slot, slots: 1, base: kva_window::slot_va(slot), bytes: 1 << 30 };
        let mut window = Window::new(placement, id.shape().0);
        let mut nodes = vec![0u64; 4096];
        unsafe { window.extents.add_nodes(nodes.as_mut_ptr() as *mut u8, nodes.len() * 8) };
        window.open().unwrap();
        // Frames from the fixture's pages, taken in an order that is not
        // ascending (seed 0x7e1), so consecutive pages get scattered frames.
        let mut pool: Vec<u64> = (0..frames).map(|_| phys.page()).collect();
        let mut seed = 0x7e1u64;
        for i in (1..pool.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            pool.swap(i, (seed % (i as u64 + 1)) as usize);
        }
        World {
            phys,
            root,
            window: RefCell::new(window),
            _nodes: nodes,
            tlbs: RefCell::new((0..4).map(|_| FakeTlb::default()).collect()),
            pool: RefCell::new(pool),
            taken: RefCell::new(Vec::new()),
            offered: RefCell::new(Vec::new()),
        }
    }

    fn take_frames(&self, count: u64) -> Result<Vec<u64>, &'static str> {
        let mut pool = self.pool.borrow_mut();
        if (pool.len() as u64) < count {
            return Err("no frames");
        }
        let frames: Vec<u64> = (0..count).map(|_| pool.pop().unwrap()).collect();
        self.taken.borrow_mut().extend(&frames);
        Ok(frames)
    }

    /// Each CPU reads each page of `[va, va + len)`, caching its translation.
    fn touch(&self, va: u64, len: u64) {
        for tlb in self.tlbs.borrow_mut().iter_mut() {
            for page in (va..va + len).step_by(PAGE as usize) {
                if let Some(pa) = translate(&self.phys, self.root, page) {
                    tlb.entries.insert(page, pa);
                }
            }
        }
    }

    /// Flush the window's pending unmaps with x86's kernel-unmap flush on
    /// each CPU, then give back their frames and VA. Returns the frames.
    fn flush(&self) -> usize {
        let mut window = self.window.borrow_mut();
        let batch = window.take_pending();
        let mut tlbs = self.tlbs.borrow_mut();
        let mut tlb = tlb_request::KernelUnmap(FixtureCpus { tlbs: &mut tlbs });
        let flushed = batch.flush::<X86, _, _>(&self.phys, self.root, &mut tlb);
        let mut frames = 0;
        flushed.reclaim::<X86, _>(&self.phys, self.root, |pa, bytes| {
            assert_eq!(bytes, PAGE);
            for (cpu, tlb) in tlbs.iter().enumerate() {
                assert!(!tlb.entries.values().any(|&cached| cached == pa), "CPU {cpu} still reaches frame {pa:#x} as it is offered back");
            }
            self.offered.borrow_mut().push(pa);
            frames += 1;
        });
        window.release(flushed).unwrap();
        frames
    }

    /// Map a span: 2 MiB of the span window, 512 scattered frames, and a
    /// host buffer, 2 MiB aligned, for the bytes the arena writes.
    fn map_span(&self) -> Unmapped<(Allocation, usize)> {
        let span = SPAN_BYTES as u64;
        let allocation = self.window.borrow_mut().reserve(span, span).unwrap();
        let frames = self.take_frames(span / PAGE).unwrap();
        kva_map::map_range::<X86, _>(&self.phys, self.root, allocation.start, span, 3, |o| frames[(o / PAGE) as usize], &mut TableSource(&self.phys))
            .unwrap();
        self.touch(allocation.start, span);
        let layout = Layout::from_size_align(SPAN_BYTES, SPAN_BYTES).unwrap();
        let host = unsafe { std::alloc::alloc(layout) };
        Unmapped { base: NonNull::new(host).unwrap(), token: (allocation, host as usize) }
    }

    fn unmap_span(&self, span: Unmapped<(Allocation, usize)>) {
        let (allocation, host) = span.token;
        assert_eq!(host, span.base.as_ptr() as usize);
        self.window.borrow_mut().unmap::<X86, _>(&self.phys, self.root, &allocation).unwrap();
        unsafe { std::alloc::dealloc(host as *mut u8, Layout::from_size_align(SPAN_BYTES, SPAN_BYTES).unwrap()) };
    }
}

type SpanArena = Arena<(Allocation, usize)>;

/// Allocate `layout` from `arena`, mapping a span when it needs one.
fn span_alloc(world: &World, arena: &mut SpanArena, layout: Layout, mapped: &mut usize) -> NonNull<u8> {
    match arena.allocate(layout) {
        Ok(ptr) => ptr,
        Err(span_arena::Miss::NeedSpan) => {
            *mapped += 1;
            let span = world.map_span();
            unsafe { arena.adopt(span.base, span.token, layout) }.ok().unwrap()
        }
        Err(miss) => panic!("{miss:?}"),
    }
}

/// Span lifecycle (6.4): a span is mapped only when no span fits, a span
/// whose last block is freed is unmapped except one empty span the arena
/// keeps, and an unmapped span's frames are offered back only after the
/// flush, when no CPU's TLB still reaches them.
#[test]
fn spans_are_mapped_on_demand_unmapped_when_empty_and_freed_after_the_flush() {
    let world = World::new(WindowId::Span, 300, 3 * 512);
    let mut arena = SpanArena::new(0);
    let block = Layout::from_size_align(300 << 10, 4096).unwrap();
    let mut mapped = 0;
    let first: Vec<NonNull<u8>> = (0..6).map(|_| span_alloc(&world, &mut arena, block, &mut mapped)).collect();
    assert_eq!(mapped, 1, "six 300 KiB blocks fit one span");
    let second: Vec<NonNull<u8>> = (0..3).map(|_| span_alloc(&world, &mut arena, block, &mut mapped)).collect();
    assert_eq!((mapped, arena.spans()), (2, 2), "a span is mapped when no span fits");
    let base = |ptr: NonNull<u8>| ptr.as_ptr() as usize & !(SPAN_BYTES - 1);
    assert!(first.iter().all(|&ptr| base(ptr) == base(first[0])) && base(second[0]) != base(first[0]));
    for &ptr in first.iter().chain(&second) {
        assert_eq!(ptr.as_ptr() as usize % 4096, 0);
        assert_eq!(unsafe { span_arena::owner::<(Allocation, usize)>(ptr) }, 0, "a block names its arena");
    }

    // The second span empties first and is kept.
    for &ptr in &second {
        assert!(unsafe { arena.free(ptr, block.size()) }.unwrap().is_none(), "the arena's only empty span was unmapped");
    }
    assert!(arena.has_retained());
    // The first span empties too; the arena keeps one empty span already.
    let mut unmapped = None;
    for &ptr in &first {
        if let Some(span) = unsafe { arena.free(ptr, block.size()) }.unwrap() {
            assert!(unmapped.is_none());
            unmapped = Some(span);
        }
    }
    let span = unmapped.expect("the second empty span is given back");
    assert_eq!(span.base.as_ptr() as usize, base(first[0]));
    assert_eq!(arena.spans(), 1);
    let frames: HashSet<u64> = (0..512).filter_map(|page| translate(&world.phys, world.root, span.token.0.start + page * PAGE)).collect();
    assert_eq!(frames.len(), 512);
    world.unmap_span(span);
    assert!(world.offered.borrow().is_empty(), "frames were offered back before the flush");
    assert_eq!(world.flush(), 512);
    let offered: HashSet<u64> = world.offered.borrow().iter().copied().collect();
    assert_eq!(offered, frames, "the unmapped span's frames, and only those, come back after the flush");

    // The kept span serves the next request without a new mapping.
    let again = span_alloc(&world, &mut arena, block, &mut mapped);
    assert_eq!((mapped, base(again)), (2, base(second[0])));
    // A free with a size the block does not hold is refused, and the block stays live.
    assert!(unsafe { arena.free(again, 4096) }.is_err());
    assert!(unsafe { arena.free(again, block.size()) }.unwrap().is_none());
}

/// A span block with a free block after it grows and shrinks in place
/// through its arena, keeping its bytes (6.4, 6.6).
#[test]
fn a_span_block_resizes_in_place() {
    let world = World::new(WindowId::Span, 301, 512);
    let mut arena = SpanArena::new(0);
    let mut mapped = 0;
    let layout = Layout::from_size_align(8192, 8).unwrap();
    let ptr = span_alloc(&world, &mut arena, layout, &mut mapped);
    unsafe { ptr.as_ptr().write_bytes(0x5a, 8192) };
    assert!(unsafe { arena.resize(ptr, 8192, 256 << 10) }.unwrap(), "a block with room after it did not grow");
    assert!(unsafe { arena.resize(ptr, 256 << 10, 12 << 10) }.unwrap());
    let bytes = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), 8192) };
    assert!(bytes.iter().all(|&byte| byte == 0x5a));
    assert!(unsafe { arena.free(ptr, 12 << 10) }.unwrap().is_none());
    assert_eq!(mapped, 1);
}

// Pressure (7.2 step 2): one world per test thread, reached from the drain
// and shrinker hooks, which are plain functions.
thread_local! {
    static PRESSURE: Cell<Option<&'static Pressure>> = const { Cell::new(None) };
}

/// Two CPUs' span arenas behind simulated locks, over one span window.
struct Pressure {
    world: World,
    arenas: [SimLock<SpanArena>; 2],
}

fn pressure() -> &'static Pressure {
    PRESSURE.with(Cell::get).unwrap()
}

/// The span arenas' step-2 drain, as `large.rs` runs it: each arena's kept
/// empty span back to the window, skipping an arena whose lock is busy. On
/// x86 the frames wait for the flush, so it reports 0 free yet.
fn drain_spans() -> usize {
    let p = pressure();
    for arena in &p.arenas {
        let Some(mut arena) = arena.try_lock() else { continue };
        if let Some(span) = arena.take_retained() {
            p.world.unmap_span(span);
        }
    }
    0
}

/// x86's `kva-pending` shrinker: flush the window's pending unmaps.
fn pending_count() -> usize {
    (pressure().world.window.borrow().extents.retired_bytes() / PAGE) as usize
}

fn pending_scan(_target: usize, _ctx: ReclaimCtx) -> usize {
    pressure().world.flush()
}

static SPAN_DRAIN: Drain = Drain::new("span-empty", drain_spans);
static PENDING: Shrinker = Shrinker::new("kva-pending", 20, pending_count, pending_scan);

/// A pool with no memory: the frame request comes from outside the front.
struct NoPool;

impl Pool for NoPool {
    fn take(&self, _: Layout, _: bool) -> Result<NonNull<u8>, Miss> {
        Err(Miss::Exhausted)
    }
    fn contains(&self, _: NonNull<u8>) -> bool {
        false
    }
    unsafe fn give(&self, _: NonNull<u8>) {}
    unsafe fn resize(&self, _: NonNull<u8>, _: usize) -> bool {
        false
    }
}

/// A slab layer with no magazines to drain.
struct NoSmall;

impl Small for NoSmall {
    fn serves(&self, _: Layout) -> bool {
        false
    }
    fn take(&self, _: Layout, _: bool) -> Result<NonNull<u8>, Miss> {
        Err(Miss::Exhausted)
    }
    unsafe fn give(&self, _: NonNull<u8>, _: Layout) {}
    fn same_object(&self, _: Layout, _: Layout) -> bool {
        false
    }
    fn drain_own(&self, _: bool) {}
    fn drain(&self, _: usize) -> bool {
        true
    }
    fn cpus(&self) -> usize {
        2
    }
    fn release_freed(&self, _: usize) -> usize {
        0
    }
}

struct SimCpu;

impl Cpu for SimCpu {
    fn in_interrupt(&self) -> bool {
        false
    }
    fn id(&self) -> usize {
        0
    }
    fn masked<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

/// Pressure across CPUs (7.2 step 2): the frame allocator is empty, and the
/// only free memory is CPU 1's kept empty span. A frame request on CPU 0
/// runs the slow path, which unmaps that span through the registered drain,
/// and the `kva-pending` shrinker's flush then offers its 512 frames back.
/// With CPU 1's arena lock held under the fixed schedule, the drain skips it
/// without waiting and the span stays.
#[test]
fn the_slow_path_unmaps_each_cpus_kept_span_and_frees_its_frames_after_the_flush() {
    for held in [false, true] {
        let p: &'static Pressure = Box::leak(Box::new(Pressure {
            world: World::new(WindowId::Span, 302, 512),
            arenas: [SimLock::new(SpanArena::new(0)), SimLock::new(SpanArena::new(1))],
        }));
        PRESSURE.with(|cell| cell.set(Some(p)));
        {
            let mut arena = p.arenas[1].lock();
            let mut mapped = 0;
            let block = Layout::from_size_align(64 << 10, 8).unwrap();
            let ptr = span_alloc(&p.world, &mut arena, block, &mut mapped);
            assert!(unsafe { arena.free(ptr, block.size()) }.unwrap().is_none());
            assert!(arena.has_retained() && p.world.pool.borrow().is_empty(), "the frame allocator is empty");
        }
        if held {
            p.arenas[1].hold_elsewhere(None);
        }
        let (shrinkers, reclaim, stats) = (Registry::new(), Reclaim::new(), Stats::new());
        shrinkers.register_drain(&SPAN_DRAIN);
        shrinkers.register(&PENDING);
        let front = Front { main: &NoPool, reserve: &NoPool, small: &NoSmall, shrinkers: &shrinkers, reclaim: &reclaim, stats: &stats };
        let freed = front.reclaim_frames(1, &SimCpu);
        if held {
            assert_eq!((freed, p.arenas[1].waits()), (0, 0), "a held arena lock was waited for or its span taken");
            assert_eq!(p.arenas[1].try_locks(), 1, "the drain tried CPU 1's arena once");
            assert!(p.world.offered.borrow().is_empty());
        } else {
            assert_eq!(freed, 512, "the slow path did not give the kept span's frames back");
            let offered: HashSet<u64> = p.world.offered.borrow().iter().copied().collect();
            let taken: HashSet<u64> = p.world.taken.borrow().iter().copied().collect();
            assert_eq!(offered, taken);
            assert!(!p.arenas[1].lock().has_retained());
        }
    }
}

/// The vmap window as `vmap.rs` reaches it, over the world's tables.
struct HostVmap<'a>(&'a World);

impl vmap::Space for HostVmap<'_> {
    type Error = &'static str;

    fn reserve(&mut self, len: u64, align: u64) -> Result<Allocation, &'static str> {
        self.0.window.borrow_mut().reserve(len, align).map_err(|_| "no VA")
    }
    fn extend(&mut self, allocation: &Allocation, extra: u64) -> Option<Allocation> {
        self.0.window.borrow_mut().extend(allocation, extra).ok()
    }
    fn settle(&mut self) {
        self.0.window.borrow_mut().settle();
    }
    fn retract(&mut self, allocation: &Allocation, extra: u64) -> Allocation {
        self.0.window.borrow_mut().retract(allocation, extra).unwrap()
    }
    fn map_fresh(&mut self, allocation: &Allocation, offset: u64, len: u64) -> Result<(), &'static str> {
        let frames = self.0.take_frames(len / PAGE)?;
        let world = self.0;
        kva_map::map_range::<X86, _>(&world.phys, world.root, allocation.start + offset, len, 3, |o| frames[(o / PAGE) as usize], &mut TableSource(&world.phys))
            .map(|_| ())
            .map_err(|_| "map")
    }
    fn map_moved(&mut self, from: &Allocation, to: &Allocation, moved: u64) -> Result<(), &'static str> {
        let frames = self.0.take_frames((to.len - moved) / PAGE)?;
        let world = self.0;
        kva_window::remap::<X86, _>(&world.phys, world.root, from, to, moved, |o| frames[((o - moved) / PAGE) as usize], &mut TableSource(&world.phys))
            .map(|_| ())
            .map_err(|_| "remap")
    }
    fn forward(&mut self, allocation: Allocation) {
        let world = self.0;
        world.window.borrow_mut().forward::<X86, _>(&world.phys, world.root, &allocation).unwrap();
    }
    fn release(&mut self, allocation: Allocation) {
        let world = self.0;
        world.window.borrow_mut().unmap::<X86, _>(&world.phys, world.root, &allocation).unwrap();
    }
}

/// The frames backing `[va, va + len)`, page by page.
fn frames_of(world: &World, va: u64, len: u64) -> Vec<Option<u64>> {
    (0..len / PAGE).map(|page| translate(&world.phys, world.root, va + page * PAGE)).collect()
}

/// Read `len` bytes at `va` through the tables, as a CPU would.
fn read(world: &World, va: u64, len: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len as usize);
    for page in (va..va + len).step_by(PAGE as usize) {
        let pa = translate(&world.phys, world.root, page).expect("an unmapped page in a live block");
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(world.phys.bytes(pa), PAGE as usize) });
    }
    out
}

/// vmap (6.5): a 4 MiB block is backed by scattered order-0 frames; a
/// resize with free VA after it grows in place; one without moves the
/// PTEs to a new range, where each old page's own frame now sits, so the
/// bytes are unchanged and no byte or frame was copied; the old range waits
/// for the flush, which gives back no frame of it; and freeing everything
/// gives each frame back exactly once.
#[test]
fn a_vmap_block_grows_in_place_or_moves_its_ptes_without_copying() {
    let world = World::new(WindowId::Vmap, 310, 4096);
    let mut space = HostVmap(&world);
    let mut block = vmap::allocate(&mut space, 4 << 20, 8).unwrap();
    let start = block.allocation.start;
    let frames: Vec<u64> = frames_of(&world, start, 4 << 20).into_iter().map(Option::unwrap).collect();
    assert_eq!(frames.iter().collect::<HashSet<_>>().len(), 1024);
    assert!(frames.windows(2).any(|pair| pair[1] != pair[0] + PAGE), "the fixture's frames are scattered");
    assert_eq!(translate(&world.phys, world.root, start + (4 << 20)), None, "a guard page after the block");
    for (index, &pa) in frames.iter().enumerate() {
        unsafe { world.phys.bytes(pa).write_bytes(index as u8 ^ 0xa5, PAGE as usize) };
    }
    let before = read(&world, start, 4 << 20);

    assert_eq!(vmap::resize(&mut space, &mut block, 6 << 20), Ok(vmap::Resized::InPlace));
    assert_eq!(block.allocation.start, start);
    assert_eq!(read(&world, start, 4 << 20), before);
    assert!(frames_of(&world, start, 6 << 20).iter().all(Option::is_some));
    assert_eq!(translate(&world.phys, world.root, start + (6 << 20)), None, "the guard page moved past the growth");

    let blocker = vmap::allocate(&mut space, 1 << 20, 8).unwrap();
    assert_eq!(blocker.allocation.extent.start, start + (6 << 20) + PAGE, "the second block sits right after the first");
    let old: Vec<u64> = frames_of(&world, start, 6 << 20).into_iter().map(Option::unwrap).collect();
    let taken = world.taken.borrow().len();
    world.touch(start, 6 << 20);

    assert_eq!(vmap::resize(&mut space, &mut block, 10 << 20), Ok(vmap::Resized::Moved));
    let moved = block.allocation.start;
    assert_ne!(moved, start);
    let now: Vec<u64> = frames_of(&world, moved, 6 << 20).into_iter().map(Option::unwrap).collect();
    assert_eq!(now, old, "each old page's frame sits at the same offset of the new range");
    assert_eq!(world.taken.borrow().len() - taken, 1024, "only the growth took frames");
    assert_eq!(read(&world, moved, 4 << 20), before, "the bytes moved with their frames");
    assert!(frames_of(&world, start, 6 << 20).iter().all(Option::is_none), "the old range still translates");
    assert_eq!(world.flush(), 0, "the flush of the moved range gave frames back");
    assert_eq!(read(&world, moved, 4 << 20), before, "the flush of the old range touched the new one");

    // A shrink keeps the block and its pages for a later growth within them.
    assert_eq!(vmap::resize(&mut space, &mut block, 700 << 10), Ok(vmap::Resized::InPlace));
    assert_eq!((block.allocation.start, block.size), (moved, 700 << 10));
    vmap::free(&mut space, block);
    vmap::free(&mut space, blocker);
    world.flush();
    let mut offered = world.offered.borrow().clone();
    let mut handed = world.taken.borrow().clone();
    offered.sort();
    handed.sort();
    assert_eq!(offered, handed, "each frame came back exactly once");
}

/// A vmap growth into the free VA after the block that cannot get its
/// frames (6.5) leaves the block and the window as they were: the block's
/// VA is its 1 MiB, and the 7 MiB the growth would have taken is free again
/// for other requests.
#[test]
fn a_failed_vmap_growth_gives_its_va_back() {
    let world = World::new(WindowId::Vmap, 311, 512);
    let mut space = HostVmap(&world);
    let mut block = vmap::allocate(&mut space, 1 << 20, 8).unwrap();
    let before = block;
    let free = world.window.borrow().extents.free_bytes();
    assert_eq!(vmap::resize(&mut space, &mut block, 8 << 20), Err("no frames"));
    assert_eq!(block, before, "a failed growth changed the block");
    assert_eq!(world.window.borrow().extents.free_bytes(), free, "a failed growth kept VA out of the window");
    let next = vmap::allocate(&mut space, 4096, 8).unwrap();
    assert_eq!(next.allocation.extent.start, block.allocation.extent.end(), "the VA after the block is not free");
    vmap::free(&mut space, next);
    vmap::free(&mut space, block);
}

/// The address space of a VMware-shaped ARM64 guest for the DMA check: the
/// direct map over 2 GiB of RAM from 1 GiB, whose linked window translates
/// to the IPA plus `ram_base_offset` (`arm64_boot_contract::kernel_va_to_ipa`),
/// a kernel image whose pages are mapped one by one, and two windows.
struct GuestSpace {
    image: HashMap<u64, u64>,
}

const DIRECT: u64 = arm64_boot_contract::HHDM_BASE;
const IMAGE: u64 = 0xffff_ffff_8000_0000;
const SPAN_BASE: u64 = 0xffff_0100_0000_0000;
const VMAP_BASE: u64 = 0xffff_0200_0000_0000;
const RAM_OFFSET: u64 = 0x4000_0000;

impl dma_core::AddressSpace for GuestSpace {
    fn region(&self, va: u64) -> dma_core::Region {
        use dma_core::Region;
        if (SPAN_BASE..SPAN_BASE + (1 << 30)).contains(&va) {
            Region::Window { window: WindowId::Span as u8 }
        } else if (VMAP_BASE..VMAP_BASE + (4 << 30)).contains(&va) {
            Region::Window { window: WindowId::Vmap as u8 }
        } else if self.image.contains_key(&(va & !(PAGE - 1))) {
            Region::Image
        } else if (DIRECT + (1 << 30)..DIRECT + (3 << 30)).contains(&va) {
            Region::DirectMap
        } else {
            Region::Other
        }
    }

    fn bus(&self, va: u64) -> Option<u64> {
        match self.region(va) {
            dma_core::Region::DirectMap => Some(arm64_boot_contract::kernel_va_to_ipa(va, RAM_OFFSET)),
            dma_core::Region::Image => Some(self.image[&(va & !(PAGE - 1))] + (va & (PAGE - 1))),
            _ => None,
        }
    }
}

/// `dma::bus_address` (5.6): span and vmap addresses are refused; a
/// direct-map range that crosses the end of the relocated linked window,
/// where the device's addresses jump, is refused at the jump; an image
/// range whose second page is mapped elsewhere is refused; contiguous
/// direct-map and image ranges are accepted with the first byte's address.
#[test]
fn bus_address_takes_only_physically_contiguous_direct_map_or_image_ranges() {
    use dma_core::{bus_address, DmaError};
    // Image pages 0 and 1 are contiguous; page 2 is not.
    let image = [(0, 0x4020_0000u64), (1, 0x4020_1000), (2, 0x4100_0000)]
        .into_iter()
        .map(|(page, pa)| (IMAGE + page * PAGE, pa))
        .collect();
    let space = GuestSpace { image };

    assert_eq!(bus_address(&space, SPAN_BASE + 0x40, 64), Err(DmaError::Window { window: 0 }));
    assert_eq!(bus_address(&space, VMAP_BASE + 3 * PAGE, 2 * PAGE), Err(DmaError::Window { window: 1 }));
    assert_eq!(bus_address(&space, DIRECT + 0x8000_0000, 0), Err(DmaError::BadRange));
    assert_eq!(bus_address(&space, DIRECT + 0x10, 64), Err(DmaError::NotDmaMemory));

    let window_end = DIRECT + arm64_boot_contract::LINKED_RAM_START + arm64_boot_contract::LINKED_RAM_SIZE;
    assert_eq!(bus_address(&space, window_end - PAGE - 0x100, 2 * PAGE), Err(DmaError::Discontiguous { offset: PAGE + 0x100 }));
    assert_eq!(bus_address(&space, window_end + 0x10, 3 * PAGE), Ok(0x6000_0010));
    assert_eq!(bus_address(&space, DIRECT + 0x4000_2000, 3 * PAGE), Ok(0x8000_2000));

    assert_eq!(bus_address(&space, IMAGE + 0x80, PAGE + 0x100), Ok(0x4020_0080));
    assert_eq!(bus_address(&space, IMAGE + PAGE + 0x80, PAGE), Err(DmaError::Discontiguous { offset: PAGE - 0x80 }));
}

/// The ARM64 image ranges the DMA check accepts (5.6) are the loaded image
/// and the `.dma` section, laid out as `linker.ld` places them: the gap
/// between `__kernel_writable_end` and `.dma` at physical `0x5000_0000`,
/// which holds the loader's boot stack and which the boot direct map
/// translates, is not image memory.
#[test]
fn the_arm64_image_ranges_leave_out_the_gap_before_dma() {
    let va = |pa: u64| arm64_boot_contract::HHDM_BASE + pa;
    let image = dma_core::ImageLayout {
        start: va(0x4020_0000),
        loaded_end: va(0x4280_0000),
        dma_start: va(0x5000_0000),
        dma_end: va(0x5000_4000),
    };
    let ranges = dma_core::image_ranges(image);
    let image_at = |pa: u64| dma_core::in_ranges(&ranges, va(pa));
    assert!(image_at(0x4020_0000) && image_at(0x427f_f000), "the loaded image");
    assert!(image_at(0x5000_0000) && image_at(0x5000_3fff), "the .dma section");
    let stack = arm64_boot_contract::LOADER_BOOT_STACK_BOTTOM_PHYS..arm64_boot_contract::LOADER_BOOT_STACK_TOP_PHYS;
    assert!(stack.clone().step_by(PAGE as usize).all(|pa| !image_at(pa)), "the loader boot stack counts as image memory");
    assert!(!image_at(0x4280_0000) && !image_at(0x4fff_f000), "the gap before .dma counts as image memory");
}

/// Routing (6.1): over each size in {1, 16, 4096, 4097, 512 KiB,
/// 512 KiB + 1, 4 MiB} and each alignment in {1, 16, 4096, 8192, 2 MiB,
/// 1 GiB}, the layout reaches one layer whose limits hold it: the slab
/// layer and the span arenas only up to 4096 alignment (a span's in-band
/// tag cannot sit before a 2 MiB-aligned block at its start), and vmap for
/// the rest, up to the vmap window's size. The design's first-draft table,
/// applied to the same grid as a model, matches two rows for
/// (4 MiB, 8 KiB), 0 rows for 1 GiB alignment, and sends (4096, 2 MiB) to a
/// span.
#[test]
fn every_layout_routes_to_exactly_one_layer_that_holds_it() {
    let sizes = [1, 16, 4096, 4097, 512 << 10, (512 << 10) + 1, 4 << 20];
    let aligns = [1, 16, 4096, 8192, 2 << 20, 1 << 30];
    let vmap_bytes = 4u64 << 30;
    // The first draft's rows, each tested on its own.
    let draft = |size: usize, align: usize| -> Vec<Layer> {
        let rows = [
            (size <= 4096 && align <= 4096, Layer::Slab),
            (size <= 512 << 10 && align <= 2 << 20, Layer::Span),
            (size > 512 << 10 && align <= 2 << 20, Layer::Vmap),
            (align > 4096 && align <= 2 << 20, Layer::Vmap),
        ];
        rows.into_iter().filter(|row| row.0).map(|row| row.1).collect()
    };
    assert_eq!(draft(4 << 20, 8192).len(), 2);
    assert!(aligns.iter().all(|_| sizes.iter().all(|&size| draft(size, 1 << 30).is_empty())));
    assert_eq!(draft(4096, 2 << 20).first(), Some(&Layer::Span));

    for &size in &sizes {
        for &align in &aligns {
            let layout = Layout::from_size_align(size, align).unwrap();
            let layer = kmem_core::route(layout, vmap_bytes).unwrap_or_else(|e| panic!("({size}, {align}) refused: {e:?}"));
            let holds = match layer {
                Layer::Slab => size <= 4096 && align <= 4096,
                Layer::Span => span_arena::serves(layout) && !(size <= 4096 && align <= 4096),
                Layer::Vmap => !span_arena::serves(layout) && align as u64 <= vmap_bytes,
            };
            assert!(holds, "({size}, {align}) routed to {layer:?}");
        }
    }
    let huge = Layout::from_size_align(16, 8 << 30).unwrap();
    assert_eq!(kmem_core::route(huge, vmap_bytes), Err(AllocError::NoVa { window: 1 }), "an alignment no window can meet");
}

/// `large::reallocate`'s fallback (6.6) keeps a moved block in a layer
/// `large::deallocate` frees, the span or vmap window, at each size and
/// alignment of the routing grid, including a span block grown to 4 KiB
/// at 4096 alignment, which the slab layer would serve as a fresh request.
#[test]
fn a_moved_large_block_stays_in_a_layer_large_frees() {
    let sizes = [1, 16, 4096, 4097, 512 << 10, (512 << 10) + 1, 4 << 20];
    let aligns = [1, 16, 4096, 8192, 2 << 20];
    for &size in &sizes {
        for &align in &aligns {
            let layout = Layout::from_size_align(size, align).unwrap();
            let layer = kmem_core::moved_layer(layout, 4 << 30).unwrap();
            assert_ne!(layer, Layer::Slab, "({size}, {align}) moves to the slab layer, which large::deallocate cannot free");
            if layer == Layer::Span {
                assert!(span_arena::serves(layout), "({size}, {align}) moves to a span that cannot hold it");
            }
        }
    }
}

/// Span search (6.4): a request whose worst case is above every free extent
/// is still served from an extent that holds it at its actual alignment.
/// One span is filled with 5000-byte blocks aligned to 4096, the gaps their
/// alignment leaves are filled with 64-byte blocks, and one 5000-byte block
/// is freed. Its extent, about 5 KiB, holds 4097 bytes at the old 4096-aligned
/// address, below the request's 8200-byte worst case.
#[test]
fn a_span_serves_an_aligned_request_from_a_hole_below_its_worst_case() {
    let world = World::new(WindowId::Span, 303, 512);
    let mut arena = SpanArena::new(0);
    let mut mapped = 0;
    let big = Layout::from_size_align(5000, 4096).unwrap();
    let mut blocks = vec![span_alloc(&world, &mut arena, big, &mut mapped)];
    while let Ok(ptr) = arena.allocate(big) {
        blocks.push(ptr);
    }
    let filler = Layout::from_size_align(64, 8).unwrap();
    while arena.allocate(filler).is_ok() {}
    assert!(mapped == 1 && blocks.len() > 100, "the fixture fills one span");
    let hole = blocks[blocks.len() / 2];
    assert!(unsafe { arena.free(hole, big.size()) }.unwrap().is_none());
    let request = Layout::from_size_align(4097, 4096).unwrap();
    assert_eq!(arena.allocate(request), Ok(hole), "the arena asked for a new span with a fitting hole");
}
