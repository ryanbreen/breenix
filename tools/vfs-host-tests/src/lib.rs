//! VFS conformance runner over the production ext2 namespace modules.
//! The shared ext2 shim supplies a disk-only mount, with no page mappings or
//! asynchronous finalizer. Kernel lifetime and transfer waits need guest coverage.
extern crate alloc;
#[path = "../../ext2-host-tests/src/block.rs"]
pub mod block;
#[path = "../../../kernel/src/boot/intent.rs"]
pub mod intent;
#[path = "../../ext2-host-tests/src/ext2.rs"]
pub mod storage;
pub mod boot {
    pub use crate::intent;
}
#[path = "../../../kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "../../../kernel/src/fs/errno.rs"]
pub mod errno;
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod syscall {
    pub use crate::syscall_errno as errno;
}
#[path = "../../../kernel/src/fs/ext2/adapter.rs"]
pub mod adapter;
pub mod host_mounts;
#[path = "../../../kernel/src/fs/vfs/mod.rs"]
pub mod vfs;
pub mod ext2 {
    pub use crate::host_mounts::{read_mount, write_mount};
    pub use crate::storage::*;
    pub mod live_inode {
        pub use crate::host_mounts::MountPin;
        pub use crate::storage::live_inode::*;
    }
}
pub mod fs {
    pub use crate::{attrs, errno, ext2, vfs};
}
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
pub mod time {
    pub fn current_unix_time() -> u64 {
        1
    }
}
