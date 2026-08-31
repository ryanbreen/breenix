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

// Read `/sbin/init` from the mounted ext2 root filesystem (x86_64 production
// init launch, #673). See init_image.rs's module doc for why this is a fork
// of aarch64's own copy in main_aarch64.rs rather than a shared helper.
#[cfg(target_arch = "x86_64")]
pub mod init_image;
