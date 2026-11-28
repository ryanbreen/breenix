//! The page cache's radix tree (E3 plan, section 4.3).
//!
//! A digital search tree on the page index (Fredkin 1960; Knuth, TAOCP vol. 3,
//! section 6.3). Each node has 64 slots and consumes 6 bits of the index per
//! level, so a u64 index needs at most 11 levels. The height grows and shrinks
//! with the largest index present.
//!
//! Entry words, by their low two bits: `00` empty (the page is absent), `01` a
//! page whose pfn is in bits 12-63, `10` an interior node, `11` a value entry
//! whose kind is in bits 2-3 (0 is `Hole`; 2 is reserved for E12's multi-index
//! entries and 3 for shadow entries, and neither is constructible here).
//!
//! Each node carries one bitmap per tag (`DIRTY`, `WRITEBACK`), set for each
//! slot whose subtree holds a page with that tag, so tagged iteration visits
//! only marked subtrees.
//!
//! Insertion takes nodes only from a [`NodeList`] the caller reserved before
//! taking the tree's lock, and checks that the reservation covers it before it
//! changes the tree. Removal does not allocate: emptied nodes go onto a list the
//! caller returns to its node source after releasing the lock.
//!
//! The module has no kernel dependency. The kernel supplies the lock and the
//! node source (E1's slab cache from E3-6).

use core::marker::PhantomData;
use core::ptr::{self, NonNull};

/// Index bits each level consumes.
pub const SLOT_BITS: u32 = 6;
/// Slots per node.
pub const SLOTS: usize = 1 << SLOT_BITS;
/// The height that covers the full u64 index range.
pub const MAX_HEIGHT: u32 = 11;
/// Number of tags.
pub const TAGS: usize = 2;

const KIND_MASK: u64 = 0b11;
const KIND_EMPTY: u64 = 0b00;
const KIND_PAGE: u64 = 0b01;
const KIND_NODE: u64 = 0b10;
const KIND_VALUE: u64 = 0b11;
const VALUE_HOLE: u64 = 0;
const PFN_SHIFT: u32 = 12;
const PFN_LIMIT: u64 = 1 << (64 - PFN_SHIFT);

/// A tag on a page entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Dirty = 0,
    Writeback = 1,
}

/// An entry the tree stores at an index: a page or a value. Interior node
/// words stay private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry(u64);

/// What an index holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Absent,
    Page { pfn: u64 },
    Hole,
}

impl Entry {
    /// A page entry for `pfn`, or no entry when the pfn does not fit in 52 bits.
    pub const fn page(pfn: u64) -> Option<Entry> {
        if pfn >= PFN_LIMIT {
            return None;
        }
        Some(Entry((pfn << PFN_SHIFT) | KIND_PAGE))
    }

    /// The value entry for a page the filesystem reported as a hole.
    pub const fn hole() -> Entry {
        Entry((VALUE_HOLE << 2) | KIND_VALUE)
    }

    pub const fn slot(self) -> Slot {
        match self.0 & KIND_MASK {
            KIND_PAGE => Slot::Page {
                pfn: self.0 >> PFN_SHIFT,
            },
            KIND_VALUE => Slot::Hole,
            _ => Slot::Absent,
        }
    }

    const fn is_page(self) -> bool {
        self.0 & KIND_MASK == KIND_PAGE
    }
}

/// A tree node: 64 entry words, an occupancy bitmap and one bitmap per tag.
#[repr(C, align(64))]
pub struct Node {
    slots: [u64; SLOTS],
    present: u64,
    tags: [u64; TAGS],
}

impl Node {
    const EMPTY: Node = Node {
        slots: [0; SLOTS],
        present: 0,
        tags: [0; TAGS],
    };
}

/// Where tree nodes come from. A [`NodeList`] and a [`PageTree`] are bound to
/// one source type, so a node can only go back to a source of the type that
/// allocated it.
///
/// # Safety
///
/// `alloc_node` returns memory valid for reads and writes of one `Node`, owned
/// by the caller until it is passed back to `free_node`. Every instance of the
/// implementing type draws from one allocator: `free_node` on any instance
/// accepts a node from `alloc_node` on any instance of the same type. A type
/// whose instances own separate storage does not meet this.
pub unsafe trait NodeSource {
    fn alloc_node(&mut self) -> Option<NonNull<Node>>;
    /// # Safety
    ///
    /// `node` came from `alloc_node` on an instance of this type and is no
    /// longer referenced.
    unsafe fn free_node(&mut self, node: NonNull<Node>);
}

/// A list of nodes from sources of type `S`, threaded through their first
/// slot. It needs no capacity, so a removal records what it frees without
/// allocating.
pub struct NodeList<S> {
    head: Option<NonNull<Node>>,
    len: usize,
    source: PhantomData<fn() -> S>,
}

/// A reservation failed: the source refused a node. The reservation it was
/// filling is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReserveError;

impl<S: NodeSource> NodeList<S> {
    pub const fn new() -> NodeList<S> {
        NodeList {
            head: None,
            len: 0,
            source: PhantomData,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Take `count` more nodes from `source`. On failure the nodes this call
    /// took go back to the source and the list is as it was.
    pub fn fill_from(&mut self, source: &mut S, count: usize) -> Result<(), ReserveError> {
        let mut taken = NodeList::new();
        for _ in 0..count {
            match source.alloc_node() {
                Some(node) => taken.put(node),
                None => {
                    taken.release(source);
                    return Err(ReserveError);
                }
            }
        }
        while let Some(node) = taken.get() {
            self.put(node);
        }
        Ok(())
    }

    /// Return the list's nodes to `source`.
    pub fn release(&mut self, source: &mut S) {
        while let Some(node) = self.get() {
            // SAFETY: the nodes on a `NodeList<S>` came from `alloc_node` on
            // an instance of `S` (through `fill_from`, or through a
            // `PageTree<S>` that took them from such a list), which the trait
            // contract lets any instance of `S` free; only the list refers to
            // them.
            unsafe { source.free_node(node) };
        }
    }

    fn put(&mut self, node: NonNull<Node>) {
        let next = self.head.map_or(0, |n| n.as_ptr() as u64);
        // SAFETY: the node is valid for writes and owned by this list.
        unsafe { ptr::addr_of_mut!((*node.as_ptr()).slots[0]).write(next) };
        self.head = Some(node);
        self.len += 1;
    }

    fn get(&mut self) -> Option<NonNull<Node>> {
        let node = self.head?;
        // SAFETY: `push` wrote the link into the node's first slot.
        let next = unsafe { ptr::addr_of!((*node.as_ptr()).slots[0]).read() };
        self.head = NonNull::new(next as *mut Node);
        self.len -= 1;
        Some(node)
    }
}

impl<S: NodeSource> Default for NodeList<S> {
    fn default() -> Self {
        NodeList::new()
    }
}

/// Why a store or a tag change was refused; the tree is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeError {
    /// The reservation holds fewer nodes than the store needs.
    NeedNodes { needed: usize, reserved: usize },
    /// Tags apply only to page entries.
    NotAPage,
}

/// The tree. Height 0 holds at most index 0, in `root` itself; height h > 0
/// has a root node covering indices below 64^h. Its owner empties it with
/// [`PageTree::clear`] and returns the nodes before dropping it. Its nodes
/// come from, and go back to, sources of type `S`.
pub struct PageTree<S> {
    root: u64,
    height: u32,
    root_tags: u8,
    nodes: usize,
    source: PhantomData<fn() -> S>,
}

// SAFETY: the tree is the sole owner of its nodes; the kernel serializes access
// with the FileCache's tree lock.
unsafe impl<S: NodeSource> Send for PageTree<S> {}

/// The height whose tree holds `index`.
pub const fn height_for(index: u64) -> u32 {
    let mut height = 0;
    while height < MAX_HEIGHT && index > max_index(height) {
        height += 1;
    }
    height
}

/// The largest index a tree of `height` holds.
pub const fn max_index(height: u32) -> u64 {
    if height == 0 {
        0
    } else if height * SLOT_BITS >= 64 {
        u64::MAX
    } else {
        (1u64 << (height * SLOT_BITS)) - 1
    }
}

/// A reservation that covers `leaves` stores into a tree of at most `height`
/// once grown: up to `height` nodes to grow the root chain, and fewer than
/// `height` new path nodes for each leaf.
pub const fn worst_case_nodes(height: u32, leaves: usize) -> usize {
    height as usize * (leaves + 1)
}

const fn slot_of(index: u64, level: u32) -> usize {
    let shift = level * SLOT_BITS;
    if shift >= 64 {
        0
    } else {
        ((index >> shift) as usize) & (SLOTS - 1)
    }
}

fn node_ptr(word: u64) -> *mut Node {
    debug_assert_eq!(word & KIND_MASK, KIND_NODE);
    (word & !KIND_MASK) as *mut Node
}

impl<S: NodeSource> PageTree<S> {
    pub const fn new() -> PageTree<S> {
        PageTree {
            root: KIND_EMPTY,
            height: 0,
            root_tags: 0,
            nodes: 0,
            source: PhantomData,
        }
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The nodes the tree holds.
    pub const fn nodes(&self) -> usize {
        self.nodes
    }

    pub const fn is_empty(&self) -> bool {
        self.root == KIND_EMPTY
    }

    /// What `index` holds.
    pub fn lookup(&self, index: u64) -> Slot {
        self.entry_word(index).map_or(Slot::Absent, |w| Entry(w).slot())
    }

    fn entry_word(&self, index: u64) -> Option<u64> {
        if index > max_index(self.height) || self.root == KIND_EMPTY {
            return None;
        }
        let mut word = self.root;
        let mut level = self.height;
        while level > 0 {
            level -= 1;
            // SAFETY: interior words point at nodes the tree owns.
            let node = unsafe { &*node_ptr(word) };
            word = node.slots[slot_of(index, level)];
            if word == KIND_EMPTY {
                return None;
            }
        }
        Some(word)
    }

    /// The nodes a store at `index` would take from its reservation now.
    pub fn nodes_needed(&self, index: u64) -> usize {
        if self.root == KIND_EMPTY {
            return height_for(index) as usize;
        }
        let target = height_for(index);
        if target > self.height {
            // New roots for the levels from the old height up, then a fresh
            // path below the top root: the index's top slot is not 0.
            return (target - self.height) as usize + (target - 1) as usize;
        }
        let mut word = self.root;
        let mut level = self.height;
        while level > 0 {
            level -= 1;
            // SAFETY: interior words point at nodes the tree owns.
            let node = unsafe { &*node_ptr(word) };
            let next = node.slots[slot_of(index, level)];
            if level > 0 && next == KIND_EMPTY {
                return level as usize;
            }
            word = next;
        }
        0
    }

    /// Store `entry` at `index`, returning what the index held. The index's
    /// tags are cleared: they described the entry being replaced. Nodes come
    /// from `reserved`; if it is short, the tree is unchanged.
    pub fn store(&mut self, index: u64, entry: Entry, reserved: &mut NodeList<S>) -> Result<Slot, TreeError> {
        let needed = self.nodes_needed(index);
        if needed > reserved.len() {
            return Err(TreeError::NeedNodes {
                needed,
                reserved: reserved.len(),
            });
        }
        if self.root == KIND_EMPTY {
            self.height = height_for(index);
            self.root_tags = 0;
            if self.height > 0 {
                self.root = self.take(reserved) as u64 | KIND_NODE;
            }
        } else {
            let target = height_for(index);
            while self.height < target {
                let node = self.take(reserved);
                // SAFETY: `take` returned a node the tree now owns alone.
                let node_ref = unsafe { &mut *node };
                node_ref.slots[0] = self.root;
                node_ref.present = 1;
                for tag in 0..TAGS {
                    node_ref.tags[tag] = u64::from(self.root_tags >> tag & 1);
                }
                self.root = node as u64 | KIND_NODE;
                self.height += 1;
            }
        }
        if self.height == 0 {
            let old = Entry(self.root).slot();
            self.root = entry.0;
            self.root_tags = 0;
            return Ok(old);
        }
        let mut path = [ptr::null_mut::<Node>(); MAX_HEIGHT as usize];
        let mut node = node_ptr(self.root);
        let mut level = self.height - 1;
        loop {
            path[level as usize] = node;
            if level == 0 {
                break;
            }
            let slot = slot_of(index, level);
            // SAFETY: `node` is a node the tree owns; only the tree refers to it.
            let current = unsafe { &mut *node };
            if current.slots[slot] == KIND_EMPTY {
                current.slots[slot] = self.take(reserved) as u64 | KIND_NODE;
                current.present |= 1 << slot;
            }
            node = node_ptr(current.slots[slot]);
            level -= 1;
        }
        let slot = slot_of(index, 0);
        // SAFETY: as above.
        let leaf = unsafe { &mut *node };
        let old = Entry(leaf.slots[slot]).slot();
        leaf.slots[slot] = entry.0;
        leaf.present |= 1 << slot;
        let had_tags = (0..TAGS).any(|t| leaf.tags[t] & (1 << slot) != 0);
        if had_tags {
            for tag in [Tag::Dirty, Tag::Writeback] {
                self.clear_tag_on_path(index, tag, &path);
            }
        }
        Ok(old)
    }

    fn take(&mut self, reserved: &mut NodeList<S>) -> *mut Node {
        let node = reserved
            .get()
            .expect("store checked the reservation before changing the tree");
        // SAFETY: a reserved node is valid for writes and owned by the list,
        // whose ownership now passes to the tree.
        unsafe { node.as_ptr().write(Node::EMPTY) };
        self.nodes += 1;
        node.as_ptr()
    }

    /// Remove `index`, returning what it held. Nodes that empty go onto
    /// `freed`, and the height shrinks to the largest index left.
    pub fn remove(&mut self, index: u64, freed: &mut NodeList<S>) -> Slot {
        if self.root == KIND_EMPTY || index > max_index(self.height) {
            return Slot::Absent;
        }
        if self.height == 0 {
            let old = Entry(self.root).slot();
            self.root = KIND_EMPTY;
            self.root_tags = 0;
            return old;
        }
        let mut path = [ptr::null_mut::<Node>(); MAX_HEIGHT as usize];
        let mut node = node_ptr(self.root);
        let mut level = self.height - 1;
        loop {
            path[level as usize] = node;
            if level == 0 {
                break;
            }
            // SAFETY: a node the tree owns.
            let word = unsafe { (*node).slots[slot_of(index, level)] };
            if word == KIND_EMPTY {
                return Slot::Absent;
            }
            node = node_ptr(word);
            level -= 1;
        }
        let slot = slot_of(index, 0);
        // SAFETY: a node the tree owns.
        let old = Entry(unsafe { (*node).slots[slot] }).slot();
        if old == Slot::Absent {
            return Slot::Absent;
        }
        for tag in [Tag::Dirty, Tag::Writeback] {
            self.clear_tag_on_path(index, tag, &path);
        }
        // Clear the slot, then free each node that empties, bottom up.
        let mut level = 0;
        loop {
            // SAFETY: a node the tree owns.
            let current = unsafe { &mut *path[level as usize] };
            let slot = slot_of(index, level);
            current.slots[slot] = KIND_EMPTY;
            current.present &= !(1 << slot);
            if current.present != 0 {
                break;
            }
            self.nodes -= 1;
            // SAFETY: the node is unlinked below (or is the root, cleared
            // below), so only the list refers to it.
            freed.put(unsafe { NonNull::new_unchecked(path[level as usize]) });
            if level + 1 == self.height {
                self.root = KIND_EMPTY;
                self.height = 0;
                self.root_tags = 0;
                return old;
            }
            level += 1;
        }
        self.shrink(freed);
        old
    }

    /// Collapse root nodes whose only occupant is slot 0.
    fn shrink(&mut self, freed: &mut NodeList<S>) {
        while self.height > 0 {
            let root = node_ptr(self.root);
            // SAFETY: a node the tree owns.
            let node = unsafe { &*root };
            if node.present != 1 {
                return;
            }
            let mut tags = 0u8;
            for tag in 0..TAGS {
                tags |= ((node.tags[tag] & 1) as u8) << tag;
            }
            self.root = node.slots[0];
            self.root_tags = tags;
            self.height -= 1;
            self.nodes -= 1;
            // SAFETY: the root was just replaced by its only child.
            freed.put(unsafe { NonNull::new_unchecked(root) });
        }
    }

    /// Empty the tree, putting its nodes on `freed`.
    pub fn clear(&mut self, freed: &mut NodeList<S>) {
        if self.height > 0 {
            Self::free_subtree(node_ptr(self.root), self.height - 1, freed);
        }
        self.root = KIND_EMPTY;
        self.height = 0;
        self.root_tags = 0;
        self.nodes = 0;
    }

    fn free_subtree(node: *mut Node, level: u32, freed: &mut NodeList<S>) {
        if level > 0 {
            // SAFETY: a node the tree owns.
            let mut bits = unsafe { (*node).present };
            while bits != 0 {
                let slot = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                // SAFETY: as above.
                let child = unsafe { (*node).slots[slot] };
                Self::free_subtree(node_ptr(child), level - 1, freed);
            }
        }
        // SAFETY: its parent is being freed with it.
        freed.put(unsafe { NonNull::new_unchecked(node) });
    }

    /// Whether any page carries `tag`: the root's bit, so O(1).
    pub const fn any_tagged(&self, tag: Tag) -> bool {
        self.root_tags & (1 << tag as u8) != 0
    }

    /// Whether the page at `index` carries `tag`.
    pub fn tagged(&self, index: u64, tag: Tag) -> bool {
        if self.root == KIND_EMPTY || index > max_index(self.height) {
            return false;
        }
        if self.root_tags & (1 << tag as u8) == 0 {
            return false;
        }
        let mut word = self.root;
        let mut level = self.height;
        while level > 0 {
            level -= 1;
            // SAFETY: a node the tree owns.
            let node = unsafe { &*node_ptr(word) };
            let slot = slot_of(index, level);
            if node.tags[tag as usize] & (1 << slot) == 0 {
                return false;
            }
            word = node.slots[slot];
        }
        true
    }

    /// Tag the page at `index`, marking the nodes on its path.
    pub fn set_tag(&mut self, index: u64, tag: Tag) -> Result<(), TreeError> {
        match self.entry_word(index) {
            Some(word) if Entry(word).is_page() => {}
            _ => return Err(TreeError::NotAPage),
        }
        self.root_tags |= 1 << tag as u8;
        let mut word = self.root;
        let mut level = self.height;
        while level > 0 {
            level -= 1;
            // SAFETY: a node the tree owns.
            let node = unsafe { &mut *node_ptr(word) };
            let slot = slot_of(index, level);
            node.tags[tag as usize] |= 1 << slot;
            word = node.slots[slot];
        }
        Ok(())
    }

    /// Clear `tag` on `index`, and on each ancestor whose subtree then holds no
    /// page with it. Clearing an untagged index leaves the tree as it was.
    pub fn clear_tag(&mut self, index: u64, tag: Tag) {
        if self.root == KIND_EMPTY || index > max_index(self.height) {
            return;
        }
        if self.height == 0 {
            self.root_tags &= !(1 << tag as u8);
            return;
        }
        let mut path = [ptr::null_mut::<Node>(); MAX_HEIGHT as usize];
        let mut node = node_ptr(self.root);
        let mut level = self.height - 1;
        loop {
            path[level as usize] = node;
            if level == 0 {
                break;
            }
            // SAFETY: a node the tree owns.
            let word = unsafe { (*node).slots[slot_of(index, level)] };
            if word == KIND_EMPTY {
                return;
            }
            node = node_ptr(word);
            level -= 1;
        }
        self.clear_tag_on_path(index, tag, &path);
    }

    fn clear_tag_on_path(&mut self, index: u64, tag: Tag, path: &[*mut Node; MAX_HEIGHT as usize]) {
        let mut level = 0;
        while level < self.height {
            // SAFETY: the path holds nodes the tree owns, leaf first.
            let node = unsafe { &mut *path[level as usize] };
            node.tags[tag as usize] &= !(1 << slot_of(index, level));
            if node.tags[tag as usize] != 0 {
                return;
            }
            level += 1;
        }
        self.root_tags &= !(1 << tag as u8);
    }

    /// The first entry at or after `from`.
    pub fn next_present(&self, from: u64) -> Option<(u64, Slot)> {
        self.find(from, None)
    }

    /// The first page at or after `from` that carries `tag`, visiting only
    /// tagged subtrees.
    pub fn next_tagged(&self, from: u64, tag: Tag) -> Option<(u64, Slot)> {
        self.find(from, Some(tag))
    }

    fn find(&self, from: u64, tag: Option<Tag>) -> Option<(u64, Slot)> {
        if self.root == KIND_EMPTY || from > max_index(self.height) {
            return None;
        }
        if let Some(tag) = tag {
            if self.root_tags & (1 << tag as u8) == 0 {
                return None;
            }
        }
        if self.height == 0 {
            return Some((0, Entry(self.root).slot()));
        }
        Self::search(node_ptr(self.root), self.height - 1, 0, from, tag)
    }

    fn search(node: *mut Node, level: u32, base: u64, from: u64, tag: Option<Tag>) -> Option<(u64, Slot)> {
        // SAFETY: a node the tree owns.
        let node = unsafe { &*node };
        let shift = level * SLOT_BITS;
        let first = if from > base { slot_of(from, level) } else { 0 };
        let mut bits = match tag {
            Some(tag) => node.tags[tag as usize],
            None => node.present,
        } & (u64::MAX << first);
        while bits != 0 {
            let slot = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let child_base = base | ((slot as u64) << shift);
            let word = node.slots[slot];
            if level == 0 {
                return Some((child_base, Entry(word).slot()));
            }
            let child_from = if slot == first { from.max(child_base) } else { child_base };
            if let Some(found) = Self::search(node_ptr(word), level - 1, child_base, child_from, tag) {
                return Some(found);
            }
        }
        None
    }
}

impl<S: NodeSource> Default for PageTree<S> {
    fn default() -> Self {
        PageTree::new()
    }
}
