//! E1-4: allocation failure, contexts and reclaim, over the kernel's heap_index.
//!
//! One simulated CPU drives the front. Other CPUs exist only as holders of a
//! lock under a fixed schedule (`sim::SimLock`), so the interleavings are
//! fixed, and the tests use neither threads nor time.
use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

#[allow(dead_code)]
#[path = "../../../kernel/src/memory/heap_index.rs"]
mod heap_index;
// The front's own code; the report-only helpers are the kernel's.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kmem_core.rs"]
mod kmem_core;
#[path = "kmem_front/sim.rs"]
mod sim;

use heap_index::Heap;
use kmem_core::{AllocError, Cpu, Ctx, Front, Miss, Pool, Reclaim, ReclaimCtx, Registry, Shrinker, Stats};
use sim::SimLock;

const ARENA_BYTES: usize = 64 * 1024;
const RESERVE_BYTES: usize = 16 * 1024;
const HELD_BLOCK: usize = 1024;
const PAGE: usize = 4096;
/// `Stats`'s index of `Ctx::Atomic`.
const ATOMIC: usize = 2;

/// A heap_index heap behind a simulated lock.
struct Arena {
    backing: Vec<u64>,
    lock: SimLock<Heap>,
}

impl Arena {
    fn new(bytes: usize) -> Self {
        let mut backing = vec![0u64; bytes / 8];
        let mut heap = Heap::empty();
        unsafe { heap.init(backing.as_mut_ptr().cast(), bytes) };
        Self { backing, lock: SimLock::new(heap) }
    }
}

impl Pool for Arena {
    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, Miss> {
        let mut heap = if wait { self.lock.lock() } else { self.lock.try_lock().ok_or(Miss::Busy)? };
        heap.allocate(layout).0.ok_or(Miss::Exhausted)
    }

    fn contains(&self, ptr: NonNull<u8>) -> bool {
        let start = self.backing.as_ptr() as usize;
        (start..start + self.backing.len() * 8).contains(&(ptr.as_ptr() as usize))
    }

    unsafe fn give(&self, ptr: NonNull<u8>) {
        unsafe { self.lock.lock().deallocate(ptr) }
    }
}

/// The simulated CPU. Masking interrupts on one CPU with no interrupt
/// source is the identity.
struct SimCpu {
    interrupt: bool,
}

impl Cpu for SimCpu {
    fn in_interrupt(&self) -> bool {
        self.interrupt
    }

    fn id(&self) -> usize {
        0
    }

    fn masked<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

/// A heap, its reserve, and the cache that holds most of the heap.
struct World {
    main: Arena,
    reserve: Arena,
    shrinkers: Registry,
    reclaim: Reclaim,
    stats: Stats,
    /// Blocks the cache holds and its shrinker can give back.
    held: RefCell<Vec<NonNull<u8>>>,
    /// The cache's own lock, which its shrinker takes with `try_lock`.
    cache_lock: SimLock<()>,
    scans: Cell<u32>,
    /// A request the shrinker makes before it gives memory back, and its result.
    scan_request: Cell<Option<Layout>>,
    scan_result: Cell<Option<Result<NonNull<u8>, AllocError>>>,
}

thread_local! {
    static WORLD: Cell<Option<&'static World>> = const { Cell::new(None) };
}

fn world() -> &'static World {
    WORLD.with(Cell::get).expect("no world on this thread")
}

fn cache_count() -> usize {
    world().held.borrow().len() * HELD_BLOCK / PAGE
}

/// The cache's shrinker: `try_lock` its lock, then free blocks from the end
/// of its list until the target's bytes are given back.
fn cache_scan(target: usize, _ctx: ReclaimCtx) -> usize {
    let world = world();
    world.scans.set(world.scans.get() + 1);
    let Some(_guard) = world.cache_lock.try_lock() else {
        return 0;
    };
    if let Some(layout) = world.scan_request.take() {
        let result = allocate(world, Request { layout, ctx: Ctx::Normal, in_interrupt: false });
        world.scan_result.set(Some(result));
    }
    let mut freed = 0;
    while freed < target * PAGE {
        let Some(ptr) = world.held.borrow_mut().pop() else {
            break;
        };
        unsafe { world.main.lock.lock().deallocate(ptr) };
        freed += HELD_BLOCK;
    }
    freed / PAGE
}

impl World {
    /// A heap filled to capacity by the cache's blocks, with an empty reserve
    /// and the cache's shrinker registered.
    fn full() -> &'static World {
        let world: &'static World = Box::leak(Box::new(World {
            main: Arena::new(ARENA_BYTES),
            reserve: Arena::new(RESERVE_BYTES),
            shrinkers: Registry::new(),
            reclaim: Reclaim::new(),
            stats: Stats::new(),
            held: RefCell::new(Vec::new()),
            cache_lock: SimLock::new(()),
            scans: Cell::new(0),
            scan_request: Cell::new(None),
            scan_result: Cell::new(None),
        }));
        WORLD.with(|cell| cell.set(Some(world)));
        let block = Layout::from_size_align(HELD_BLOCK, 8).unwrap();
        while let (Some(ptr), _) = world.main.lock.lock().allocate(block) {
            world.held.borrow_mut().push(ptr);
        }
        assert!(world.held.borrow().len() >= 8, "the cache holds enough to relieve a 4 KiB request");
        let shrinker = Box::leak(Box::new(Shrinker::new("cache", 1, cache_count, cache_scan)));
        world.shrinkers.register(shrinker);
        world
    }
}

/// One request as the CPU makes it.
struct Request {
    layout: Layout,
    ctx: Ctx,
    in_interrupt: bool,
}

/// The front under test.
fn allocate(world: &'static World, request: Request) -> Result<NonNull<u8>, AllocError> {
    let front = Front {
        main: &world.main,
        reserve: &world.reserve,
        shrinkers: &world.shrinkers,
        reclaim: &world.reclaim,
        stats: &world.stats,
    };
    front.allocate(request.layout, request.ctx, &SimCpu { interrupt: request.in_interrupt })
}

fn page() -> Layout {
    Layout::from_size_align(4096, 8).unwrap()
}

#[test]
fn a_full_heap_is_relieved_by_its_shrinker_after_one_scan() {
    let world = World::full();
    let ptr = allocate(world, Request { layout: page(), ctx: Ctx::Normal, in_interrupt: false });
    assert!(ptr.is_ok(), "a full heap whose cache can give back enough bytes failed the request");
    assert_eq!(world.scans.get(), 1, "the request ran the shrinker exactly once");
}

#[test]
fn an_interrupt_request_never_waits_for_a_held_heap_lock() {
    let world = World::full();
    // Another CPU holds the heap lock and releases it only after three polls.
    world.main.lock.hold_elsewhere(Some(3));
    let ptr = allocate(world, Request { layout: page(), ctx: Ctx::Normal, in_interrupt: true });
    assert_eq!(world.main.lock.waits(), 0, "an interrupt-context request waited on the heap lock");
    assert_eq!(world.main.lock.try_locks(), 1, "an interrupt-context request makes one try_lock");
    assert_eq!(world.scans.get(), 0, "an interrupt-context request ran a shrinker");
    // With the heap lock busy it is served from the reserve.
    assert!(ptr.is_ok(), "the reserve did not serve an interrupt-context request");
    assert!(world.reserve.lock.lock().used() > 0, "the request did not come from the reserve");
    assert_eq!(world.stats.refused[ATOMIC].load(Ordering::Relaxed), 1);
}

#[test]
fn an_atomic_request_the_reserve_cannot_serve_fails_without_waiting() {
    let world = World::full();
    world.main.lock.hold_elsewhere(Some(3));
    world.reserve.lock.hold_elsewhere(Some(3));
    let result = allocate(world, Request { layout: page(), ctx: Ctx::Atomic, in_interrupt: false });
    assert_eq!(result, Err(AllocError::Refused));
    assert_eq!((world.main.lock.try_locks(), world.main.lock.waits()), (1, 0));
    assert_eq!((world.reserve.lock.try_locks(), world.reserve.lock.waits()), (1, 0));
    assert_eq!(world.scans.get(), 0);
    // An exhausted heap and a reserve too small for the request.
    let world = World::full();
    let big = Layout::from_size_align(RESERVE_BYTES, 8).unwrap();
    let result = allocate(world, Request { layout: big, ctx: Ctx::Atomic, in_interrupt: false });
    assert_eq!(result, Err(AllocError::HeapExhausted));
    assert_eq!(world.scans.get(), 0);
}

#[test]
fn a_noreclaim_request_runs_no_shrinker() {
    let world = World::full();
    let result = allocate(world, Request { layout: page(), ctx: Ctx::NoReclaim, in_interrupt: false });
    assert_eq!(result, Err(AllocError::HeapExhausted));
    assert_eq!(world.scans.get(), 0, "a NoReclaim request ran a shrinker");
    assert_eq!(world.reserve.lock.try_locks(), 0, "a NoReclaim request used the reserve");
}

#[test]
fn a_shrinker_that_allocates_gets_an_atomic_allocation_and_no_recursive_reclaim() {
    let world = World::full();
    world.scan_request.set(Some(Layout::from_size_align(HELD_BLOCK, 8).unwrap()));
    let outer = allocate(world, Request { layout: page(), ctx: Ctx::Normal, in_interrupt: false });
    assert!(outer.is_ok(), "the outer request is met after the scan");
    let inner = world.scan_result.take().expect("the shrinker made its request");
    assert!(inner.is_ok(), "the shrinker's request is served from the reserve");
    assert_eq!(world.scans.get(), 1, "the shrinker's request reclaimed again");
    assert_eq!(world.stats.reclaim_busy.load(Ordering::Relaxed), 0, "the shrinker's request entered reclaim");
    assert_eq!(world.stats.slow[ATOMIC].load(Ordering::Relaxed), 1, "the shrinker's request did not run as Atomic");
    assert_eq!(world.stats.reserve_hits.load(Ordering::Relaxed), 1);
}

#[test]
fn a_shrinker_whose_lock_is_held_is_skipped_without_waiting() {
    let world = World::full();
    // The caller itself holds the cache's lock while it allocates.
    world.cache_lock.hold_elsewhere(None);
    let result = allocate(world, Request { layout: page(), ctx: Ctx::Normal, in_interrupt: false });
    assert_eq!(result, Err(AllocError::HeapExhausted));
    assert_eq!(world.scans.get(), 1);
    assert_eq!((world.cache_lock.try_locks(), world.cache_lock.waits()), (1, 0));
}

thread_local! {
    static ORDER: RefCell<Vec<(&'static str, usize)>> = const { RefCell::new(Vec::new()) };
}

fn one() -> usize {
    1
}

fn scan_named(name: &'static str, target: usize, gives: usize) -> usize {
    ORDER.with(|order| order.borrow_mut().push((name, target)));
    target.min(gives)
}

fn scan_cheap(target: usize, _: ReclaimCtx) -> usize {
    scan_named("cheap", target, 3)
}

fn scan_middle(target: usize, _: ReclaimCtx) -> usize {
    scan_named("middle", target, 10)
}

fn scan_dear(target: usize, _: ReclaimCtx) -> usize {
    scan_named("dear", target, 5)
}

static CHEAP: Shrinker = Shrinker::new("cheap", 1, one, scan_cheap);
static MIDDLE: Shrinker = Shrinker::new("middle", 2, one, scan_middle);
static DEAR: Shrinker = Shrinker::new("dear", 3, one, scan_dear);

#[test]
fn shrinkers_run_in_cost_order_and_stop_at_the_target() {
    let registry = Registry::new();
    for shrinker in [&DEAR, &CHEAP, &MIDDLE] {
        registry.register(shrinker);
    }
    assert_eq!(registry.reclaim(9), 9);
    let order = ORDER.with(|order| order.borrow().clone());
    assert_eq!(order, [("cheap", 9), ("middle", 6)], "a shrinker ran out of cost order or after the target was met");
}
