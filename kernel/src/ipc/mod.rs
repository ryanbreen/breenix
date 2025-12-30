//! Inter-Process Communication (IPC) module
//!
//! This module provides IPC primitives for Breenix:
//! - File descriptors (fd.rs) - Per-process file descriptor tables
//! - Pipes (pipe.rs) - Unidirectional byte streams

pub mod fd;
pub mod pipe;

// Re-export public API - some of these are not used yet but are part of the public API
pub use fd::{FdKind, FdTable, MAX_FDS};
pub use pipe::create_pipe;
