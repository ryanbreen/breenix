//! Inter-Process Communication (IPC) module
//!
//! This module provides IPC primitives for Breenix:
//! - File descriptors (fd.rs) - Per-process file descriptor tables
//! - Pipes (pipe.rs) - Unidirectional byte streams
//! - FIFOs (fifo.rs) - Named pipes for filesystem-based IPC (x86_64 only)
//! - Stdin (stdin.rs) - Kernel stdin ring buffer for keyboard input
//! - Poll (poll.rs) - Poll file descriptors for I/O readiness

pub mod fd;
#[cfg(target_arch = "x86_64")]
pub mod fifo;
pub mod pipe;
pub mod poll;
pub mod stdin;

// Re-export public API - some of these are not used yet but are part of the public API
pub use fd::{FdKind, FdTable, MAX_FDS};
pub use pipe::create_pipe;
