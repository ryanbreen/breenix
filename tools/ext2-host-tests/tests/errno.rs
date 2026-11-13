//! The classifier is independent of the storage fixture harness.
#[path = "../../../kernel/src/fs/errno.rs"]
pub mod fs_errno;
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
#[path = "../../../kernel/src/fs/ext2/errno.rs"]
pub mod ext2_errno;
pub mod fs {
    pub use crate::fs_errno as errno;
}
pub mod syscall {
    pub use crate::syscall_errno as errno;
}
pub mod ext2 {
    pub use crate::ext2_errno as errno;
}
#[path = "../src/errno.rs"]
mod errno;
