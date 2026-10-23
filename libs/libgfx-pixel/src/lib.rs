//! BreenGL's colour value and the alpha formulas of docs/gui/breengel.md section 2, shared
//! by the GPU drawing library (`libgfx`) and its CPU reference (`libgfx-ref`) so both name
//! one `Color` type. Arithmetic on single values only: no surface, no pixel loop.

#![no_std]

pub mod alpha;
pub mod color;
