//! Host boundary for the production BreenFS adapter's teardown checks.
#[path = "../../../kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "../../../kernel/src/fs/errno.rs"]
pub mod errno;
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
#[path = "../../vfs-host-tests/src/userptr.rs"]
pub mod host_userptr;
pub mod syscall {
    pub use super::host_userptr as userptr;
    pub use crate::syscall_errno as errno;
}
#[path = "../../vfs-host-tests/src/attribute_support.rs"]
mod attribute_support;
#[path = "../../../kernel/src/process/credentials.rs"]
pub mod credentials;
#[path = "../../../kernel/src/fs/permissions.rs"]
pub mod permissions;
pub use attribute_support::{process, task, test_control};
#[path = "../../../kernel/src/fs/vfs/mod.rs"]
pub mod vfs;
pub mod fs {
    pub use crate::{attrs, errno, permissions, vfs};
}
#[path = "test_clock.rs"]
pub mod time;
// The mapping fixture executes the production writeback traversal and copy.
pub mod memory {
    pub fn physical_memory_offset() -> HostOffset { HostOffset }
    pub struct HostOffset;
    impl HostOffset { pub fn as_u64(&self) -> u64 { 0 } }
    pub mod file_map { include!("mapping_support.rs"); }
}
