//! PTY (Pseudo-Terminal) subsystem
//!
//! Provides pseudo-terminal support for remote shell access.

// Allow unused - this is public API for Phase 2+ PTY syscalls:
// - allocate() will be called by posix_openpt syscall
// - MAX_PTYS defines the system limit
// - PtyAllocator.next_pty_num tracks allocation
#![allow(dead_code)]

pub mod pair;

// Re-export PtyPair for external use
pub use pair::PtyPair;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::{Mutex, Once};
use crate::syscall::errno::{ENOMEM, ENOSPC};

/// Maximum number of PTY pairs
pub const MAX_PTYS: u32 = 256;

/// Global PTY allocator
static PTY_ALLOCATOR: Once<Mutex<PtyAllocator>> = Once::new();

struct PtyAllocator {
    next_pty_num: u32,
    pairs: BTreeMap<u32, Arc<PtyPair>>,
}

impl PtyAllocator {
    fn new() -> Self {
        Self {
            next_pty_num: 0,
            pairs: BTreeMap::new(),
        }
    }
}

/// Initialize the PTY subsystem
pub fn init() {
    PTY_ALLOCATOR.call_once(|| Mutex::new(PtyAllocator::new()));
    log::info!("PTY subsystem initialized");
}

/// Allocate a new PTY pair
pub fn allocate() -> Result<Arc<PtyPair>, i32> {
    let mut alloc = PTY_ALLOCATOR.get().ok_or(ENOMEM)?.lock();

    if alloc.next_pty_num >= MAX_PTYS {
        return Err(ENOSPC);
    }

    let pty_num = alloc.next_pty_num;
    alloc.next_pty_num += 1;

    let pair = Arc::new(PtyPair::new(pty_num));
    alloc.pairs.insert(pty_num, pair.clone());

    Ok(pair)
}

/// Get an existing PTY pair by number
pub fn get(pty_num: u32) -> Option<Arc<PtyPair>> {
    PTY_ALLOCATOR.get()?.lock().pairs.get(&pty_num).cloned()
}

/// Release a PTY pair
pub fn release(pty_num: u32) {
    if let Some(alloc) = PTY_ALLOCATOR.get() {
        alloc.lock().pairs.remove(&pty_num);
    }
}

/// List all active (allocated) PTY numbers
/// Used by devpts filesystem for directory listing
pub fn list_active() -> alloc::vec::Vec<u32> {
    match PTY_ALLOCATOR.get() {
        Some(alloc) => alloc.lock().pairs.keys().copied().collect(),
        None => alloc::vec::Vec::new(),
    }
}
