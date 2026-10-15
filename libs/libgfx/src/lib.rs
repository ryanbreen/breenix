//! Breenix Userspace Graphics Library
//!
//! Pure drawing library operating on raw pixel buffers. No syscall dependencies —
//! callers provide the framebuffer memory and handle flushing themselves.

#![no_std]

extern crate alloc;

pub mod alpha;
pub mod bitmap_font;
pub mod blit;
pub mod color;
pub mod diagnostics;
pub mod font;
pub mod framebuf;
pub mod math;
pub mod shapes;
pub mod ttf_font;
pub mod work;
