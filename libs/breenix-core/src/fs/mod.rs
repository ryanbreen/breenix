//! Filesystem abstractions for Breenix
//!
//! This module provides portable filesystem types and implementations that
//! can be used across all target architectures (x86_64, aarch64, wasm32).

pub mod vfs;
pub mod devfs;
pub mod ext2;
pub mod ramfs;
