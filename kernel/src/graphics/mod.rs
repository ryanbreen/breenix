//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

#[cfg(target_arch = "aarch64")]
pub mod arm64_fb;
#[cfg(target_arch = "x86_64")]
pub mod demo;
pub mod double_buffer;
pub mod font;
#[cfg(target_arch = "aarch64")]
pub mod particles;
pub mod primitives;
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
pub mod render_queue;
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
pub mod render_task;
pub mod split_screen;
pub mod terminal;
pub mod terminal_manager;

pub use double_buffer::DoubleBufferedFrameBuffer;
