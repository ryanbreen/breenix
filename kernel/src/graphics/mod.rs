//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

pub mod demo;
pub mod double_buffer;
pub mod font;
pub mod primitives;
pub mod render_queue;
pub mod render_task;

pub use double_buffer::DoubleBufferedFrameBuffer;
