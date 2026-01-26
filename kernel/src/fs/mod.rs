//! Filesystem abstraction layer
//!
//! Provides support for various filesystem types including ext2 and devfs.
//!
//! Note: The filesystem layer is complete but not yet integrated into
//! kernel initialization. Call ext2::init_root_fs() to mount the root
//! filesystem before using sys_open().

// Allow dead code for filesystem modules until they are integrated into kernel init
#![allow(dead_code)]

pub mod devfs;
// devptsfs depends on tty module which is x86_64-only
#[cfg(target_arch = "x86_64")]
pub mod devptsfs;
pub mod ext2;
pub mod vfs;
