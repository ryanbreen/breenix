//! Filesystem abstraction layer
//!
//! Provides support for various filesystem types including ext2, devfs, and procfs.
//!
//! Note: The filesystem layer is complete but not yet integrated into
//! kernel initialization. Call ext2::init_root_fs() to mount the root
//! filesystem before using sys_open().

// Allow dead code for filesystem modules until they are integrated into kernel init
#![allow(dead_code)]

pub mod devfs;
pub mod devptsfs;
pub mod ext2;
pub mod procfs;
pub mod vfs;
