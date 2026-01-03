//! Virtual File System (VFS) Layer
//!
//! Provides a unified interface for filesystem operations, abstracting
//! the underlying filesystem implementation (ext2, tmpfs, etc.).
//!
//! The VFS layer serves as the interface between system calls and concrete
//! filesystem implementations. It provides:
//!
//! - Abstract inode representation (`VfsInode`)
//! - Open file handles (`OpenFile`)
//! - Mount point management (`mount`, `unmount`, `find_mount`)
//! - Common error types (`VfsError`)
//!
//! # Architecture
//!
//! ```text
//! System Calls (open, read, write, etc.)
//!         |
//!         v
//!     VFS Layer (this module)
//!         |
//!         v
//! Filesystem Implementations (ext2, tmpfs, etc.)
//!         |
//!         v
//!     Block Devices
//! ```

pub mod error;
pub mod file;
pub mod inode;
pub mod mount;

// Suppress unused import warnings for public API re-exports
#[allow(unused_imports)]
pub use error::*;
#[allow(unused_imports)]
pub use file::*;
#[allow(unused_imports)]
pub use inode::*;
#[allow(unused_imports)]
pub use mount::*;
