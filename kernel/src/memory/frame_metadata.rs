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
//!
//! ## Arch split (x86_64 vs aarch64)
//!
//! The public API (`frame_register`, `frame_incref`, `frame_decref`,
//! `frame_refcount`, `frame_metadata_stats`) is defined twice, once per
//! architecture. The x86_64 bodies are kept byte-identical to the
//! pre-teardown-redesign implementation: a plain `FRAME_METADATA.lock()`,
//! with `log::error!`/`log::trace!` on the underflow/untracked-frame
//! diagnostic paths.
//!
//! aarch64 additionally pins preemption around the critical section
//! (`PinnedMetadataGuard`) and reports underflow/untracked events via
//! atomic counters instead of logging. Both are requirements specific to
//! the proof-gated `kreclaimd` grave/reclamation design: the reclaimer and
//! the IRQ-masked CoW fault retry path (`FaultMetadataTransaction`,
//! aarch64-only, see `arch_impl::aarch64::exception`) can run this code
//! with interrupts masked or from contexts where migrating CPUs mid-update
//! would violate the TTBR0 lease invariants, and `log::*` macros take the
//! SERIAL/framebuffer locks, which is the same class of deadlock documented
//! for kthread_entry/workqueue worker logging (see top-level CLAUDE.md,
//! "Interrupt and Syscall Development"). Neither requirement applies to the
//! x86_64 CoW fault handler, which is not IRQ-masked and has no reclaimer,
//! so x86_64 keeps the original plain-lock, logging implementation.

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::PhysFrame;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::AtomicU64;
use spin::Mutex;
#[cfg(target_arch = "aarch64")]
use spin::MutexGuard;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PhysFrame;

/// Global frame metadata storage
/// Uses BTreeMap for sparse storage - only frames that need tracking are stored
static FRAME_METADATA: Mutex<BTreeMap<u64, FrameMetadata>> = Mutex::new(BTreeMap::new());

// aarch64-only: diagnostic counters read by `task::reclaim::dump_reclaim_state`.
// These replace `log::error!`/`log::trace!` on the aarch64 decref path only
// (see module doc comment for why logging is unsafe there); x86_64 keeps the
// original logging behavior and has no use for these counters.
#[cfg(target_arch = "aarch64")]
pub static FRAME_DECREF_UNDERFLOW: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
pub static FRAME_DECREF_UNTRACKED: AtomicU64 = AtomicU64::new(0);

/// aarch64-only: pins the current CPU (disables preemption) for the
/// duration of a `FRAME_METADATA` critical section. Required so the
/// proof-gated reclaimer and the TTBR0 lease transitions it coordinates
/// with cannot observe a half-updated map from a preempted holder. Not
/// used on x86_64: main's plain `FRAME_METADATA.lock()` is unchanged there.
#[cfg(target_arch = "aarch64")]
struct PinnedMetadataGuard {
    metadata: Option<MutexGuard<'static, BTreeMap<u64, FrameMetadata>>>,
}

#[cfg(target_arch = "aarch64")]
impl PinnedMetadataGuard {
    fn lock() -> Self {
        crate::per_cpu_aarch64::preempt_disable();
        Self {
            metadata: Some(FRAME_METADATA.lock()),
        }
    }

    fn metadata(&self) -> &BTreeMap<u64, FrameMetadata> {
        self.metadata.as_ref().expect("metadata guard missing")
    }

    fn metadata_mut(&mut self) -> &mut BTreeMap<u64, FrameMetadata> {
        self.metadata.as_mut().expect("metadata guard missing")
    }
}

#[cfg(target_arch = "aarch64")]
impl Drop for PinnedMetadataGuard {
    fn drop(&mut self) {
        drop(self.metadata.take());
        crate::per_cpu_aarch64::preempt_enable();
    }
}

/// aarch64-only: returned by `try_fault_transaction` when the metadata lock
/// is currently held, so the IRQ-masked CoW fault path can retry rather than
/// block. No x86_64 equivalent: the x86_64 CoW fault handler is not
/// IRQ-masked and uses `handle_cow_direct`'s own lock-held fallback instead.
#[cfg(target_arch = "aarch64")]
pub struct FrameMetadataRetry;

/// aarch64-only: a held `FRAME_METADATA` lock scoped to a single IRQ-masked
/// CoW fault retry attempt (see `arch_impl::aarch64::exception`).
#[cfg(target_arch = "aarch64")]
pub struct FaultMetadataTransaction {
    metadata: MutexGuard<'static, BTreeMap<u64, FrameMetadata>>,
}

#[cfg(target_arch = "aarch64")]
impl FaultMetadataTransaction {
    pub fn is_shared(&self, frame: PhysFrame) -> bool {
        self.metadata
            .get(&frame.start_address().as_u64())
            .map(|metadata| metadata.refcount.load(Ordering::SeqCst))
            .unwrap_or(1)
            > 1
    }

    pub fn register(&mut self, frame: PhysFrame) {
        self.metadata
            .entry(frame.start_address().as_u64())
            .or_insert_with(|| FrameMetadata::new(1));
    }

    pub fn decref(&mut self, frame: PhysFrame) -> bool {
        decref_locked(&mut self.metadata, frame.start_address().as_u64())
    }
}

#[cfg(target_arch = "aarch64")]
pub fn try_fault_transaction() -> Result<FaultMetadataTransaction, FrameMetadataRetry> {
    FRAME_METADATA
        .try_lock()
        .map(|metadata| FaultMetadataTransaction { metadata })
        .ok_or(FrameMetadataRetry)
}

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
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)] // Public API for explicit frame tracking
pub fn frame_register(frame: PhysFrame) {
    let addr = frame.start_address().as_u64();
    let mut metadata = FRAME_METADATA.lock();

    if !metadata.contains_key(&addr) {
        metadata.insert(addr, FrameMetadata::new(1));
    }
}

/// Register a frame in the metadata system with refcount=1 (private)
/// (aarch64: pins preemption around the critical section, see module docs)
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)] // Public API for explicit frame tracking
pub fn frame_register(frame: PhysFrame) {
    let addr = frame.start_address().as_u64();
    let mut guard = PinnedMetadataGuard::lock();
    let metadata = guard.metadata_mut();

    if !metadata.contains_key(&addr) {
        metadata.insert(addr, FrameMetadata::new(1));
    }
}

/// Increment reference count for a frame
/// Called when fork() shares a page between parent and child
#[cfg(target_arch = "x86_64")]
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

/// Increment reference count for a frame
/// (aarch64: pins preemption around the critical section, see module docs)
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub fn frame_incref(frame: PhysFrame) {
    let addr = frame.start_address().as_u64();
    let mut guard = PinnedMetadataGuard::lock();
    let metadata = guard.metadata_mut();

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
#[cfg(target_arch = "x86_64")]
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

/// Decrement reference count for a frame
/// Returns true if frame can be freed (refcount reached 0)
/// (aarch64: pins preemption around the critical section and reports
/// underflow/untracked events via atomic counters instead of `log::*`,
/// since this runs from IRQ-masked reclaimer/CoW-retry contexts where
/// taking the logger's locks would deadlock — see module docs)
#[cfg(target_arch = "aarch64")]
pub fn frame_decref(frame: PhysFrame) -> bool {
    let addr = frame.start_address().as_u64();
    let mut guard = PinnedMetadataGuard::lock();
    decref_locked(guard.metadata_mut(), addr)
}

#[cfg(target_arch = "aarch64")]
fn decref_locked(metadata: &mut BTreeMap<u64, FrameMetadata>, addr: u64) -> bool {
    if let Some(meta) = metadata.get(&addr) {
        let old_count = meta.refcount.fetch_sub(1, Ordering::SeqCst);
        if old_count == 1 {
            // Was 1, now 0 - remove from tracking and allow free
            metadata.remove(&addr);
            return true;
        } else if old_count == 0 {
            // This shouldn't happen - underflow protection
            FRAME_DECREF_UNDERFLOW.fetch_add(1, Ordering::Relaxed);
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
        // We only reach here from cleanup_for_exec, which iterates process-owned
        // user pages.
        FRAME_DECREF_UNTRACKED.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// Get current reference count for a frame
/// Returns 1 if frame is not tracked (assumed private)
#[cfg(target_arch = "x86_64")]
pub fn frame_refcount(frame: PhysFrame) -> u32 {
    let addr = frame.start_address().as_u64();
    let metadata = FRAME_METADATA.lock();

    metadata
        .get(&addr)
        .map(|m| m.refcount.load(Ordering::SeqCst))
        .unwrap_or(1) // Untracked frames are private
}

/// Get current reference count for a frame
/// Returns 1 if frame is not tracked (assumed private)
/// (aarch64: pins preemption around the critical section, see module docs)
#[cfg(target_arch = "aarch64")]
pub fn frame_refcount(frame: PhysFrame) -> u32 {
    let addr = frame.start_address().as_u64();
    let guard = PinnedMetadataGuard::lock();
    let metadata = guard.metadata();

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
#[cfg(target_arch = "x86_64")]
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

/// Get statistics about frame metadata tracking
/// Returns (tracked_frames, total_refcount)
/// (aarch64: pins preemption around the critical section, see module docs)
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)] // Diagnostic API for future CoW debugging
pub fn frame_metadata_stats() -> (usize, u64) {
    let guard = PinnedMetadataGuard::lock();
    let metadata = guard.metadata();
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
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::PhysAddr;
    #[cfg(target_arch = "x86_64")]
    use x86_64::PhysAddr;

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
