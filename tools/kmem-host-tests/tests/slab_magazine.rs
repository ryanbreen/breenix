//! E1-6: slab caches with per-CPU magazines, through the allocator's front.
//!
//! The front, the slab caches and the magazine layer are the kernel's own
//! code. The page layer underneath is simulated, with a page limit. CPUs are
//! simulated as explicit parameters under a fixed schedule: no threads and no
//! timing.
use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::sync::atomic::AtomicPtr;

#[allow(dead_code)]
#[path = "../../../kernel/src/memory/heap_index.rs"]
mod heap_index;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kmem_core.rs"]
mod kmem_core;
// The kernel's glue reads the descriptor cache and the overhead pages.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/magazine.rs"]
mod magazine;
#[path = "../../../kernel/src/memory/slab_cache.rs"]
mod slab_cache;

use heap_index::Heap;
use kmem_core::{cpu_of_base, AllocError, Cpu, Ctx, Front, Miss, Pool, Reclaim, ReclaimCtx, Registry, Shrinker, Small, Stats};
use magazine::{CpuCaches, Kmem, UPDATE_MS};
use slab_cache::{class_of, Backing, Short, Slab, CLASSES, CLASS_COUNT, PAGE};

const ARENA_BYTES: usize = 1 << 20;

/// A heap_index heap behind a lock that counts its acquisitions.
struct Arena {
    backing: Vec<u64>,
    heap: RefCell<Heap>,
    acquisitions: Cell<u64>,
}

impl Arena {
    fn new(bytes: usize) -> Self {
        let mut backing = vec![0u64; bytes / 8];
        let mut heap = Heap::empty();
        unsafe { heap.init(backing.as_mut_ptr().cast(), bytes) };
        Self { backing, heap: RefCell::new(heap), acquisitions: Cell::new(0) }
    }

    fn locked(&self) -> std::cell::RefMut<'_, Heap> {
        self.acquisitions.set(self.acquisitions.get() + 1);
        self.heap.borrow_mut()
    }
}

impl Pool for Arena {
    fn take(&self, layout: Layout, _wait: bool) -> Result<NonNull<u8>, Miss> {
        self.locked().allocate(layout).0.ok_or(Miss::Exhausted)
    }

    fn contains(&self, ptr: NonNull<u8>) -> bool {
        let start = self.backing.as_ptr() as usize;
        (start..start + self.backing.len() * 8).contains(&(ptr.as_ptr() as usize))
    }

    unsafe fn give(&self, ptr: NonNull<u8>) {
        unsafe { self.locked().deallocate(ptr) }
    }
}

/// The page layer: page-aligned host pages, at most `limit` at once, and the
/// per-page slab record that `FrameMeta` holds in the kernel.
struct SimPages {
    limit: Cell<usize>,
    in_use: RefCell<HashSet<usize>>,
    slabs: RefCell<HashMap<usize, usize>>,
    now: Cell<u64>,
    /// Another CPU holds the frame allocator's free-list lock.
    frame_lock_held: Cell<bool>,
    /// Page requests that waited for that held lock.
    frame_waits: Cell<u64>,
}

fn page_layout() -> Layout {
    Layout::from_size_align(PAGE, PAGE).unwrap()
}

impl Backing for SimPages {
    fn take_page(&self, wait: bool) -> Result<NonNull<u8>, Short> {
        if self.frame_lock_held.get() {
            if !wait {
                return Err(Short::Busy);
            }
            // A waiting request waits for the free-list lock; the holder is
            // modelled as releasing it.
            self.frame_waits.set(self.frame_waits.get() + 1);
        }
        if self.in_use.borrow().len() >= self.limit.get() {
            return Err(Short::NoPage);
        }
        let page = NonNull::new(unsafe { std::alloc::alloc(page_layout()) }).ok_or(Short::NoPage)?;
        self.in_use.borrow_mut().insert(page.as_ptr() as usize);
        Ok(page)
    }

    fn give_page(&self, page: NonNull<u8>) {
        let address = page.as_ptr() as usize;
        assert!(self.in_use.borrow_mut().remove(&address), "a page given back twice or never taken");
        assert!(!self.slabs.borrow().contains_key(&address), "a page given back with its slab record set");
        unsafe { std::alloc::dealloc(page.as_ptr(), page_layout()) };
    }

    fn set_slab(&self, page: NonNull<u8>, slab: *mut Slab) {
        let address = page.as_ptr() as usize;
        assert!(self.in_use.borrow().contains(&address), "a slab record for a page not taken");
        if slab.is_null() {
            self.slabs.borrow_mut().remove(&address);
        } else {
            self.slabs.borrow_mut().insert(address, slab as usize);
        }
    }

    fn slab(&self, page: NonNull<u8>) -> *mut Slab {
        self.slabs.borrow().get(&(page.as_ptr() as usize)).map_or(std::ptr::null_mut(), |&slab| slab as *mut Slab)
    }

    fn now_ms(&self) -> u64 {
        self.now.get()
    }
}

/// The small-object layer as the kernel's `KernelSmall` presents it, over the
/// simulated pages, on the CPU the schedule is running on.
struct SimSmall<'a> {
    kmem: &'a Kmem<SimPages>,
    cpu: Option<usize>,
    /// Off: the slow path's step 2 reaches no other CPU, as the plan's first
    /// draft had it.
    remote_drains: bool,
}

fn miss(short: Short) -> Miss {
    match short {
        Short::Busy => Miss::Busy,
        Short::NoPage => Miss::Exhausted,
    }
}

impl Small for SimSmall<'_> {
    fn serves(&self, layout: Layout) -> bool {
        class_of(layout.size(), layout.align()).is_some()
    }

    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, Miss> {
        let class = class_of(layout.size(), layout.align()).ok_or(Miss::Exhausted)?;
        self.kmem.alloc(class, self.cpu, wait).map_err(miss)
    }

    unsafe fn give(&self, ptr: NonNull<u8>, layout: Layout) {
        self.kmem.free(ptr, class_of(layout.size(), layout.align()).unwrap(), self.cpu);
    }

    fn same_object(&self, old: Layout, new: Layout) -> bool {
        let old = class_of(old.size(), old.align());
        old.is_some() && old == class_of(new.size(), new.align())
    }

    fn drain_own(&self, wait: bool) {
        if let Some(cpu) = self.cpu {
            self.kmem.drain(cpu, wait);
        }
    }

    fn drain(&self, cpu: usize) -> bool {
        if !self.remote_drains && Some(cpu) != self.cpu {
            return true;
        }
        self.kmem.drain(cpu, false)
    }

    fn cpus(&self) -> usize {
        self.kmem.slot_count()
    }
}

struct SimCpu {
    id: usize,
    interrupt: bool,
}

impl Cpu for SimCpu {
    fn in_interrupt(&self) -> bool {
        self.interrupt
    }

    fn id(&self) -> usize {
        self.id
    }

    fn masked<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

struct World {
    main: Arena,
    reserve: Arena,
    kmem: Kmem<SimPages>,
    shrinkers: Registry,
    reclaim: Reclaim,
    stats: Stats,
    cpu: Cell<Option<usize>>,
    remote_drains: Cell<bool>,
}

thread_local! {
    static WORLD: Cell<Option<&'static World>> = const { Cell::new(None) };
}

fn world() -> &'static World {
    WORLD.with(Cell::get).expect("no world on this thread")
}

fn slab_count() -> usize {
    world().kmem.reclaimable()
}

fn slab_scan(target: usize, _ctx: ReclaimCtx) -> usize {
    world().kmem.shrink(target)
}

impl World {
    /// A world whose firmware describes `cpus` CPUs, with 0 of them
    /// registered, over a page layer of at most `pages` pages, with the slab
    /// shrinker registered.
    fn new(cpus: usize, pages: usize) -> &'static World {
        let world: &'static World = Box::leak(Box::new(World {
            main: Arena::new(ARENA_BYTES),
            reserve: Arena::new(ARENA_BYTES / 16),
            kmem: Kmem::new(SimPages {
                limit: Cell::new(pages),
                in_use: RefCell::new(HashSet::new()),
                slabs: RefCell::new(HashMap::new()),
                now: Cell::new(0),
                frame_lock_held: Cell::new(false),
                frame_waits: Cell::new(0),
            }),
            shrinkers: Registry::new(),
            reclaim: Reclaim::new(),
            stats: Stats::new(),
            cpu: Cell::new(None),
            remote_drains: Cell::new(true),
        }));
        WORLD.with(|cell| cell.set(Some(world)));
        let slots: Vec<AtomicPtr<CpuCaches>> = (0..cpus).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect();
        assert!(world.kmem.install_slots(Box::leak(slots.into_boxed_slice())));
        world.shrinkers.register(Box::leak(Box::new(Shrinker::new("slab", 10, slab_count, slab_scan))));
        world
    }

    /// A world of `cpus` registered CPUs and an ample page layer.
    fn registered(cpus: usize) -> &'static World {
        let world = World::new(cpus, usize::MAX);
        for cpu in 0..cpus {
            assert!(world.register(cpu));
        }
        world
    }

    fn register(&self, cpu: usize) -> bool {
        self.kmem.register(cpu, Box::leak(Box::new(CpuCaches::new())))
    }

    fn on(&self, cpu: usize) {
        self.cpu.set(Some(cpu));
    }

    fn try_alloc(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        self.try_alloc_on(layout, false)
    }

    /// A `Normal` request from an interrupt handler, which the front lowers
    /// to `Atomic`.
    fn try_alloc_in_interrupt(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        self.try_alloc_on(layout, true)
    }

    fn try_alloc_on(&self, layout: Layout, interrupt: bool) -> Result<NonNull<u8>, AllocError> {
        let small = SimSmall { kmem: &self.kmem, cpu: self.cpu.get(), remote_drains: self.remote_drains.get() };
        let front = Front {
            main: &self.main,
            reserve: &self.reserve,
            small: &small,
            shrinkers: &self.shrinkers,
            reclaim: &self.reclaim,
            stats: &self.stats,
        };
        front.allocate(layout, Ctx::Normal, &SimCpu { id: self.cpu.get().unwrap_or(0), interrupt })
    }

    fn alloc(&self, layout: Layout) -> NonNull<u8> {
        self.try_alloc(layout).expect("allocation")
    }

    fn free(&self, ptr: NonNull<u8>, layout: Layout) {
        let small = SimSmall { kmem: &self.kmem, cpu: self.cpu.get(), remote_drains: true };
        let front = Front {
            main: &self.main,
            reserve: &self.reserve,
            small: &small,
            shrinkers: &self.shrinkers,
            reclaim: &self.reclaim,
            stats: &self.stats,
        };
        unsafe { front.deallocate(ptr, layout) }
    }

    /// Acquisitions of the locks the CPUs share: the heap's and the
    /// reserve's, and each depot, slab, descriptor and magazine cache lock.
    fn shared_acquisitions(&self) -> u64 {
        self.main.acquisitions.get() + self.reserve.acquisitions.get() + self.kmem.shared_acquisitions()
    }

    fn pages_in_use(&self) -> usize {
        self.kmem.backing().in_use.borrow().len()
    }

    /// Each class's objects are live, in a CPU's magazine, in the depot, or
    /// free in a slab, and the slab layer's count of handed-out objects is
    /// the first three together.
    fn assert_conserved(&self, live: &HashMap<usize, Layout>) {
        let mut by_class = [0usize; CLASS_COUNT];
        for layout in live.values() {
            by_class[class_of(layout.size(), layout.align()).unwrap()] += 1;
        }
        for (class, &live) in by_class.iter().enumerate() {
            let stats = self.kmem.class_stats(class);
            assert_eq!(
                live + stats.cpu_rounds + stats.depot_rounds,
                stats.slabs.objects,
                "class {}: live + magazine rounds + depot rounds is not the objects the slabs handed out",
                CLASSES[class]
            );
            assert!(stats.slabs.objects <= stats.slabs.slabs * stats.slabs.per_slab);
        }
    }
}

fn bytes(size: usize) -> Layout {
    Layout::from_size_align(size, 8).unwrap()
}

/// Check first (E1-6): 4 CPUs allocate and free 64-byte objects under a fixed
/// schedule. Once each CPU has run once, its operations must take no lock
/// another CPU takes. The fixed heap's front took its global lock on each.
#[test]
fn small_objects_on_four_cpus_take_no_shared_lock_on_the_fast_path() {
    let world = World::registered(4);
    let layout = bytes(64);
    // Each CPU's first operations may fill its per-CPU layer.
    for cpu in 0..4 {
        world.on(cpu);
        let objects: Vec<_> = (0..8).map(|_| world.alloc(layout)).collect();
        for object in objects {
            world.free(object, layout);
        }
    }
    let before = world.shared_acquisitions();
    let mut operations = 0;
    for _round in 0..16 {
        for cpu in 0..4 {
            world.on(cpu);
            let objects: Vec<_> = (0..4).map(|_| world.alloc(layout)).collect();
            for object in objects {
                world.free(object, layout);
            }
            operations += 8;
        }
    }
    assert_eq!(
        world.shared_acquisitions() - before,
        0,
        "{operations} fast-path operations on 4 CPUs took a lock the CPUs share"
    );
}

/// A fixed xorshift sequence.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn objects_are_conserved_and_never_handed_out_twice_seed_5eede106() {
    let world = World::registered(4);
    let sizes = [8, 16, 40, 64, 100, 160, 200, 512, 700, 1360, 2000, 4096];
    let mut rng = Rng(0x5eed_e106);
    let mut live: HashMap<usize, Layout> = HashMap::new();
    for step in 0..40_000 {
        world.on(rng.below(4));
        if live.is_empty() || (live.len() < 3_000 && rng.below(2) == 0) {
            let layout = bytes(sizes[rng.below(sizes.len())]);
            let object = world.alloc(layout);
            assert!(live.insert(object.as_ptr() as usize, layout).is_none(), "an object was handed out twice");
        } else {
            // Free a live object, often on a CPU other than its allocator's.
            let index = rng.below(live.len());
            let (&address, &layout) = live.iter().nth(index).unwrap();
            live.remove(&address);
            world.free(NonNull::new(address as *mut u8).unwrap(), layout);
        }
        if step % 1_000 == 0 {
            world.assert_conserved(&live);
        }
    }
    world.assert_conserved(&live);
    for (address, layout) in live.drain() {
        world.free(NonNull::new(address as *mut u8).unwrap(), layout);
    }
    for cpu in 0..4 {
        assert!(world.kmem.drain(cpu, true));
    }
    world.kmem.shrink(usize::MAX);
    assert_eq!(world.pages_in_use(), 0, "pages stayed held after every object was freed and the layer shrunk");
}

#[test]
fn an_object_freed_on_another_cpu_returns_to_a_slab_of_its_cache() {
    let world = World::registered(2);
    let layout = bytes(64);
    let class = class_of(64, 8).unwrap();
    world.on(0);
    let object = world.alloc(layout);
    world.on(1);
    world.free(object, layout);
    assert_eq!(
        world.kmem.class_stats(class).cpu_rounds,
        16,
        "CPU 0's 15-round fill after its own object, plus CPU 1's free"
    );
    for cpu in 0..2 {
        assert!(world.kmem.drain(cpu, true));
    }
    world.kmem.trim_depot(class);
    let stats = world.kmem.class_stats(class);
    assert_eq!((stats.slabs.objects, stats.slabs.slabs, stats.slabs.empty), (0, 1, 1));
    // No other class received it.
    for other in (0..CLASS_COUNT).filter(|&other| other != class) {
        assert_eq!(world.kmem.class_stats(other).slabs.slabs, 0);
    }
}

#[test]
fn cpus_register_into_a_table_sized_from_the_firmware_count() {
    let world = World::new(12, usize::MAX);
    assert_eq!(world.kmem.slot_count(), 12);
    let layout = bytes(64);
    let class = class_of(64, 8).unwrap();
    let mut live: HashMap<usize, Layout> = HashMap::new();
    let alloc = |world: &World, live: &mut HashMap<usize, Layout>| {
        let object = world.alloc(layout);
        assert!(live.insert(object.as_ptr() as usize, layout).is_none(), "an object was handed out twice");
        object
    };
    // Before CPU 0 publishes its slot, its requests take the shared path.
    world.on(0);
    let early: Vec<_> = (0..3).map(|_| alloc(world, &mut live)).collect();
    live.remove(&(early[2].as_ptr() as usize));
    world.free(early[2], layout);
    assert_eq!(world.kmem.class_stats(class).slabs.objects, 2, "a shared-path free did not reach the slab");
    world.assert_conserved(&live);

    assert!(world.register(0));
    assert!(world.register(1));
    // CPU 2's block exists before it is published, and no CPU may touch it.
    let cpu2: &'static CpuCaches = Box::leak(Box::new(CpuCaches::new()));
    world.on(0);
    let a = alloc(world, &mut live);
    // CPU 0's first fill came from the slab the shared path used.
    assert_eq!(world.kmem.class_stats(class).slabs.slabs, 1, "the shared path's slab was not found afterwards");
    world.on(1);
    let b = alloc(world, &mut live);
    assert_eq!(cpu2.acquisitions(), 0, "an operation touched CPU 2's slot before CPU 2 published it");
    assert!(world.kmem.register(2, cpu2));
    world.on(0);
    live.remove(&(a.as_ptr() as usize));
    world.free(a, layout);
    world.on(2);
    let c = alloc(world, &mut live);
    world.on(1);
    live.remove(&(b.as_ptr() as usize));
    world.free(b, layout);
    world.on(2);
    live.remove(&(c.as_ptr() as usize));
    world.free(c, layout);
    world.on(0);
    for object in &early[..2] {
        live.remove(&(object.as_ptr() as usize));
        world.free(*object, layout);
    }
    assert!(cpu2.acquisitions() > 0, "CPU 2 did not use its published slot");
    world.assert_conserved(&live);

    // CPUs past 8 have slots; a CPU past the firmware's count stays shared.
    assert!(world.register(11));
    assert!(!world.register(12), "a CPU past the table's end was given a slot");
    world.on(12);
    let shared = alloc(world, &mut live);
    world.assert_conserved(&live);
    world.free(shared, layout);
    live.remove(&(shared.as_ptr() as usize));
    world.assert_conserved(&live);
}

#[test]
fn drained_cpus_and_a_trimmed_depot_leave_one_empty_slab_per_cache() {
    let world = World::registered(4);
    let layout = bytes(64);
    let class = class_of(64, 8).unwrap();
    for cpu in 0..4 {
        world.on(cpu);
        let objects: Vec<_> = (0..100).map(|_| world.alloc(layout)).collect();
        world.on((cpu + 1) % 4);
        for object in objects {
            world.free(object, layout);
        }
    }
    assert!(world.kmem.class_stats(class).slabs.slabs > 1);
    for cpu in 0..4 {
        assert!(world.kmem.drain(cpu, true));
    }
    world.kmem.trim_depot(class);
    let stats = world.kmem.class_stats(class);
    assert_eq!((stats.cpu_rounds, stats.depot_rounds, stats.slabs.objects), (0, 0, 0));
    assert_eq!((stats.slabs.slabs, stats.slabs.empty), (1, 1), "a cache kept other than one empty slab");
    world.kmem.shrink(usize::MAX);
    assert_eq!(world.kmem.class_stats(class).slabs.slabs, 0, "the shrinker did not return the kept slab");
}

/// The page layer is empty, and the only free objects of the 4096-byte class
/// are in CPU 1's magazines, which fill whole slabs. A request on CPU 0 that
/// needs a page succeeds once the slow path drains CPU 1 (step 2) and the
/// slab shrinker returns the emptied slabs (step 3).
#[test]
fn a_page_request_under_pressure_is_met_from_another_cpus_magazines() {
    for remote_drains in [true, false] {
        let world = World::registered(2);
        world.remote_drains.set(remote_drains);
        let page_object = bytes(4096);
        world.on(1);
        let objects: Vec<_> = (0..15).map(|_| world.alloc(page_object)).collect();
        for object in objects {
            world.free(object, page_object);
        }
        let stats = world.kmem.class_stats(class_of(4096, 8).unwrap());
        // The first object, then a 15-round fill: 16 page-sized objects.
        assert_eq!((stats.cpu_rounds, stats.slabs.slabs), (16, 16));
        world.kmem.backing().limit.set(world.pages_in_use());
        world.on(0);
        let result = world.try_alloc(bytes(64));
        if remote_drains {
            assert!(result.is_ok(), "the slow path did not reach CPU 1's magazines");
        } else {
            assert_eq!(result, Err(AllocError::NoFrames), "a slow path that drains only its own CPU must fail here");
        }
    }
}

#[test]
fn each_object_meets_its_alignment_and_stays_inside_its_page() {
    let world = World::registered(1);
    world.on(0);
    for size in [1, 8, 16, 24, 100, 160, 161, 500, 1000, 1500, 3000, 4096] {
        for align in (0..=12).map(|shift| 1usize << shift) {
            let Some(class) = class_of(size, align) else { continue };
            let layout = Layout::from_size_align(size, align).unwrap();
            let objects: Vec<_> = (0..3).map(|_| world.alloc(layout)).collect();
            for object in &objects {
                let address = object.as_ptr() as usize;
                assert_eq!(address % align, 0, "{size} bytes aligned to {align}: {address:#x}");
                assert!(CLASSES[class] >= size);
                assert!(address % PAGE + CLASSES[class] <= PAGE, "an object crosses its slab's page");
            }
            for object in objects {
                world.free(object, layout);
            }
        }
    }
}

#[test]
fn each_class_matches_the_plans_page_arithmetic() {
    for &class in &CLASSES {
        assert_eq!(class % 16, 0, "{class} is not a multiple of 16");
        let per_page = PAGE / class;
        let tail = PAGE - per_page * class;
        assert!(tail <= 96, "{class} leaves {tail} bytes of a page unused");
        if class.is_power_of_two() {
            assert_eq!(tail, 0);
        }
        if class > 128 {
            assert_eq!(class, (PAGE / per_page) / 16 * 16, "{class} is not the largest multiple of 16 for {per_page} a page");
        }
    }
    assert_eq!(CLASSES[..10], [16, 32, 48, 64, 80, 96, 112, 128, 144, 160]);
}

/// ARM64 leaves TPIDR_EL1's reset value UNKNOWN, and firmware runs before the
/// kernel, so the register may hold any value until `per_cpu_aarch64::init_cpu`
/// sets it. Only the address of an entry of the per-CPU array names a CPU;
/// any other value must name none, so the slab layer never reads through it.
#[test]
fn only_a_per_cpu_array_entry_names_a_cpu() {
    let (first, stride, count) = (0xffff_0000_4010_0000u64, 0x140u64, 8usize);
    assert_eq!(cpu_of_base(0, first, stride, count), None, "0 before per-CPU init");
    for leftover in [0x8000_0000u64, 0xdead_beef_0000, first - stride, first + 1, first + 2 * stride + 8] {
        assert_eq!(cpu_of_base(leftover, first, stride, count), None, "{leftover:#x} is not an entry");
    }
    assert_eq!(cpu_of_base(first + 8 * stride, first, stride, count), None, "past the array's end");
    for cpu in 0..count {
        assert_eq!(cpu_of_base(first + cpu as u64 * stride, first, stride, count), Some(cpu));
    }
}

/// A cold CPU's first request when the page layer has 2 pages left: one for
/// the 64-byte class's slab and one for the descriptor cache's slab, which
/// holds that slab's descriptor. That meets the request. A magazine for the
/// CPU needs a third page, so the request must not depend on one.
#[test]
fn a_cold_refill_with_two_pages_left_meets_the_request_before_building_a_magazine() {
    let world = World::new(1, 2);
    assert!(world.register(0));
    world.on(0);
    let layout = bytes(64);
    let object = world.try_alloc(layout).expect("2 pages meet a cold 64-byte request");
    world.free(object, layout);
}

/// 6.6: an interrupt-context miss reaches at most one `try_lock` per layer
/// and then the emergency reserve. A cold CPU's request needs a page while
/// another CPU holds the frame allocator's free-list lock, so the slab layer
/// must report the lock busy, and the reserve meets the request.
#[test]
fn an_interrupt_request_does_not_wait_for_the_frame_lock() {
    let world = World::registered(2);
    world.on(0);
    world.kmem.backing().frame_lock_held.set(true);
    let before = world.kmem.shared_waiting();
    let layout = bytes(64);
    let object = world.try_alloc_in_interrupt(layout).expect("the emergency reserve meets it");
    assert_eq!(world.kmem.backing().frame_waits.get(), 0, "the request waited for the frame lock");
    assert_eq!(world.kmem.shared_waiting() - before, 0, "the request waited for a slab-layer lock");
    assert!(world.reserve.contains(object), "the reserve did not meet the request");
    world.free(object, layout);
}

/// The first refill after a 15-second interval detaches the depot's surplus
/// magazines (6.3). Returning their rounds takes the class's slab lock and
/// the magazine cache's lock, so a refill that may not wait must leave that
/// to a context that may.
#[test]
fn an_interrupt_refill_at_the_interval_waits_for_no_slab_lock() {
    let world = World::registered(3);
    let layout = bytes(64);
    world.on(1);
    let objects: Vec<_> = (0..64).map(|_| world.alloc(layout)).collect();
    for object in objects {
        world.free(object, layout);
    }
    // CPU 1's frees left 4 full magazines in the depot. CPU 0's refill, the
    // first exchange after 15 s, starts an interval and takes 1 of them.
    assert_eq!(world.kmem.class_stats(class_of(64, 8).unwrap()).depot_rounds, 4 * 15);
    world.kmem.backing().now.set(UPDATE_MS);
    world.on(0);
    let held = world.alloc(layout);
    // 15 s more with no exchange: the depot's other 3 are surplus.
    world.kmem.backing().now.set(2 * UPDATE_MS);
    world.on(2);
    let before = world.kmem.shared_waiting();
    let object = world.try_alloc_in_interrupt(layout).expect("the slabs meet it");
    assert_eq!(world.kmem.shared_waiting() - before, 0, "the refill waited for a lock to return the surplus");
    world.free(object, layout);
    world.on(0);
    world.free(held, layout);
}
