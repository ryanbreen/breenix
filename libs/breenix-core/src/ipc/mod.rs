//! Inter-Process Communication primitives
//!
//! Provides portable pipe buffers and file descriptor table abstractions.

pub mod pipe;
pub mod fd;

pub use fd::{FdKind, FdTable, MAX_FDS};
pub use pipe::create_pipe;
