//! The kernel's keyed hash, at the path `tcp_ident` names it by, and a pool
//! that reports ready.

#[path = "../../../kernel/src/random/blake2s.rs"]
pub mod blake2s;
#[path = "../../../kernel/src/random/erase.rs"]
mod erase;

pub fn is_ready() -> bool {
    true
}

/// A fixed stream (splitmix64) standing in for the pool.
pub fn fill(out: &mut [u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0x0123_4567_89ab_cdef);
    for byte in out.iter_mut() {
        let s = STATE.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
        let mut z = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        *byte = (z ^ (z >> 31)) as u8;
    }
}

#[path = "../../../kernel/src/random/siphash.rs"]
pub mod siphash;
