//! Inter-Process Communication (IPC) module
//!
//! This module provides IPC primitives for Breenix:
//! - File descriptors (fd.rs) - Per-process file descriptor tables
//! - Pipes (pipe.rs) - Unidirectional byte streams
//! - Stdin (stdin.rs) - Kernel stdin ring buffer for keyboard input
//! - Poll (poll.rs) - Poll file descriptors for I/O readiness

pub mod fd;
pub mod pipe;
pub mod poll;
pub mod stdin;

// Re-export public API - some of these are not used yet but are part of the public API
pub use fd::{FdKind, FdTable, MAX_FDS};
pub use pipe::create_pipe;
