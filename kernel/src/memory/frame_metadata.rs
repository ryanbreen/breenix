//! Frame metadata for Copy-on-Write reference counting
//!
//! Each physical frame that can be shared needs metadata tracking:
//! - Reference count (how many page tables point to this frame)
//!
//! Design decisions:
//! - Uses BTreeMap for sparse storage (only track shared frames)
//! - Untracked frames are assumed to have refcount=1 (private)
//! - Untracked frames CAN be freed: frame_decref on an untracked frame
//!   returns true (refcount 1->0), allowing proper cleanup on process exit
//! - Single global lock (acceptable for initial implementation)

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PhysFrame;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::PhysFrame;

/// Global frame metadata storage
/// Uses BTreeMap for sparse storage - only frames that need tracking are stored
static FRAME_METADATA: Mutex<BTreeMap<u64, FrameMetadata>> = Mutex::new(BTreeMap::new());

/// Metadata for a single physical frame
#[derive(Debug)]
struct FrameMetadata {
    /// Number of page tables referencing this frame
    /// 0 = frame is free (should be removed from map)
    /// 1 = frame is private (can be written directly)
    /// >1 = frame is shared (CoW semantics apply)
    refcount: AtomicU32,
}

impl FrameMetadata {
    fn new(initial_count: u32) -> Self {
        Self {
            refcount: AtomicU32::new(initial_count),
        }
    }
}

/// Register a frame in the metadata system with refcount=1 (private)
///
/// Call this when allocating a new frame that will be tracked for cleanup.
/// This is used by CoW fault handlers to register replacement frames so they
/// can be properly freed when the process exits.
///
/// If the frame is already tracked, this is a no-op.
#[allow(dead_code)] // Public API for explicit frame tracking
pub fn frame_register(frame: PhysFrame) {
    let addr = frame.start_address().as_u64();
    let mut metadata = FRAME_METADATA.lock();

    if !metadata.contains_key(&addr) {
        metadata.insert(addr, FrameMetadata::new(1));
    }
}

/// Increment reference count for a frame
/// Called when fork() shares a page between parent and child
#[allow(dead_code)]
pub fn frame_incref(frame: PhysFrame) {
    let addr = frame.start_address().as_u64();
    let mut metadata = FRAME_METADATA.lock();

    if let Some(meta) = metadata.get(&addr) {
        meta.refcount.fetch_add(1, Ordering::SeqCst);
    } else {
        // First time tracking this frame - it's being shared
        // When we start tracking, the frame is being shared between 2 processes
        let meta = FrameMetadata::new(2);
        metadata.insert(addr, meta);
    }
}

/// Decrement reference count for a frame
/// Returns true if frame can be freed (refcount reached 0)
pub fn frame_decref(frame: PhysFrame) -> bool {
    let addr = frame.start_address().as_u64();
    let mut metadata = FRAME_METADATA.lock();

    if let Some(meta) = metadata.get(&addr) {
        let old_count = meta.refcount.fetch_sub(1, Ordering::SeqCst);
        if old_count == 1 {
            // Was 1, now 0 - remove from tracking and allow free
            metadata.remove(&addr);
            return true;
        } else if old_count == 0 {
            // This shouldn't happen - underflow protection
            log::error!(
                "frame_decref: underflow for frame {:#x}, restoring to 0",
                addr
            );
            meta.refcount.store(0, Ordering::SeqCst);
            metadata.remove(&addr);
            return false;
        }
        // old_count > 1, still shared
        false
    } else {
        // Frame wasn't tracked in CoW metadata.
        // This means it's a private frame that was never shared via CoW
        // (e.g., allocated during ELF loading, brk, or stack growth).
        // It belongs solely to the exiting process, so it's safe to free.
        //
        // We only reach here from cleanup_cow_frames / cleanup_for_exec
        // which iterate USER_ACCESSIBLE pages — all of which belong to
        // the process being cleaned up.
        log::trace!(
            "frame_decref: frame {:#x} not tracked (private), allowing free",
            addr
        );
        true
    }
}

/// Get current reference count for a frame
/// Returns 1 if frame is not tracked (assumed private)
pub fn frame_refcount(frame: PhysFrame) -> u32 {
    let addr = frame.start_address().as_u64();
    let metadata = FRAME_METADATA.lock();

    metadata
        .get(&addr)
        .map(|m| m.refcount.load(Ordering::SeqCst))
        .unwrap_or(1) // Untracked frames are private
}

/// Check if a frame is shared (refcount > 1)
pub fn frame_is_shared(frame: PhysFrame) -> bool {
    frame_refcount(frame) > 1
}

/// Get statistics about frame metadata tracking
/// Returns (tracked_frames, total_refcount)
#[allow(dead_code)] // Diagnostic API for future CoW debugging
pub fn frame_metadata_stats() -> (usize, u64) {
    let metadata = FRAME_METADATA.lock();
    let tracked = metadata.len();
    let total_refs: u64 = metadata
        .values()
        .map(|m| m.refcount.load(Ordering::Relaxed) as u64)
        .sum();
    (tracked, total_refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use x86_64::PhysAddr;
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::PhysAddr;

    fn test_frame(addr: u64) -> PhysFrame {
        PhysFrame::containing_address(PhysAddr::new(addr))
    }

    #[test_case]
    fn test_untracked_frame_is_private() {
        let frame = test_frame(0x1000_0000);
        assert_eq!(frame_refcount(frame), 1);
        assert!(!frame_is_shared(frame));
    }

    #[test_case]
    fn test_incref_creates_shared() {
        let frame = test_frame(0x2000_0000);
        frame_incref(frame);
        assert_eq!(frame_refcount(frame), 2);
        assert!(frame_is_shared(frame));

        // Cleanup
        frame_decref(frame);
        frame_decref(frame);
    }

    #[test_case]
    fn test_multiple_incref() {
        let frame = test_frame(0x3000_0000);
        frame_incref(frame); // Now 2
        frame_incref(frame); // Now 3
        frame_incref(frame); // Now 4
        assert_eq!(frame_refcount(frame), 4);

        // Cleanup
        while frame_refcount(frame) > 1 {
            frame_decref(frame);
        }
        frame_decref(frame);
    }

    #[test_case]
    fn test_decref_to_zero() {
        let frame = test_frame(0x4000_0000);
        frame_incref(frame); // Now 2

        assert!(!frame_decref(frame)); // Now 1, not freeable
        assert!(frame_decref(frame)); // Now 0, freeable
        assert_eq!(frame_refcount(frame), 1); // Back to untracked
    }

    #[test_case]
    fn test_decref_untracked_allows_free() {
        // Untracked frames are private — decref should allow freeing
        let frame = test_frame(0x5000_0000);
        assert!(frame_decref(frame)); // Untracked, returns true
    }

    #[test_case]
    fn test_register_then_decref() {
        let frame = test_frame(0x6000_0000);
        frame_register(frame); // Tracked at refcount=1
        assert_eq!(frame_refcount(frame), 1);
        assert!(!frame_is_shared(frame));

        assert!(frame_decref(frame)); // 1->0, freeable
        assert_eq!(frame_refcount(frame), 1); // Back to untracked
    }

    #[test_case]
    fn test_register_then_incref_then_decref() {
        // Simulates: allocate CoW copy, then fork again, then both exit
        let frame = test_frame(0x7000_0000);
        frame_register(frame); // rc=1
        frame_incref(frame); // rc=2 (forked)
        assert_eq!(frame_refcount(frame), 2);
        assert!(frame_is_shared(frame));

        assert!(!frame_decref(frame)); // rc=1, still referenced
        assert!(frame_decref(frame)); // rc=0, freeable
    }
}
