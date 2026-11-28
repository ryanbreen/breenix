//! The E3 page cache's pure modules, compiled unchanged from the kernel tree.

#[path = "../../../kernel/src/memory/dirty_limit.rs"]
pub mod dirty_limit;
#[path = "../../../kernel/src/memory/lru.rs"]
pub mod lru;
#[path = "../../../kernel/src/memory/oom_score.rs"]
pub mod oom_score;
#[path = "../../../kernel/src/memory/page_state.rs"]
pub mod page_state;
#[path = "../../../kernel/src/memory/page_tree.rs"]
pub mod page_tree;
#[path = "../../../kernel/src/memory/readahead.rs"]
pub mod readahead;

/// Checks the compiler makes: each snippet must be refused with a type
/// mismatch (E0308).
///
/// A node reserved from one node source cannot be returned to another:
///
/// ```compile_fail,E0308
/// use core::ptr::NonNull;
/// use pagecache_host_tests::page_tree::{Node, NodeList, NodeSource};
/// struct A;
/// struct B;
/// unsafe impl NodeSource for A {
///     fn alloc_node(&mut self) -> Option<NonNull<Node>> { None }
///     unsafe fn free_node(&mut self, _: NonNull<Node>) {}
/// }
/// unsafe impl NodeSource for B {
///     fn alloc_node(&mut self) -> Option<NonNull<Node>> { None }
///     unsafe fn free_node(&mut self, _: NonNull<Node>) {}
/// }
/// let mut list = NodeList::new();
/// let _ = list.fill_from(&mut A, 1);
/// list.release(&mut B);
/// ```
///
/// Nor can a node a tree took from one source's reservation be freed to
/// another source after removal:
///
/// ```compile_fail,E0308
/// use core::ptr::NonNull;
/// use pagecache_host_tests::page_tree::{Entry, Node, NodeList, NodeSource, PageTree};
/// struct A;
/// struct B;
/// unsafe impl NodeSource for A {
///     fn alloc_node(&mut self) -> Option<NonNull<Node>> { None }
///     unsafe fn free_node(&mut self, _: NonNull<Node>) {}
/// }
/// unsafe impl NodeSource for B {
///     fn alloc_node(&mut self) -> Option<NonNull<Node>> { None }
///     unsafe fn free_node(&mut self, _: NonNull<Node>) {}
/// }
/// let mut tree = PageTree::new();
/// let mut reserved = NodeList::new();
/// let _ = reserved.fill_from(&mut A, 2);
/// let _ = tree.store(64, Entry::hole(), &mut reserved);
/// let mut freed = NodeList::new();
/// tree.remove(64, &mut freed);
/// freed.release(&mut B);
/// ```
pub mod compile_checks {}

/// A fixed-seed generator for the randomized cases (a 64-bit LCG, Knuth's
/// MMIX constants).
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}
