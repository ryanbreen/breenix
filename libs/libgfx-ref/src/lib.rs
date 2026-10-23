//! The CPU reference for BreenGL drawing (docs/gui/gpu.md section 2.4): libgfx's CPU
//! primitives, blits, text and stage, moved here unchanged when libgfx began drawing through
//! BGL on the GPU core. Its sanctioned consumers are host tests, `tools/gfx-refgen` and the
//! suites' expectation code; tests/breengel_gpu_only.rs refuses it to production crates and
//! lists the consumers that still draw on the CPU until their lanes port them.
//!
//! The frozen coverage rasterizer is the CPU text oracle; production keeps only geometry.

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
pub use libfont::work;
pub use libfont::{error, outline};
pub mod float;
pub mod rasterizer;
pub mod cache;
pub mod font_api;
