use linked_list_allocator::Heap;
use std::alloc::Layout;
use std::mem::{size_of, size_of_val};
use std::ptr::NonNull;

const HEAP_BYTES: usize = 4 * 1024 * 1024;

struct TestHeap {
    heap: Heap,
    holes: Vec<NonNull<u8>>,
}

impl TestHeap {
    fn new(bytes: &mut [u64]) -> Self {
        let heap = unsafe { Heap::new(bytes.as_mut_ptr().cast(), size_of_val(bytes)) };
        Self { heap, holes: Vec::new() }
    }

    fn allocate(&mut self, layout: Layout) -> NonNull<u8> {
        self.heap.allocate_first_fit(layout).unwrap()
    }

    fn free(&mut self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { self.heap.deallocate(ptr, layout) };
    }

    fn free_payload(&mut self, ptr: NonNull<u8>, layout: Layout) {
        self.free(ptr, layout);
        self.holes.push(ptr);
    }

    fn submit_words(&mut self, layout: Layout) -> (NonNull<u8>, usize) {
        let ptr = self.allocate(layout);
        // The fixture's only holes are the isolated, undersized payload
        // holes and the tail. The old allocator visits them in address order.
        let skipped = self.holes.iter().filter(|hole| hole.addr() < ptr.addr()).count();
        assert_eq!(skipped, self.holes.len());
        (ptr, skipped + 1)
    }
}

/// Model the fixed-quad packet pair (gk208/encode.rs::quads) while the
/// decoded leaf cache holds inode records between evicted extent records.
/// Each cohort also retains a verified 16 KiB node buffer.
fn submit_walk(leaf_cohorts: usize) -> usize {
    let mut memory = vec![0u64; HEAP_BYTES / 8];
    let mut heap = TestHeap::new(&mut memory);
    let inode = Layout::from_size_align(128 + 2 * size_of::<usize>(), 8).unwrap();
    let extent = Layout::from_size_align(24 + 2 * size_of::<usize>(), 8).unwrap();
    let raw_node = Layout::from_size_align(16 * 1024, 8).unwrap();
    let words = Layout::array::<u32>(256).unwrap();
    let relocations = Layout::array::<usize>(8).unwrap();
    let mut old_extents = Vec::new();

    for _ in 0..leaf_cohorts {
        heap.allocate(raw_node);
        for _ in 0..16 {
            old_extents.push(heap.allocate(extent));
            heap.allocate(inode);
        }
    }
    for ptr in old_extents {
        heap.free_payload(ptr, extent);
    }

    let mut max_walk = 0;
    for _ in 0..32 {
        let (packets, walk) = heap.submit_words(words);
        let relocations_ptr = heap.allocate(relocations);
        unsafe {
            packets.as_ptr().write_bytes(0x55, words.size());
            relocations_ptr.as_ptr().write_bytes(0xaa, relocations.size());
        }
        heap.free(relocations_ptr, relocations);
        heap.free(packets, words);
        max_walk = max_walk.max(walk);
    }
    max_walk
}

#[test]
fn native_packet_lookup_is_independent_of_small_breenfs_holes() {
    let single = submit_walk(1);
    let full = submit_walk(64);
    assert_eq!(full, single, "fixed method-buffer lookup grew with the resident set");
}
