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
// The ext2/VFS fault-injection leg. Test profile only: every byte of it is
// behind this feature, and `scripts/check-fs-fault-production-clean.sh` proves
// a production ELF carries none of it.
#[cfg(feature = "fs_fault_inject")]
pub mod fault_inject;
// #728 gate-observable repro oracle for ext2 lock discipline. Test profile
// only: the module and its single call site (one per architecture main) are
// behind this feature.
#[cfg(feature = "ext2_lock_race")]
pub mod ext2_lock_race;
pub mod procfs;
pub mod vfs;
