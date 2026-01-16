//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

pub mod double_buffer;

pub use double_buffer::DoubleBufferedFrameBuffer;
