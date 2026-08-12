//! Frame metadata for Copy-on-Write reference counting
//!
//! Mapping reference counts live beside allocator generation/state, so a frame
//! can be decremented only while the allocator still identifies it as live.
//! Process page tables acquire these references from virtual-page custody
//! records; the legacy helpers remain thin wrappers for CoW queries and tests.

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::PhysFrame;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PhysFrame;

/// Register a frame in the metadata system with refcount=1 (private)
///
/// Call this when allocating a new frame that will be tracked for cleanup.
/// This is used by CoW fault handlers to register replacement frames so they
/// can be properly freed when the process exits.
///
/// If the frame is already tracked, this is a no-op.
#[allow(dead_code)] // Public API for explicit frame tracking
pub fn frame_register(frame: PhysFrame) {
    if frame_refcount(frame) == 0 {
        let _ = crate::memory::frame_allocator::acquire_leaf_mapping(frame);
    }
}

/// Increment reference count for a frame
/// Called when fork() shares a page between parent and child
#[allow(dead_code)]
pub fn frame_incref(frame: PhysFrame) {
    let _ = crate::memory::frame_allocator::acquire_leaf_mapping(frame);
}

/// Decrement reference count for a frame
/// Returns true if frame can be freed (refcount reached 0)
pub fn frame_decref(frame: PhysFrame) -> bool {
    crate::memory::frame_allocator::decref_leaf_mapping(frame)
}

/// Get current reference count for a frame
/// Returns 0 for an unregistered frame. External frames report `u32::MAX`,
/// which makes CoW fault handling copy instead of granting an in-place write.
pub fn frame_refcount(frame: PhysFrame) -> u32 {
    crate::memory::frame_allocator::leaf_mapping_refcount(frame)
}

/// Check if a frame is shared (refcount > 1)
pub fn frame_is_shared(frame: PhysFrame) -> bool {
    frame_refcount(frame) > 1
}

/// Get statistics about frame metadata tracking
/// Returns (tracked_frames, total_refcount)
#[allow(dead_code)] // Diagnostic API for future CoW debugging
pub fn frame_metadata_stats() -> (usize, u64) {
    crate::memory::frame_allocator::leaf_mapping_stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame() -> PhysFrame {
        crate::memory::frame_allocator::allocate_frame().expect("test frame allocation")
    }

    fn return_test_frame(frame: PhysFrame) {
        assert_eq!(
            crate::memory::frame_allocator::deallocate_leaf_frame(frame),
            crate::memory::frame_allocator::ReturnOutcome::Returned
        );
    }

    #[test_case]
    fn test_untracked_frame_fails_closed() {
        let frame = test_frame();
        assert_eq!(frame_refcount(frame), 0);
        assert!(!frame_is_shared(frame));
        assert!(!frame_decref(frame));
        return_test_frame(frame);
    }

    #[test_case]
    fn test_incref_creates_shared() {
        let frame = test_frame();
        frame_register(frame);
        frame_incref(frame);
        assert_eq!(frame_refcount(frame), 2);
        assert!(frame_is_shared(frame));

        frame_decref(frame);
        assert!(frame_decref(frame));
        return_test_frame(frame);
    }

    #[test_case]
    fn test_multiple_incref() {
        let frame = test_frame();
        frame_register(frame);
        frame_incref(frame); // Now 2
        frame_incref(frame); // Now 3
        frame_incref(frame); // Now 4
        assert_eq!(frame_refcount(frame), 4);

        // Cleanup
        while frame_refcount(frame) > 1 {
            frame_decref(frame);
        }
        assert!(frame_decref(frame));
        return_test_frame(frame);
    }

    #[test_case]
    fn test_decref_to_zero() {
        let frame = test_frame();
        frame_register(frame);
        frame_incref(frame); // Now 2

        assert!(!frame_decref(frame)); // Now 1, not freeable
        assert!(frame_decref(frame)); // Now 0, freeable
        assert_eq!(frame_refcount(frame), 0);
        return_test_frame(frame);
    }

    #[test_case]
    fn test_decref_untracked_refuses_free() {
        let frame = test_frame();
        assert!(!frame_decref(frame));
        return_test_frame(frame);
    }

    #[test_case]
    fn test_register_then_decref() {
        let frame = test_frame();
        frame_register(frame); // Tracked at refcount=1
        assert_eq!(frame_refcount(frame), 1);
        assert!(!frame_is_shared(frame));

        assert!(frame_decref(frame)); // 1->0, freeable
        assert_eq!(frame_refcount(frame), 0);
        return_test_frame(frame);
    }

    #[test_case]
    fn test_register_then_incref_then_decref() {
        // Simulates: allocate CoW copy, then fork again, then both exit
        let frame = test_frame();
        frame_register(frame); // rc=1
        frame_incref(frame); // rc=2 (forked)
        assert_eq!(frame_refcount(frame), 2);
        assert!(frame_is_shared(frame));

        assert!(!frame_decref(frame)); // rc=1, still referenced
        assert!(frame_decref(frame)); // rc=0, freeable
        return_test_frame(frame);
    }
}
