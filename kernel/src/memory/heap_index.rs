//! Size-indexed extents over one fixed region. The index lives in free blocks.
//! For ordinary alignment, allocation looks at at most two class heads. Each
//! class is an address radix tree with its largest extent at the root. Its
//! depth is bounded by the offset width, including classes of equal-size holes.
//! Boundary tags let free find both physical neighbors directly.

use core::alloc::Layout;
use core::ptr::NonNull;

const NONE: u32 = u32::MAX;
const ALIGN: u32 = 8;
const HEADER: u32 = 8;
const MIN_BLOCK: u32 = 16;
const FREE: u32 = 1;
const PADDING: u32 = 2;
const CLASSES: usize = 128;

#[derive(Clone, Copy)]
#[repr(C)]
struct Header {
    size_flags: u32,
    previous_size: u32,
}

impl Header {
    fn size(self) -> u32 {
        self.size_flags & !(ALIGN - 1)
    }

    fn is_free(self) -> bool {
        self.size_flags & FREE != 0
    }
}

/// Offsets are relative to an exclusive, aligned region smaller than 4 GiB.
/// An allocated block has a header and user data. A free block uses the first
/// two words of that data for its child offsets.
pub(super) struct Heap {
    base: *mut u8,
    size: u32,
    used: u32,
    nodes: u32,
    nonempty: u128,
    roots: [u32; CLASSES],
}

// SAFETY: The caller serializes access. A move preserves the backing region
// and links are relative to that region.
unsafe impl Send for Heap {}

impl Heap {
    pub(super) const fn empty() -> Self {
        Self {
            base: core::ptr::null_mut(),
            size: 0,
            used: 0,
            nodes: 0,
            nonempty: 0,
            roots: [NONE; CLASSES],
        }
    }

    /// Safety: `base` is 8-byte aligned and the region is valid for exclusive
    /// use by this heap for the lifetime of its allocations.
    pub(super) unsafe fn init(&mut self, base: *mut u8, size: usize) {
        assert!(self.size == 0 && base.addr().is_multiple_of(ALIGN as usize));
        let size = u32::try_from(size).expect("kernel heap offsets must fit in u32") & !(ALIGN - 1);
        assert!(size >= MIN_BLOCK);
        self.base = base;
        self.size = size;
        self.write_header(0, size | FREE, 0);
        self.insert(0, size);
    }

    pub(super) fn used(&self) -> usize {
        self.used as usize
    }

    pub(super) fn free(&self) -> usize {
        (self.size - self.used) as usize
    }

    /// Count of free extents and exact size of the largest.
    pub(super) fn free_shape(&self) -> (u32, u32) {
        let largest = if self.nonempty == 0 {
            0
        } else {
            let class = CLASSES - 1 - self.nonempty.leading_zeros() as usize;
            self.header(self.roots[class]).size()
        };
        (self.nodes, largest)
    }

    pub(super) fn allocate(&mut self, layout: Layout) -> (Option<NonNull<u8>>, u32) {
        let mut visits = 0;
        let ptr = self.allocate_inner(layout, &mut visits);
        (ptr, visits)
    }

    fn allocate_inner(&mut self, layout: Layout, visits: &mut u32) -> Option<NonNull<u8>> {
        let size = layout.size().checked_next_multiple_of(ALIGN as usize)?;
        let size = u32::try_from(size)
            .ok()?
            .checked_add(HEADER)?
            .max(MIN_BLOCK);
        let align = layout.align().max(ALIGN as usize);
        let first = self.candidate(size, visits)?;
        let (block, pad) = if let Some(pad) = self.padding(first, size, align) {
            (first, pad)
        } else {
            // An unusually aligned request may need a larger extent. Try one
            // that fits at any address before searching smaller candidates
            // near exhaustion, where rejecting a fitting extent would be OOM.
            let guaranteed = u32::try_from(align - ALIGN as usize)
                .ok()
                .and_then(|pad| size.checked_add(pad))
                .and_then(|need| self.candidate(need, visits));
            if let Some(block) = guaranteed {
                (
                    block,
                    self.padding(block, size, align)
                        .expect("guaranteed alignment"),
                )
            } else {
                self.find_aligned(size, align, visits)?
            }
        };

        let old = self.header(block);
        *visits += self.remove(block, old.size());
        let ptr_offset = block + pad + HEADER;
        // Leave a reusable prefix when possible. One alignment word is too
        // small to index, so charge it and flag it just before the user pointer.
        let prefix = if pad >= MIN_BLOCK { pad } else { 0 };
        let start = block + prefix;
        let previous = if prefix == 0 {
            old.previous_size
        } else {
            self.write_header(block, prefix | FREE, old.previous_size);
            *visits += self.insert(block, prefix);
            prefix
        };
        let end = block + old.size();
        let mut used = size + pad - prefix;
        if end - (start + used) < MIN_BLOCK {
            used = end - start;
        }
        self.write_header(start, used, previous);
        if pad != prefix {
            self.write_header(start + HEADER, PADDING, 0);
        }
        let tail = end - (start + used);
        if tail != 0 {
            let rest = start + used;
            self.write_header(rest, tail | FREE, used);
            *visits += self.insert(rest, tail);
        }
        self.set_previous(end, if tail == 0 { used } else { tail });
        self.used += used;

        // SAFETY: Placement leaves `layout.size()` bytes aligned inside the
        // region, and the extent is no longer indexed as free.
        Some(unsafe { NonNull::new_unchecked(self.base.add(ptr_offset as usize)) })
    }

    /// Safety: `ptr` is a still-live allocation from this heap.
    pub(super) unsafe fn deallocate(&mut self, ptr: NonNull<u8>) {
        let mut start = (ptr.addr().get() - self.base.addr()) as u32 - HEADER;
        if self.header(start).size_flags == PADDING {
            start -= ALIGN;
        }
        let allocation = self.header(start);
        let mut size = allocation.size();
        let mut previous_size = allocation.previous_size;
        self.used -= size;

        let next = start + size;
        if next < self.size {
            let header = self.header(next);
            if header.is_free() {
                self.remove(next, header.size());
                size += header.size();
            }
        }
        if previous_size != 0 {
            let previous = start - previous_size;
            let header = self.header(previous);
            if header.is_free() {
                self.remove(previous, previous_size);
                start = previous;
                size += previous_size;
                previous_size = header.previous_size;
            }
        }
        self.write_header(start, size | FREE, previous_size);
        self.set_previous(start + size, size);
        self.insert(start, size);
    }

    fn class(size: u32) -> usize {
        let units = size / ALIGN;
        if units < 16 {
            units as usize
        } else {
            let power = 31 - units.leading_zeros();
            // Four subdivisions per doubling, ordered by exact size within
            // each class at the root. Allocations are carved at block alignment.
            (16 + 4 * (power - 4) + ((units >> (power - 2)) & 3)) as usize
        }
    }

    fn candidate(&self, size: u32, visits: &mut u32) -> Option<u32> {
        let mut classes = self.nonempty & (u128::MAX << Self::class(size));
        if classes == 0 {
            return None;
        }
        let block = self.roots[classes.trailing_zeros() as usize];
        *visits += 1;
        if self.header(block).size() >= size {
            return Some(block);
        }
        // The next class has a strictly larger minimum size.
        classes &= classes - 1;
        if classes == 0 {
            return None;
        }
        *visits += 1;
        Some(self.roots[classes.trailing_zeros() as usize])
    }

    fn padding(&self, offset: u32, size: u32, align: usize) -> Option<u32> {
        let space = self.header(offset).size().checked_sub(size)?;
        let addr = self.base.addr() + offset as usize + HEADER as usize;
        let pad = addr.wrapping_neg() & (align - 1);
        (pad <= space as usize).then_some(pad as u32)
    }

    fn find_aligned(&self, size: u32, align: usize, visits: &mut u32) -> Option<(u32, u32)> {
        let mut classes = self.nonempty & (u128::MAX << Self::class(size));
        while classes != 0 {
            let root = self.roots[classes.trailing_zeros() as usize];
            if let Some(found) = self.find_aligned_in(root, size, align, visits) {
                return Some(found);
            }
            classes &= classes - 1;
        }
        None
    }

    fn find_aligned_in(
        &self,
        node: u32,
        size: u32,
        align: usize,
        visits: &mut u32,
    ) -> Option<(u32, u32)> {
        if node == NONE {
            return None;
        }
        *visits += 1;
        // Descendants are no larger than this node.
        if self.header(node).size() < size {
            return None;
        }
        if let Some(pad) = self.padding(node, size, align) {
            return Some((node, pad));
        }
        let [left, right] = self.links(node);
        self.find_aligned_in(left, size, align, visits)
            .or_else(|| self.find_aligned_in(right, size, align, visits))
    }

    fn branch(offset: u32, depth: u32) -> usize {
        ((offset / ALIGN >> depth) & 1) as usize
    }

    fn insert(&mut self, mut block: u32, size: u32) -> u32 {
        let class = Self::class(size);
        self.nonempty |= 1 << class;
        self.nodes += 1;
        let mut node = self.roots[class];
        let (mut parent, mut side, mut depth) = (NONE, 0, 0);
        while node != NONE {
            let children = self.links(node);
            if self.header(block).size() > self.header(node).size() {
                // Put the largest block here. The displaced node shares this
                // subtree's address prefix and is reinserted one bit deeper.
                self.set_link(class, parent, side, block);
                self.write_links(block, children);
                core::mem::swap(&mut block, &mut node);
            }
            parent = node;
            side = Self::branch(block, depth);
            depth += 1;
            node = children[side];
        }
        self.write_links(block, [NONE; 2]);
        self.set_link(class, parent, side, block);
        depth
    }

    fn remove(&mut self, block: u32, size: u32) -> u32 {
        let class = Self::class(size);
        let (mut parent, mut side, mut depth) = (NONE, 0, 0);
        let mut node = self.roots[class];
        let mut visits = 0;
        while node != block {
            visits += 1;
            parent = node;
            side = Self::branch(block, depth);
            depth += 1;
            node = self.links(node)[side];
        }

        // Fill the vacancy with the larger child, repeating down that child's
        // old path. The sibling stays under its original address-prefix bit.
        let mut children = self.links(node);
        loop {
            let [left, right] = children;
            visits += u32::from(left != NONE) + u32::from(right != NONE);
            let larger = usize::from(
                right != NONE
                    && (left == NONE || self.header(right).size() > self.header(left).size()),
            );
            let promote = children[larger];
            self.set_link(class, parent, side, promote);
            if promote == NONE {
                break;
            }
            parent = promote;
            side = larger;
            let descendants = self.links(promote);
            children[larger] = NONE;
            self.write_links(promote, children);
            children = descendants;
        }
        self.nodes -= 1;
        if self.roots[class] == NONE {
            self.nonempty &= !(1 << class);
        }
        visits
    }

    fn set_link(&mut self, class: usize, parent: u32, side: usize, child: u32) {
        if parent == NONE {
            self.roots[class] = child;
        } else {
            self.write_word(parent + HEADER + side as u32 * 4, child);
        }
    }

    fn set_previous(&mut self, offset: u32, previous_size: u32) {
        if offset < self.size {
            self.write_word(offset + 4, previous_size);
        }
    }

    fn header(&self, offset: u32) -> Header {
        // SAFETY: Only indexed or adjacent initialized headers are read.
        unsafe { self.base.add(offset as usize).cast::<Header>().read() }
    }

    fn write_header(&mut self, offset: u32, size_flags: u32, previous_size: u32) {
        // SAFETY: The block starts on an aligned offset in this region.
        unsafe {
            self.base
                .add(offset as usize)
                .cast::<Header>()
                .write(Header {
                    size_flags,
                    previous_size,
                });
        }
    }

    fn links(&self, offset: u32) -> [u32; 2] {
        // SAFETY: Only indexed blocks are read. They contain two initialized links.
        unsafe {
            self.base
                .add((offset + HEADER) as usize)
                .cast::<[u32; 2]>()
                .read()
        }
    }

    fn write_links(&mut self, offset: u32, links: [u32; 2]) {
        // SAFETY: Only free blocks, which have at least MIN_BLOCK bytes, hold links.
        unsafe {
            self.base
                .add((offset + HEADER) as usize)
                .cast::<[u32; 2]>()
                .write(links)
        }
    }

    fn write_word(&mut self, offset: u32, value: u32) {
        // SAFETY: Callers write only aligned words in their metadata.
        unsafe { self.base.add(offset as usize).cast::<u32>().write(value) }
    }
}
