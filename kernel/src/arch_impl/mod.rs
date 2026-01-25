//! Architecture abstraction layer for Breenix.
//!
//! This module provides architecture-agnostic traits and re-exports the current
//! architecture's implementation. Code outside this module should use the traits
//! defined here rather than architecture-specific types directly.
//!
//! # Supported Architectures
//!
//! - `x86_64`: Full support (current primary target)
//! - `aarch64`: Planned (ARM64 for Apple Silicon VMs)

// Select the implementation with cfg; x86_64 is the current primary target
// while aarch64 remains planned.
#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64 as current;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64 as current;

pub mod traits;
pub use traits::*;
