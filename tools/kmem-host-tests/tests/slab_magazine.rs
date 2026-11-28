//! E1-6: small requests through the allocator's front on several CPUs.
//!
//! CPUs are simulated as explicit parameters under a fixed schedule: no
//! threads and no timing.
use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

#[allow(dead_code)]
#[path = "../../../kernel/src/memory/heap_index.rs"]
mod heap_index;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kmem_core.rs"]
mod kmem_core;

use heap_index::Heap;
use kmem_core::{Cpu, Ctx, Front, Miss, Pool, Reclaim, Registry, Stats};

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

/// The CPU the schedule is running on.
struct SimCpu<'a> {
    current: &'a Cell<usize>,
}

impl Cpu for SimCpu<'_> {
    fn in_interrupt(&self) -> bool {
        false
    }

    fn id(&self) -> usize {
        self.current.get()
    }

    fn masked<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

struct World {
    main: Arena,
    reserve: Arena,
    shrinkers: Registry,
    reclaim: Reclaim,
    stats: Stats,
    cpu: Cell<usize>,
}

impl World {
    fn new() -> Box<World> {
        Box::new(World {
            main: Arena::new(ARENA_BYTES),
            reserve: Arena::new(ARENA_BYTES / 16),
            shrinkers: Registry::new(),
            reclaim: Reclaim::new(),
            stats: Stats::new(),
            cpu: Cell::new(0),
        })
    }

    fn front(&self) -> Front<'_, Arena, Arena> {
        Front {
            main: &self.main,
            reserve: &self.reserve,
            shrinkers: &self.shrinkers,
            reclaim: &self.reclaim,
            stats: &self.stats,
        }
    }

    fn on(&self, cpu: usize) {
        self.cpu.set(cpu);
    }

    fn alloc(&self, layout: Layout) -> NonNull<u8> {
        self.front().allocate(layout, Ctx::Normal, &SimCpu { current: &self.cpu }).expect("allocation")
    }

    fn free(&self, ptr: NonNull<u8>, _layout: Layout) {
        unsafe { self.front().deallocate(ptr) }
    }

    /// Acquisitions of the locks every CPU shares.
    fn shared_acquisitions(&self) -> u64 {
        self.main.acquisitions.get() + self.reserve.acquisitions.get()
    }
}

/// Check first (E1-6): 4 CPUs allocate and free 64-byte objects under a fixed
/// schedule. Once each CPU has run once, its operations must take no lock
/// another CPU takes. Today's front takes the heap's global lock on each one.
#[test]
fn small_objects_on_four_cpus_take_no_shared_lock_on_the_fast_path() {
    let world = World::new();
    let layout = Layout::from_size_align(64, 8).unwrap();
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
