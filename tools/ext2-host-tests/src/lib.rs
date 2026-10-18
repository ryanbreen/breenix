//! Host runner for the production ext2 storage modules and their unit tests.
//! Run with `cargo test --manifest-path tools/ext2-host-tests/Cargo.toml`.
#![cfg(test)]
extern crate alloc;
#[path = "../../../kernel/src/boot/console_intent.rs"]
pub mod console_intent;
pub mod boot {
    pub use crate::console_intent;
}
pub mod block;
pub mod ext2;
pub mod fs {
    pub use crate::ext2;
}
/// The host has no interrupts to mask.
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
pub mod time {
    pub fn current_unix_time() -> u64 {
        1
    }
}
