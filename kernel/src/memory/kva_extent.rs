//! The free extents of one kernel VA window, with no kernel dependencies, so
//! `tools/kmem-host-tests` compiles this file as is (E1 plan, 6.5).
//!
//! Free extents sit in two trees at once: one ordered by start, which finds
//! the neighbours a freed extent coalesces with, and one ordered by
//! (length, start), which finds the smallest extent that fits. Both are
//! treaps (Seidel and Aragon, "Randomized Search Trees", Algorithmica 16,
//! 1996), whose split and merge keep each operation O(log n) expected with
//! no rebalancing rules.
//!
//! Nodes come from pages the caller adds (`add_nodes`), not from the
//! window itself, so the allocator does not recurse into the memory it
//! manages. An operation needs at most one spare node and says so with
//! `NoNode`, before it changes anything.

use core::ptr::null_mut;

/// A range of a window: `[start, start + len)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub start: u64,
    pub len: u64,
}

impl Extent {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// An allocation: `extent` is what the window gave out, guard pages
/// included, and `[start, start + len)` is the part the caller maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub extent: Extent,
    pub start: u64,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtentError {
    /// No free extent fits the request.
    NoVa,
    /// The operation needs a node and has no spare one; add a node page.
    NoNode,
    /// The request is empty, unaligned, or its alignment is not a power of two.
    BadRequest,
    /// The extent overlaps a free one: a double free or a foreign extent.
    Overlap,
}

const BY_START: usize = 0;
const BY_SIZE: usize = 1;
const LEFT: usize = 0;
const RIGHT: usize = 1;

/// One extent's node: in both trees while free, in the retired list (linked
/// through `link[BY_START][LEFT]`) while awaiting a flush, and in the spare
/// list (linked the same way) while unused.
#[repr(C)]
pub struct Node {
    start: u64,
    len: u64,
    priority: u64,
    link: [[*mut Node; 2]; 2],
}

/// The bytes of one node, for sizing node pages.
pub const NODE_BYTES: usize = core::mem::size_of::<Node>();

/// A detached list of retired extents, from `take_retired`.
pub struct Retired {
    head: *mut Node,
}

impl Retired {
    /// The retired extents, in no particular order.
    pub fn extents(&self) -> impl Iterator<Item = Extent> + '_ {
        let mut node = self.head;
        core::iter::from_fn(move || {
            if node.is_null() {
                return None;
            }
            // SAFETY: the list's nodes belong to the allocator that detached
            // it and are not touched until `release` takes the list back.
            let extent = unsafe { Extent { start: (*node).start, len: (*node).len } };
            node = unsafe { (*node).link[BY_START][LEFT] };
            Some(extent)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }
}

/// The free extents of one window, in `granule` units.
pub struct Extents {
    granule: u64,
    roots: [*mut Node; 2],
    spare: *mut Node,
    spares: usize,
    retired: *mut Node,
    retired_bytes: u64,
    free_bytes: u64,
    seed: u64,
}

// SAFETY: the nodes are reached only through the `Extents` that owns them,
// and the kernel serializes each `Extents` under its window's lock.
unsafe impl Send for Extents {}

type Key = (u64, u64);

unsafe fn key(node: *mut Node, tree: usize) -> Key {
    if tree == BY_START {
        ((*node).start, 0)
    } else {
        ((*node).len, (*node).start)
    }
}

/// Split `root` into the nodes with keys below `at` and the rest.
unsafe fn split(root: *mut Node, tree: usize, at: Key) -> (*mut Node, *mut Node) {
    if root.is_null() {
        return (null_mut(), null_mut());
    }
    if key(root, tree) < at {
        let (low, high) = split((*root).link[tree][RIGHT], tree, at);
        (*root).link[tree][RIGHT] = low;
        (root, high)
    } else {
        let (low, high) = split((*root).link[tree][LEFT], tree, at);
        (*root).link[tree][LEFT] = high;
        (low, root)
    }
}

/// Join two trees where each key of `low` is below each key of `high`.
unsafe fn merge(low: *mut Node, high: *mut Node, tree: usize) -> *mut Node {
    if low.is_null() {
        return high;
    }
    if high.is_null() {
        return low;
    }
    if (*low).priority > (*high).priority {
        (*low).link[tree][RIGHT] = merge((*low).link[tree][RIGHT], high, tree);
        low
    } else {
        (*high).link[tree][LEFT] = merge(low, (*high).link[tree][LEFT], tree);
        high
    }
}

impl Extents {
    /// An empty window whose extents are multiples of `granule` bytes, a
    /// power of two of at least 4 KiB.
    pub const fn new(granule: u64) -> Self {
        Self {
            granule,
            roots: [null_mut(); 2],
            spare: null_mut(),
            spares: 0,
            retired: null_mut(),
            retired_bytes: 0,
            free_bytes: 0,
            seed: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// Carve `bytes` at `memory` into spare nodes.
    ///
    /// # Safety
    /// The memory is 8-byte aligned, writable, unused by anything else, and
    /// stays so for the allocator's lifetime.
    pub unsafe fn add_nodes(&mut self, memory: *mut u8, bytes: usize) {
        for slot in 0..bytes / NODE_BYTES {
            let node = memory.add(slot * NODE_BYTES) as *mut Node;
            node.write(Node { start: 0, len: 0, priority: 0, link: [[null_mut(); 2]; 2] });
            self.put_spare(node);
        }
    }

    pub fn spare_nodes(&self) -> usize {
        self.spares
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_bytes
    }

    pub fn retired_bytes(&self) -> u64 {
        self.retired_bytes
    }

    fn put_spare(&mut self, node: *mut Node) {
        // SAFETY: `node` is one of this allocator's nodes and in no list.
        unsafe { (*node).link[BY_START][LEFT] = self.spare };
        self.spare = node;
        self.spares += 1;
    }

    fn take_spare(&mut self) -> Option<*mut Node> {
        if self.spare.is_null() {
            return None;
        }
        let node = self.spare;
        // SAFETY: as in `put_spare`.
        unsafe {
            self.spare = (*node).link[BY_START][LEFT];
            (*node).link = [[null_mut(); 2]; 2];
        }
        self.spares -= 1;
        // xorshift64 (Marsaglia 2003): a fixed sequence, so runs repeat.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        unsafe { (*node).priority = self.seed };
        Some(node)
    }

    unsafe fn insert(&mut self, node: *mut Node, tree: usize) {
        (*node).link[tree] = [null_mut(); 2];
        let (low, high) = split(self.roots[tree], tree, key(node, tree));
        self.roots[tree] = merge(merge(low, node, tree), high, tree);
    }

    unsafe fn remove(&mut self, node: *mut Node, tree: usize) {
        let at = key(node, tree);
        let (low, rest) = split(self.roots[tree], tree, at);
        let (found, high) = split(rest, tree, (at.0, at.1 + 1));
        debug_assert!(found == node);
        self.roots[tree] = merge(low, high, tree);
    }

    /// The free extent with the largest start below `start`, and the one
    /// with the smallest start at or above it.
    fn neighbours(&self, start: u64) -> (*mut Node, *mut Node) {
        let (mut below, mut above) = (null_mut(), null_mut());
        let mut node = self.roots[BY_START];
        // SAFETY: the tree's nodes are this allocator's.
        unsafe {
            while !node.is_null() {
                if (*node).start < start {
                    below = node;
                    node = (*node).link[BY_START][RIGHT];
                } else {
                    above = node;
                    node = (*node).link[BY_START][LEFT];
                }
            }
        }
        (below, above)
    }

    /// The smallest free extent of at least `len` bytes.
    fn smallest_fitting(&self, len: u64) -> *mut Node {
        let mut best = null_mut();
        let mut node = self.roots[BY_SIZE];
        // SAFETY: the tree's nodes are this allocator's.
        unsafe {
            while !node.is_null() {
                if (*node).len >= len {
                    best = node;
                    node = (*node).link[BY_SIZE][LEFT];
                } else {
                    node = (*node).link[BY_SIZE][RIGHT];
                }
            }
        }
        best
    }

    fn check(&self, extent: Extent) -> Result<(), ExtentError> {
        let mask = self.granule - 1;
        if extent.len == 0 || extent.start & mask != 0 || extent.len & mask != 0 {
            return Err(ExtentError::BadRequest);
        }
        extent.start.checked_add(extent.len).map(|_| ()).ok_or(ExtentError::BadRequest)
    }

    /// Return `extent` to the free trees, coalescing it with free neighbours.
    pub fn free(&mut self, extent: Extent) -> Result<(), ExtentError> {
        self.check(extent)?;
        let (below, above) = self.neighbours(extent.start);
        // SAFETY: the neighbours are this allocator's nodes.
        let (joins_below, joins_above) = unsafe {
            if !below.is_null() && (*below).start + (*below).len > extent.start {
                return Err(ExtentError::Overlap);
            }
            if !above.is_null() && (*above).start < extent.end() {
                return Err(ExtentError::Overlap);
            }
            (
                !below.is_null() && (*below).start + (*below).len == extent.start,
                !above.is_null() && (*above).start == extent.end(),
            )
        };
        let node = if joins_below || joins_above {
            null_mut()
        } else {
            self.take_spare().ok_or(ExtentError::NoNode)?
        };
        // SAFETY: each node named here is this allocator's.
        unsafe { self.link_free(extent, node, below, above, joins_below, joins_above) };
        self.free_bytes += extent.len;
        Ok(())
    }

    /// Link `extent` into the trees, joined to the neighbours it touches, or
    /// as `node` when it touches neither. A node a join leaves unused goes
    /// back to the spare list.
    unsafe fn link_free(
        &mut self,
        extent: Extent,
        node: *mut Node,
        below: *mut Node,
        above: *mut Node,
        joins_below: bool,
        joins_above: bool,
    ) {
        match (joins_below, joins_above) {
            (true, true) => {
                self.remove(below, BY_SIZE);
                self.remove(above, BY_SIZE);
                self.remove(above, BY_START);
                (*below).len += extent.len + (*above).len;
                self.insert(below, BY_SIZE);
                self.put_spare(above);
            }
            (true, false) => {
                self.remove(below, BY_SIZE);
                (*below).len += extent.len;
                self.insert(below, BY_SIZE);
            }
            (false, true) => {
                self.remove(above, BY_SIZE);
                self.remove(above, BY_START);
                (*above).start = extent.start;
                (*above).len += extent.len;
                self.insert(above, BY_START);
                self.insert(above, BY_SIZE);
            }
            (false, false) => {
                (*node).start = extent.start;
                (*node).len = extent.len;
                self.insert(node, BY_START);
                self.insert(node, BY_SIZE);
                return;
            }
        }
        if !node.is_null() {
            self.put_spare(node);
        }
    }

    /// Allocate `len` bytes whose start is aligned to `align`, with
    /// `guard_before` and `guard_after` unmapped bytes around them inside the
    /// same allocation. Each is a multiple of the granule, and `align` is a
    /// power of two.
    ///
    /// The search asks for the extent that fits even at the worst alignment,
    /// so it is one descent of the size tree.
    pub fn allocate(
        &mut self,
        len: u64,
        align: u64,
        guard_before: u64,
        guard_after: u64,
    ) -> Result<Allocation, ExtentError> {
        let align = align.max(self.granule);
        let mask = self.granule - 1;
        if len == 0 || !align.is_power_of_two() || (len | guard_before | guard_after) & mask != 0 {
            return Err(ExtentError::BadRequest);
        }
        let total = guard_before
            .checked_add(len)
            .and_then(|bytes| bytes.checked_add(guard_after))
            .ok_or(ExtentError::BadRequest)?;
        let search = total.checked_add(align - self.granule).ok_or(ExtentError::NoVa)?;
        let node = self.smallest_fitting(search);
        if node.is_null() {
            return Err(ExtentError::NoVa);
        }
        // SAFETY: `node` is a free node of this allocator.
        unsafe {
            let (node_start, node_end) = ((*node).start, (*node).start + (*node).len);
            let start = (node_start + guard_before + (align - 1)) & !(align - 1);
            let extent = Extent { start: start - guard_before, len: total };
            let head = extent.start - node_start;
            let tail = node_end - extent.end();
            if head != 0 && tail != 0 && self.spares == 0 {
                return Err(ExtentError::NoNode);
            }
            self.remove(node, BY_SIZE);
            match (head != 0, tail != 0) {
                (false, false) => {
                    self.remove(node, BY_START);
                    self.put_spare(node);
                }
                (true, _) => {
                    (*node).len = head;
                    self.insert(node, BY_SIZE);
                    if tail != 0 {
                        let rest = self.take_spare().expect("a spare node was checked for");
                        (*rest).start = extent.end();
                        (*rest).len = tail;
                        self.insert(rest, BY_START);
                        self.insert(rest, BY_SIZE);
                    }
                }
                (false, true) => {
                    self.remove(node, BY_START);
                    (*node).start = extent.end();
                    (*node).len = tail;
                    self.insert(node, BY_START);
                    self.insert(node, BY_SIZE);
                }
            }
            self.free_bytes -= total;
            Ok(Allocation { extent, start, len })
        }
    }

    /// Hold `extent` out of the free trees until a TLB flush covers it.
    pub fn retire(&mut self, extent: Extent) -> Result<(), ExtentError> {
        self.check(extent)?;
        let node = self.take_spare().ok_or(ExtentError::NoNode)?;
        // SAFETY: `node` is a spare node of this allocator.
        unsafe {
            (*node).start = extent.start;
            (*node).len = extent.len;
            (*node).link[BY_START][LEFT] = self.retired;
        }
        self.retired = node;
        self.retired_bytes += extent.len;
        Ok(())
    }

    /// Detach the retired extents, for one flush to cover.
    pub fn take_retired(&mut self) -> Retired {
        let retired = Retired { head: self.retired };
        self.retired = null_mut();
        self.retired_bytes = 0;
        retired
    }

    /// Return the extents of a flushed `Retired` list to the free trees.
    /// Each brings its own node, so this needs no spare.
    pub fn release(&mut self, retired: Retired) -> Result<(), ExtentError> {
        let mut node = retired.head;
        while !node.is_null() {
            // SAFETY: the list's nodes are this allocator's, detached by
            // `take_retired`.
            let (next, extent) = unsafe {
                ((*node).link[BY_START][LEFT], Extent { start: (*node).start, len: (*node).len })
            };
            let (below, above) = self.neighbours(extent.start);
            // SAFETY: as above.
            let joins = unsafe {
                if (!below.is_null() && (*below).start + (*below).len > extent.start)
                    || (!above.is_null() && (*above).start < extent.end())
                {
                    return Err(ExtentError::Overlap);
                }
                (
                    !below.is_null() && (*below).start + (*below).len == extent.start,
                    !above.is_null() && (*above).start == extent.end(),
                )
            };
            // SAFETY: as above.
            unsafe { self.link_free(extent, node, below, above, joins.0, joins.1) };
            self.free_bytes += extent.len;
            node = next;
        }
        Ok(())
    }

    /// The free extents in address order.
    pub fn for_each_free(&self, mut each: impl FnMut(Extent)) {
        // SAFETY: the tree's nodes are this allocator's.
        unsafe fn walk(node: *mut Node, each: &mut impl FnMut(Extent)) {
            if node.is_null() {
                return;
            }
            walk((*node).link[BY_START][LEFT], each);
            each(Extent { start: (*node).start, len: (*node).len });
            walk((*node).link[BY_START][RIGHT], each);
        }
        unsafe { walk(self.roots[BY_START], &mut each) };
    }
}
