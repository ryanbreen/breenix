//! Breenix Standard Library
//!
//! This crate provides std-like APIs for Breenix userspace programs,
//! enabling the use of familiar Rust patterns like `println!`, `Vec`,
//! `String`, and `Box` without requiring the full Rust standard library.
//!
//! # Stage 1: Hello World
//!
//! This is the first stage of Breenix std support, providing:
//! - `println!` / `print!` macros for formatted output
//! - `eprintln!` / `eprint!` macros for error output
//! - Global allocator for heap allocation (`Vec`, `Box`, `String`)
//! - Panic handler with location information
//! - Process control (`exit`)
//!
//! # Usage
//!
//! ```rust,ignore
//! #![no_std]
//! #![no_main]
//!
//! extern crate alloc;
//! use breenix_std::prelude::*;
//!
//! #[no_mangle]
//! pub extern "C" fn _start() -> ! {
//!     println!("Hello from Breenix!");
//!
//!     // Heap allocations work too
//!     let v: Vec<i32> = vec![1, 2, 3];
//!     println!("Vector: {:?}", v);
//!
//!     exit(0);
//! }
//! ```
//!
//! # Future Stages
//!
//! - Stage 2: File I/O (`std::fs::File`, `read_to_string`)
//! - Stage 3: Process/Time (`Command`, `Instant`, `SystemTime`)
//! - Stage 4: Threading (`thread::spawn`, `Mutex`, `Arc`)

#![no_std]

// Re-export alloc crate for Vec, Box, String, etc.
extern crate alloc;

// Internal modules
pub mod alloc_impl;
pub mod io;
pub mod panic;
pub mod process;

// Make the allocator available (it's registered as #[global_allocator])
pub use alloc_impl::ALLOCATOR;

// Re-export alloc types for convenience
pub use alloc::{boxed::Box, format, string::String, string::ToString, vec, vec::Vec};

// Re-export libbreenix for low-level access
pub use libbreenix;

/// Prelude module - import this to get all common functionality.
///
/// # Usage
/// ```rust,ignore
/// use breenix_std::prelude::*;
/// ```
pub mod prelude {
    // Macros
    pub use crate::{eprint, eprintln, print, println};

    // Process control
    pub use crate::process::exit;

    // Alloc types
    pub use alloc::{boxed::Box, format, string::String, string::ToString, vec, vec::Vec};
}
