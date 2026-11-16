//! Host boundary for the production BreenFS adapter's teardown checks.
#[path = "../../../kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "../../../kernel/src/fs/errno.rs"]
pub mod errno;
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod syscall {
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
pub mod time {
    pub fn current_unix_time() -> u64 {
        1
    }
}
// These checks have open VNodes but no guest mappings or frame allocator.
pub mod memory {
    pub mod file_map {
        #[derive(Clone)]
        pub struct MapState;
        impl MapState {
            pub fn new(
                _size: u64,
                _epoch: u64,
                _fs: alloc::sync::Arc<dyn crate::vfs::FileSystem>,
            ) -> Result<Self, crate::errno::Errno> {
                Ok(Self)
            }
            pub fn resync_unbound(&self, _size: u64) {}
            pub fn has_dirty(&self) -> bool {
                false
            }
            pub fn may_change_exec_image(&self) -> bool {
                false
            }
        }
    }
}
