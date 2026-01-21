//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

pub mod demo;
pub mod double_buffer;
pub mod font;
pub mod primitives;
#[cfg(feature = "interactive")]
pub mod render_queue;
#[cfg(feature = "interactive")]
pub mod render_task;
pub mod split_screen;
pub mod terminal;
pub mod terminal_manager;

pub use double_buffer::DoubleBufferedFrameBuffer;
