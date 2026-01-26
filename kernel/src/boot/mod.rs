//! Boot-related utilities
//!
//! This module contains boot-time utilities such as test disk loading.

// test_disk uses VirtIO block_mmio which is ARM64-only
#[cfg(target_arch = "aarch64")]
pub mod test_disk;
