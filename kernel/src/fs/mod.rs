//! Filesystem abstraction layer
//!
//! Provides support for various filesystem types including ext2, devfs, and procfs.
//!
//! Boot discovers on-disk formats through the VFS registry; callers use filesystem operations.

// Allow dead code for filesystem modules until they are integrated into kernel init
#![allow(dead_code)]

pub mod attrs;
pub mod devfs;
pub mod devptsfs;
pub mod errno;
macro_rules! filesystems {
    ($($backend:ident),+ $(,)?) => {
        $(pub mod $backend;)+
        pub static FORMATS: &[vfs::Format] = &[$($backend::FORMAT),+];
    };
}
filesystems! { ext2, breenfs }
#[cfg(feature = "ext2_lock_race")]
pub use ext2::lock_race::run_ext2_lock_race_leg;
pub mod locks;
pub mod namei;
pub(crate) mod permissions;
pub mod procfs;
pub mod ufs;
pub mod vfs;
