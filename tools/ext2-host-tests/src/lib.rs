//! Host runner for the production ext2 storage modules and their unit tests.
//! Run with `cargo test --manifest-path tools/ext2-host-tests/Cargo.toml`.
#![cfg(test)]
extern crate alloc;
#[path = "../../../kernel/src/boot/intent.rs"]
pub mod intent;
pub mod boot {
    pub use crate::intent;
}
pub mod block;
pub mod ext2;
#[path = "../../../kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "../../../kernel/src/fs/errno.rs"]
pub mod fs_errno;
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod syscall { pub use crate::syscall_errno as errno; }
pub mod fs {
    pub use crate::fs_errno as errno;
    pub use crate::ext2;
    pub use crate::attrs;
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

mod errno {
    use crate::{fs::errno::Errno, syscall::errno::ENOENT};

    #[test]
    fn ino_above_u32_is_enoent() {
        assert_eq!(crate::ext2::ino32(u32::MAX as u64), Ok(u32::MAX));
        for ino in [u32::MAX as u64 + 1, (1u64 << 32) + 2, u64::MAX] {
            assert_eq!(crate::ext2::ino32(ino), Err(Errno(ENOENT)), "{ino}");
        }
    }
}
