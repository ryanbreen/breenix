//! Host runner for the production ext2 storage modules and their unit tests.
//! Run with `cargo test --manifest-path tools/ext2-host-tests/Cargo.toml`.
#![cfg(test)]
extern crate alloc;
pub mod block;
pub mod ext2;
pub mod fs {
    pub use crate::ext2;
}
pub mod time {
    pub fn current_unix_time() -> u64 {
        1
    }
}
