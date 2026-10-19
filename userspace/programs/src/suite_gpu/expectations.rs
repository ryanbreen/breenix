//! CPU reference available only to suite expectation calculations.
#[path = "../../../../libs/libgfx-ref/src/rasterizer.rs"]
pub mod rasterizer;
#[path = "../../../../libs/libgfx-ref/src/error.rs"]
pub mod error;
#[path = "../../../../libs/libgfx-ref/src/float.rs"]
pub mod float;
#[path = "../../../../libs/libgfx-ref/src/work.rs"]
pub mod work;
pub mod outline { pub use libfont::outline::LineSegment; }
pub use rasterizer::rasterize;
