//! BreenGL's drawing library (docs/gui/breengel.md sections 2 to 8.2), drawing through BGL on
//! the GPU core (docs/gui/gpu.md section 2.1). A `Surface` is a GPU render target; each
//! primitive computes its clipped geometry on the CPU and emits one or a few instances of a
//! profile pipeline (`solid`, `line`, `circle_fill`, `circle_outline`, `rounded_rect`,
//! `gradient_v`, `copy`, `copy_scaled`, `over`, `over_straight`, `bitmap_text`). Pixels reach
//! the CPU only through device readback. The CPU raster this replaced is `libgfx-ref`.

#![no_std]

extern crate alloc;

pub use libgfx_pixel::{alpha, color};

pub mod bitmap_font;
pub mod blit;
mod device;
pub mod font;
pub mod shapes;
pub mod stage;
pub mod surface;
pub mod work;

pub mod ttf_font;
