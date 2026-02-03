//! Architecture-independent Copy-on-Write statistics
//!
//! This module provides tracking for CoW page fault handling across all
//! architectures. Both x86_64 and ARM64 use these counters.

use core::sync::atomic::{AtomicU64, Ordering};

/// Total CoW faults handled
pub static TOTAL_FAULTS: AtomicU64 = AtomicU64::new(0);
/// Faults handled via process manager (normal path)
pub static MANAGER_PATH: AtomicU64 = AtomicU64::new(0);
/// Faults handled via direct page table manipulation (lock-held path)
pub static DIRECT_PATH: AtomicU64 = AtomicU64::new(0);
/// Pages that were copied (frame was shared)
pub static PAGES_COPIED: AtomicU64 = AtomicU64::new(0);
/// Pages made writable without copy (sole owner optimization)
pub static SOLE_OWNER_OPT: AtomicU64 = AtomicU64::new(0);

/// Get current CoW statistics
pub fn get_stats() -> CowStats {
    CowStats {
        total_faults: TOTAL_FAULTS.load(Ordering::Relaxed),
        manager_path: MANAGER_PATH.load(Ordering::Relaxed),
        direct_path: DIRECT_PATH.load(Ordering::Relaxed),
        pages_copied: PAGES_COPIED.load(Ordering::Relaxed),
        sole_owner_opt: SOLE_OWNER_OPT.load(Ordering::Relaxed),
    }
}

/// Reset all statistics (for testing)
#[allow(dead_code)]
pub fn reset_stats() {
    TOTAL_FAULTS.store(0, Ordering::Relaxed);
    MANAGER_PATH.store(0, Ordering::Relaxed);
    DIRECT_PATH.store(0, Ordering::Relaxed);
    PAGES_COPIED.store(0, Ordering::Relaxed);
    SOLE_OWNER_OPT.store(0, Ordering::Relaxed);
}

/// CoW statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct CowStats {
    pub total_faults: u64,
    pub manager_path: u64,
    pub direct_path: u64,
    pub pages_copied: u64,
    pub sole_owner_opt: u64,
}

#[allow(dead_code)]
impl CowStats {
    /// Print statistics to serial output
    pub fn print(&self) {
        crate::serial_println!(
            "[COW STATS] total={} manager={} direct={} copied={} sole_owner={}",
            self.total_faults,
            self.manager_path,
            self.direct_path,
            self.pages_copied,
            self.sole_owner_opt
        );
    }
}
