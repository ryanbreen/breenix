//! Boot-related utilities
//!
//! This module contains boot-time utilities such as test disk loading
//! and the canonical test binary list.

// test_disk uses VirtIO block_mmio which is ARM64-only
#[cfg(target_arch = "aarch64")]
pub mod test_disk;

// Canonical list of test binaries shared by both x86_64 and ARM64
#[cfg(feature = "testing")]
pub mod test_list;
