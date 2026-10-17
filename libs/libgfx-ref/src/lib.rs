//! Test-only CPU reference, preserved from breengel-cpu-393.
#![no_std]
extern crate alloc;
pub mod rasterizer;
pub mod work;
pub mod error;
pub mod float;
pub mod outline { pub use libfont::outline::LineSegment; }
