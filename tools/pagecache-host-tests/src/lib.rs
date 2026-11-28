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
