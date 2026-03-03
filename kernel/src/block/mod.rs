//! Block Device Abstraction Layer — re-exported from breenix-core.
//!
//! The hardware-specific VirtIO block driver remains in this module.

pub use breenix_core::block::*;

pub mod virtio;
