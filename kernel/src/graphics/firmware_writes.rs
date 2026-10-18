//! Kernel writes to the firmware framebuffer after the GPU console's handoff.
//!
//! `docs/gui/gpu-abi.md`, G7 display interface, `firmware_writes_after_handoff` (ruling 50):
//! a device-wide, monotonic, saturating count of kernel store, fill and copy operations
//! that write the firmware framebuffer once `handoff` has become 1. It is reset only at
//! boot. Lock-free, so any writer, including a panic path, may count.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static HANDED_OFF: AtomicBool = AtomicBool::new(false);
static WRITES: AtomicU64 = AtomicU64::new(0);

/// The GPU console's first present completed: count kernel writes from now on.
pub fn handoff() {
    HANDED_OFF.store(true, Ordering::Release);
}

/// One kernel operation stored to the firmware framebuffer.
#[inline]
pub fn note() {
    if HANDED_OFF.load(Ordering::Acquire) {
        let _ = WRITES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
    }
}

/// The operations counted since handoff.
pub fn count() -> u64 {
    WRITES.load(Ordering::Acquire)
}
