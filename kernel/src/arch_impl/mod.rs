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

// Currently we only support x86_64, so export unconditionally.
// When ARM64 support is added, we'll use cfg to select the appropriate module.
pub mod x86_64;
pub use x86_64 as current;

pub mod traits;
pub use traits::*;
