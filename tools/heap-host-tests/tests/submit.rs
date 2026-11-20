#[path = "../../../kernel/src/memory/heap_index.rs"]
mod indexed;

use linked_list_allocator::Heap as LegacyHeap;
use std::alloc::Layout;
use std::mem::size_of;
use std::ptr::NonNull;

const HEAP_BYTES: usize = 4 * 1024 * 1024;

trait TestHeap {
    fn allocate(&mut self, layout: Layout) -> (NonNull<u8>, usize);
    fn free(&mut self, ptr: NonNull<u8>, layout: Layout);
}

struct Legacy {
    heap: LegacyHeap,
    holes: Vec<NonNull<u8>>,
}

impl TestHeap for Legacy {
    fn allocate(&mut self, layout: Layout) -> (NonNull<u8>, usize) {
        let ptr = self.heap.allocate_first_fit(layout).unwrap();
        // During the submit the fixture's only holes before the tail are the
        // isolated, undersized extent holes. First-fit visits them in order.
        let skipped = self
            .holes
            .iter()
            .filter(|hole| hole.addr() < ptr.addr())
            .count();
        if layout.size() >= 64 && !self.holes.is_empty() {
            assert_eq!(skipped, self.holes.len());
        }
        (ptr, skipped + 1)
    }

    fn free(&mut self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { self.heap.deallocate(ptr, layout) };
        if layout.size() == 24 + 2 * size_of::<usize>() {
            self.holes.push(ptr);
        }
    }
}

impl TestHeap for indexed::Heap {
    fn allocate(&mut self, layout: Layout) -> (NonNull<u8>, usize) {
        let (ptr, visits) = indexed::Heap::allocate(self, layout);
        (ptr.unwrap(), visits as usize)
    }

    fn free(&mut self, ptr: NonNull<u8>, _layout: Layout) {
        unsafe { self.deallocate(ptr) };
    }
}

/// Model the fixed-quad packet pair (gk208/encode.rs::quads) while the
/// decoded leaf cache holds inode records between evicted extent records.
/// Each cohort also retains a verified 16 KiB node buffer.
fn submit_walk(heap: &mut impl TestHeap, leaf_cohorts: usize) -> (usize, usize) {
    let inode = Layout::from_size_align(128 + 2 * size_of::<usize>(), 8).unwrap();
    let extent = Layout::from_size_align(24 + 2 * size_of::<usize>(), 8).unwrap();
    let raw_node = Layout::from_size_align(16 * 1024, 8).unwrap();
    let words = Layout::array::<u32>(256).unwrap();
    let relocations = Layout::array::<usize>(8).unwrap();
    let mut old_extents = Vec::new();

    for _ in 0..leaf_cohorts {
        heap.allocate(raw_node);
        for _ in 0..16 {
            old_extents.push(heap.allocate(extent).0);
            heap.allocate(inode);
        }
    }
    for ptr in old_extents {
        heap.free(ptr, extent);
    }

    let mut max_words_walk = 0;
    let mut max_pair_walk = 0;
    for _ in 0..32 {
        let (packets, walk) = heap.allocate(words);
        let (relocations_ptr, relocation_walk) = heap.allocate(relocations);
        unsafe {
            packets.as_ptr().write_bytes(0x55, words.size());
            relocations_ptr
                .as_ptr()
                .write_bytes(0xaa, relocations.size());
        }
        heap.free(relocations_ptr, relocations);
        heap.free(packets, words);
        max_words_walk = max_words_walk.max(walk);
        max_pair_walk = max_pair_walk.max(walk + relocation_walk);
    }
    (max_words_walk, max_pair_walk)
}

#[test]
fn native_packet_lookup_is_independent_of_small_breenfs_holes() {
    fn legacy_walk(cohorts: usize) -> (usize, usize) {
        let mut backing = vec![0u64; HEAP_BYTES / 8];
        let heap = unsafe { LegacyHeap::new(backing.as_mut_ptr().cast(), HEAP_BYTES) };
        submit_walk(
            &mut Legacy {
                heap,
                holes: Vec::new(),
            },
            cohorts,
        )
    }
    fn indexed_walk(cohorts: usize) -> (usize, usize) {
        let mut backing = vec![0u64; HEAP_BYTES / 8];
        let mut heap = indexed::Heap::empty();
        unsafe { heap.init(backing.as_mut_ptr().cast(), HEAP_BYTES) };
        submit_walk(&mut heap, cohorts)
    }

    let old_single = legacy_walk(1);
    let old_full = legacy_walk(64);
    assert_eq!((old_single, old_full), ((17, 34), (1025, 2050)));
    let single = indexed_walk(1);
    let full = indexed_walk(64);
    eprintln!("packet words and pair visits: first-fit {old_single:?}/{old_full:?}, indexed {single:?}/{full:?}");
    assert_eq!(
        full, single,
        "fixed method-buffer lookup grew with the resident set"
    );
}

#[test]
fn mixed_layouts_preserve_live_bytes_and_coalesce() {
    let mut backing = vec![0u64; HEAP_BYTES / 8];
    let mut heap = indexed::Heap::empty();
    unsafe { heap.init(backing.as_mut_ptr().cast(), HEAP_BYTES) };
    let mut live = [None; 512];
    let mut seed = 723u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };

    for _ in 0..12_000 {
        let slot = next() % live.len();
        if let Some((ptr, len, value)) = live[slot].take() {
            check_and_free(&mut heap, ptr, len, value);
        }
        let len = 1 + next() % 8192;
        let layout = Layout::from_size_align(len, 1 << (next() % 13)).unwrap();
        if let (Some(ptr), _) = heap.allocate(layout) {
            assert_eq!(ptr.addr().get() % layout.align(), 0);
            let value = (1 + next() % 255) as u8;
            unsafe { ptr.as_ptr().write_bytes(value, len) };
            live[slot] = Some((ptr, len, value));
        }
    }
    for (ptr, len, value) in live.into_iter().flatten() {
        check_and_free(&mut heap, ptr, len, value);
    }
    assert_eq!(heap.used(), 0);
    assert_eq!(heap.free(), HEAP_BYTES);
    assert_eq!(heap.free_shape(), (1, HEAP_BYTES as u32));
}

fn check_and_free(heap: &mut indexed::Heap, ptr: NonNull<u8>, len: usize, value: u8) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), len) };
    assert!(
        bytes.iter().all(|&byte| byte == value),
        "live allocation was overwritten"
    );
    unsafe { heap.deallocate(ptr) };
}

#[test]
fn aligned_request_finds_the_only_usable_extent() {
    #[repr(align(64))]
    struct Backing([u8; 192]);
    let mut backing = Backing([0; 192]);
    let mut heap = indexed::Heap::empty();
    unsafe { heap.init(backing.0.as_mut_ptr(), 192) };
    let extent = Layout::from_size_align(40, 8).unwrap();
    let (first, _) = TestHeap::allocate(&mut heap, extent);
    let (separator, _) = TestHeap::allocate(&mut heap, Layout::from_size_align(64, 8).unwrap());
    let (second, _) = TestHeap::allocate(&mut heap, extent);
    let (last, _) = TestHeap::allocate(&mut heap, Layout::from_size_align(16, 8).unwrap());
    unsafe {
        heap.deallocate(first);
        heap.deallocate(second);
    }
    assert_eq!(heap.free_shape(), (2, 48));
    let (aligned, _) = TestHeap::allocate(&mut heap, Layout::from_size_align(40, 64).unwrap());
    assert_eq!(aligned, second);
    unsafe {
        heap.deallocate(aligned);
        heap.deallocate(separator);
        heap.deallocate(last);
    }
    assert_eq!(heap.free_shape(), (1, 192));
}
