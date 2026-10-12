use super::align_up;
use crate::hole::HoleList;
use core::mem::align_of;

/// A fixed size heap backed by a linked list of free memory blocks.
pub struct Heap {
    bottom: usize,
    size: usize,
    holes: HoleList,
}

impl Heap {
    /// Creates an empty heap. All allocate calls will return `None`.
    pub const fn empty() -> Heap {
        Heap {
            bottom: 0,
            size: 0,
            holes: HoleList::empty(),
        }
    }

    /// Creates a new heap with the given `bottom` and `size`. The bottom address must be valid
    /// and the memory in the `[heap_bottom, heap_bottom + heap_size)` range must not be used for
    /// anything else. This function is unsafe because it can cause undefined behavior if the
    /// given address is invalid.
    pub unsafe fn new(heap_bottom: usize, heap_size: usize) -> Heap {
        Heap {
            bottom: heap_bottom,
            size: heap_size,
            holes: HoleList::new(heap_bottom, heap_size),
        }
    }

    /// Allocates a chunk of the given size with the given alignment. Returns a pointer to the
    /// beginning of that chunk if it was successful. Else it returns `None`.
    /// This function scans the list of free memory blocks and uses the first block that is big
    /// enough. The runtime is in O(n) where n is the number of free blocks, but it should be
    /// reasonably fast for small allocations.
    pub fn allocate_first_fit(&mut self, mut size: usize, align: usize) -> *mut u8 {
        if size < HoleList::min_size() {
            size = HoleList::min_size();
        }

        self.holes.allocate_first_fit(
            align_up(size, align_of::<usize>()),
            align.max(align_of::<usize>()),
        )
    }

    /// Frees the given allocation. `ptr` must be a pointer returned
    /// by a call to the `allocate_first_fit` function with identical size and alignment. Undefined
    /// behavior may occur for invalid arguments, thus this function is unsafe.
    ///
    /// This function walks the list of free memory blocks and inserts the freed block at the
    /// correct place. If the freed block is adjacent to another free block, the blocks are merged
    /// again. This operation is in `O(n)` since the list needs to be sorted by address.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8, mut size: usize, _align: usize) {
        if size < HoleList::min_size() {
            size = HoleList::min_size();
        }
        self.holes
            .deallocate(ptr, align_up(size, align_of::<usize>()));
    }

    /// Returns the bottom address of the heap.
    pub fn bottom(&self) -> usize {
        self.bottom
    }

    /// Returns the size of the heap.
    pub fn size(&self) -> usize {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_heap_returns_null() {
        let mut heap = Heap::empty();
        assert_eq!(heap.bottom(), 0);
        assert_eq!(heap.size(), 0);
        assert!(heap.allocate_first_fit(16, 8).is_null());
    }

    #[test]
    fn freeing_last_block_merges_with_previous() {
        let mut storage = [0usize; 16];
        let bottom = storage.as_mut_ptr() as usize;
        let size = core::mem::size_of_val(&storage);
        let mut heap = unsafe { Heap::new(bottom, size) };
        let first = heap.allocate_first_fit(size / 2, 8);
        let second = heap.allocate_first_fit(size / 2, 8);
        assert!(!first.is_null() && !second.is_null());
        unsafe {
            heap.deallocate(first, size / 2, 8);
            heap.deallocate(second, size / 2, 8);
        }
        assert_eq!(heap.holes.first_hole(), Some((bottom, size)));
        assert_eq!(heap.allocate_first_fit(size, 8) as usize, bottom);
    }

    #[test]
    fn aligned_allocations_reclaim_and_coalesce() {
        // Back the heap with real aligned host memory, not the kernel's fixed address.
        let mut storage = [0usize; 128];
        let bottom = storage.as_mut_ptr() as usize;
        let size = core::mem::size_of_val(&storage);
        let mut heap = unsafe { Heap::new(bottom, size) };
        assert_eq!(heap.bottom(), bottom);
        assert_eq!(heap.size(), size);
        let first = heap.allocate_first_fit(17, 8);
        let second = heap.allocate_first_fit(33, 8);
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert_eq!(first as usize % 8, 0);
        assert_eq!(second as usize % 8, 0);
        unsafe {
            first.write_bytes(0xA5, 17);
            second.write_bytes(0x5A, 33);
            heap.deallocate(second, 33, 8);
            heap.deallocate(first, 17, 8);
        }
        assert_eq!(heap.holes.first_hole(), Some((bottom, size)));
        let whole = heap.allocate_first_fit(size, 8);
        assert_eq!(whole as usize, bottom);
        assert!(heap.allocate_first_fit(16, 8).is_null());
    }
}
