//! The CPU reference for BreenGL drawing (docs/gui/gpu.md section 2.4): libgfx's CPU
//! primitives, blits, text and stage, moved here unchanged when libgfx began drawing through
//! BGL on the GPU core. Its sanctioned consumers are host tests, `tools/gfx-refgen` and the
//! suites' expectation code; tests/breengel_gpu_only.rs refuses it to production crates and
//! lists the consumers that still draw on the CPU until their lanes port them.
//!
//! `rasterizer.rs`, `error.rs` and `float.rs` are frozen copies of libfont's coverage
//! rasterizer (scripts/frozen-copies.json) for the text lane and are not built here yet.

#![no_std]

extern crate alloc;

pub use libgfx_pixel::{alpha, color};

pub mod bitmap_font;
pub mod blit;
pub mod diagnostics;
pub mod font;
pub mod framebuf;
pub mod math;
pub mod shapes;
pub mod stage;
pub mod ttf_font;
/// The frozen copy of libfont's step counter, whose API (`reset`, `steps`, `add`) is the one
/// the moved primitives count into (section 7).
pub mod work;
