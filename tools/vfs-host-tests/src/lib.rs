//! Run `cargo test --manifest-path tools/vfs-host-tests/Cargo.toml vfs_host_tests::`.
//! VFS conformance runner over the production ext2 namespace modules.
//! The shared ext2 shim supplies a disk-only mount, with no page mappings or
//! asynchronous finalizer. Kernel lifetime and transfer waits need guest coverage.
#![cfg(test)]
#![feature(allocator_api)]
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
    pub use crate::host_mounts::{mount_root_device, mount_home_device};
    pub use crate::host_mounts::{read_mount, read_mount as sync_mount_guard, write_mount};
    pub use crate::storage::rename::RenameError;
    pub use crate::storage::*;
    pub mod live_inode {
        pub use crate::host_mounts::MountPin;
        pub use crate::storage::live_inode::*;
    }
}
pub mod fs {
    pub use crate::{attrs, errno, ext2, vfs, permissions};
    pub use crate::statfs_support::{namei, ufs};
}
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
pub mod time {
    std::thread_local! {
        static UNIX_TIME: core::cell::Cell<u64> = const { core::cell::Cell::new(1) };
    }
    pub fn current_unix_time() -> u64 {
        UNIX_TIME.with(core::cell::Cell::get)
    }
    pub fn with_unix_time<T>(seconds: u64, f: impl FnOnce() -> T) -> T {
        struct Restore(u64);
        impl Drop for Restore {
            fn drop(&mut self) { UNIX_TIME.with(|clock| clock.set(self.0)); }
        }
        let _restore = Restore(UNIX_TIME.with(|clock| clock.replace(seconds)));
        f()
    }
}

// Scope execution to the conformance module: storage modules also carry their
// own unit tests, which are run separately by ext2-host-tests.
#[path = "../tests/vfs_host_tests.rs"]
mod vfs_host_tests;

// Share the pinned-frame fixture and production mapping entry points.
pub mod memory {
    pub fn physical_memory_offset() -> HostOffset { HostOffset }
    pub struct HostOffset;
    impl HostOffset { pub fn as_u64(&self) -> u64 { 0 } }
    pub mod file_map { include!("../../breenfs-host-tests/src/mapping_support.rs"); }
}

pub use vfs::mount_table;
#[path = "../tests/mount_table_tests.rs"]
mod mount_table_tests;

#[path = "../../../kernel/src/fs/permissions.rs"]
pub mod permissions;
#[path = "../../../kernel/src/process/credentials.rs"]
pub mod credentials;
mod attribute_support;
pub use attribute_support::{process, task, test_control};

#[path = "../../../kernel/src/fs/vfs/exec.rs"]
pub mod exec;

pub use vfs::format;
#[path = "../../../kernel/src/fs/ext2/format.rs"]
pub mod ext2_format;
#[path = "../tests/boot_mount_tests.rs"]
mod boot_mount_tests;

pub mod statfs_support;

#[path = "breenfs.rs"]
pub mod breenfs;
#[path = "../tests/read_conformance.rs"]
mod read_conformance;
#[path = "../tests/manifest_walk.rs"]
mod manifest_walk;
#[path = "../tests/write_conformance.rs"]
mod write_conformance;

#[path = "../tests/model.rs"]
mod model;

mod trait_adapter;

#[path = "../tests/breenfs_boot_probe.rs"]
mod breenfs_boot_probe;

#[path = "../tests/writable_owner.rs"]
mod writable_owner;
