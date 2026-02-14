//! Breenix Userspace Graphics Library
//!
//! Pure drawing library operating on raw pixel buffers. No syscall dependencies —
//! callers provide the framebuffer memory and handle flushing themselves.

#![no_std]

pub mod bitmap_font;
pub mod color;
pub mod font;
pub mod framebuf;
pub mod math;
pub mod shapes;
