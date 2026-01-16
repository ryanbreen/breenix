//! Graphics utilities for the Breenix kernel.
//!
//! Provides framebuffer abstractions used by the kernel graphics stack.

pub mod double_buffer;
pub mod font;
pub mod primitives;

pub use double_buffer::DoubleBufferedFrameBuffer;
pub use font::{Font, FontMetrics, FontSize, Glyph, Weight};
pub use primitives::TextStyle;
