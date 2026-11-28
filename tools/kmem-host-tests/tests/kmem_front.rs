//! E1-4: allocation failure, contexts and reclaim, over the kernel's heap_index.
//!
//! One simulated CPU drives the front. Other CPUs exist only as holders of a
//! lock under a fixed schedule (`sim::SimLock`), so the interleavings are
//! fixed, and the tests use neither threads nor time.
use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

#[allow(dead_code)]
#[path = "../../../kernel/src/memory/heap_index.rs"]
mod heap_index;
#[path = "kmem_front/old_front.rs"]
mod old_front;
#[path = "kmem_front/sim.rs"]
mod sim;

use heap_index::Heap;
use sim::SimLock;

const ARENA_BYTES: usize = 64 * 1024;
const HELD_BLOCK: usize = 1024;

/// A heap and the cache that holds most of it.
struct World {
    _backing: Vec<u64>,
    main: SimLock<Heap>,
    /// Blocks the cache holds and its shrinker can give back.
    held: RefCell<Vec<NonNull<u8>>>,
    scans: Cell<u32>,
}

impl World {
    /// A heap filled to capacity by the cache's blocks.
    fn full() -> &'static World {
        let mut backing = vec![0u64; ARENA_BYTES / 8];
        let mut heap = Heap::empty();
        unsafe { heap.init(backing.as_mut_ptr().cast(), ARENA_BYTES) };
        let world: &'static World = Box::leak(Box::new(World {
            _backing: backing,
            main: SimLock::new(heap),
            held: RefCell::new(Vec::new()),
            scans: Cell::new(0),
        }));
        let block = Layout::from_size_align(HELD_BLOCK, 8).unwrap();
        while let (Some(ptr), _) = world.main.lock().allocate(block) {
            world.held.borrow_mut().push(ptr);
        }
        assert!(world.held.borrow().len() >= 8, "the cache holds enough to relieve a 4 KiB request");
        world
    }
}

/// One request as the CPU makes it.
struct Request {
    layout: Layout,
    in_interrupt: bool,
}

/// The front under test. 445c4c5e has no contexts and no shrinkers, so a
/// request is `GlobalAlloc::alloc`, which a request from an interrupt handler
/// reaches with interrupts masked.
fn allocate(world: &'static World, request: Request) -> *mut u8 {
    let _ = request.in_interrupt;
    old_front::alloc(&world.main, false, request.layout)
}

fn page() -> Layout {
    Layout::from_size_align(4096, 8).unwrap()
}

#[test]
fn a_full_heap_is_relieved_by_its_shrinker_after_one_scan() {
    let world = World::full();
    let ptr = allocate(world, Request { layout: page(), in_interrupt: false });
    assert!(!ptr.is_null(), "a full heap whose cache can give back enough bytes failed the request");
    assert_eq!(world.scans.get(), 1, "the request ran the shrinker exactly once");
}

#[test]
fn an_interrupt_request_never_waits_for_a_held_heap_lock() {
    let world = World::full();
    // Another CPU holds the heap lock and releases it only after three polls.
    world.main.hold_elsewhere(Some(3));
    let ptr = allocate(world, Request { layout: page(), in_interrupt: true });
    assert_eq!(world.main.waits(), 0, "an interrupt-context request waited on the heap lock");
    assert_eq!(world.main.try_locks(), 1, "an interrupt-context request makes one try_lock");
    assert_eq!(world.scans.get(), 0, "an interrupt-context request ran a shrinker");
    let _ = ptr;
}
