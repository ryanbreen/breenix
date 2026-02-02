//! Per-CPU ring buffer implementation for trace events.
//!
//! This module provides a lock-free ring buffer optimized for single-writer
//! (the owning CPU) and occasional multi-reader (GDB, dump functions) access.
//!
//! # Design
//!
//! - Fixed-size ring buffer (1024 entries = 16 KiB)
//! - Power-of-2 size for efficient modulo via bitmask
//! - Atomic write index for lock-free append
//! - Overwrite mode: old events are silently overwritten when full
//!
//! # Memory Layout
//!
//! Each TraceCpuBuffer is cache-line aligned (64 bytes) to prevent
//! false sharing between CPUs.
//!
//! ```text
//! +------------------+
//! | entries[0..1024] |  16384 bytes (1024 * 16)
//! +------------------+
//! | write_idx        |  8 bytes (atomic)
//! +------------------+
//! | read_idx         |  8 bytes (atomic, for dump)
//! +------------------+
//! | dropped          |  8 bytes (atomic, diagnostics)
//! +------------------+
//! | _padding         |  Align to 64 bytes
//! +------------------+
//! ```

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::core::TraceEvent;

// =============================================================================
// Buffer Configuration
// =============================================================================

/// Number of entries in each per-CPU trace buffer.
/// Must be a power of 2 for efficient masking.
pub const TRACE_BUFFER_SIZE: usize = 1024;

/// Mask for efficient modulo operation (size - 1).
const TRACE_BUFFER_MASK: usize = TRACE_BUFFER_SIZE - 1;

// Compile-time verification that buffer size is power of 2
const _: () = assert!(
    TRACE_BUFFER_SIZE.is_power_of_two(),
    "TRACE_BUFFER_SIZE must be a power of 2"
);

// =============================================================================
// Per-CPU Ring Buffer
// =============================================================================

/// Per-CPU trace ring buffer.
///
/// This structure is cache-line aligned to prevent false sharing between CPUs.
/// Each CPU has exclusive write access to its own buffer; reads are only done
/// during post-mortem analysis or via GDB inspection.
///
/// # Thread Safety
///
/// - **Single writer**: Only the owning CPU writes to the buffer
/// - **Multiple readers**: GDB or dump functions may read (relaxed ordering OK)
/// - **No locks**: All operations use atomic primitives
#[repr(C, align(64))]
pub struct TraceCpuBuffer {
    /// Ring buffer entries (16 bytes each, 16 KiB total).
    entries: [TraceEvent; TRACE_BUFFER_SIZE],

    /// Write index (wraps around using TRACE_BUFFER_MASK).
    /// This is atomically incremented for each event.
    write_idx: AtomicUsize,

    /// Read index (only used during dump operations).
    /// Not used for normal operation; provided for analysis tools.
    read_idx: AtomicUsize,

    /// Count of dropped events (for diagnostics).
    /// In overwrite mode, this tracks how many times the buffer wrapped.
    dropped: AtomicU64,

    /// Padding to ensure 64-byte alignment of the structure.
    _padding: [u8; 24], // 8+8+8+24 = 48 bytes metadata, + 16384 entries = total aligned
}

impl TraceCpuBuffer {
    /// Create a new empty trace buffer.
    ///
    /// This is const to allow static initialization.
    pub const fn new() -> Self {
        Self {
            entries: [TraceEvent::empty(); TRACE_BUFFER_SIZE],
            write_idx: AtomicUsize::new(0),
            read_idx: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            _padding: [0; 24],
        }
    }

    /// Reset the buffer to empty state.
    ///
    /// # Safety
    ///
    /// This should only be called during initialization or when
    /// the buffer is known to not be in use.
    pub fn reset(&mut self) {
        self.write_idx.store(0, Ordering::Relaxed);
        self.read_idx.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        // Don't clear entries - they'll be overwritten naturally
    }

    /// Record a trace event to this buffer.
    ///
    /// This is a lock-free operation:
    /// 1. Atomically increment write index
    /// 2. Store event at the (old) index position
    ///
    /// If the buffer is full, old events are silently overwritten
    /// (overwrite mode, like ftrace default).
    ///
    /// # Safety
    ///
    /// This function assumes single-writer semantics (one CPU per buffer).
    /// Multiple simultaneous writers would cause race conditions.
    #[inline(always)]
    pub fn record(&mut self, event: TraceEvent) {
        // Get next index and increment atomically
        // Uses Relaxed ordering because:
        // 1. We're the only writer (single-writer assumption)
        // 2. We don't need synchronization with other operations
        let idx = self.write_idx.fetch_add(1, Ordering::Relaxed) & TRACE_BUFFER_MASK;

        // Check if we're about to overwrite (buffer has wrapped)
        // This is purely for diagnostics; we don't block
        let previous = &self.entries[idx];
        if !previous.is_empty() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }

        // Store the event
        // SAFETY: idx is bounded by TRACE_BUFFER_MASK, and we're the only writer
        self.entries[idx] = event;
    }

    /// Get the current write index.
    ///
    /// This can be called from any context (including GDB) to see
    /// how many events have been recorded.
    #[inline]
    pub fn write_index(&self) -> usize {
        self.write_idx.load(Ordering::Relaxed)
    }

    /// Get the number of dropped (overwritten) events.
    #[inline]
    #[allow(dead_code)]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Get the number of events in the buffer (up to TRACE_BUFFER_SIZE).
    #[inline]
    #[allow(dead_code)]
    pub fn event_count(&self) -> usize {
        let write = self.write_idx.load(Ordering::Relaxed);
        core::cmp::min(write, TRACE_BUFFER_SIZE)
    }

    /// Get a reference to an event at the given index.
    ///
    /// Returns None if the index is out of bounds or the slot is empty.
    ///
    /// # Note
    ///
    /// The returned event may be stale if tracing is active and the
    /// buffer has wrapped. Disable tracing before iterating for analysis.
    #[allow(dead_code)]
    pub fn get_event(&self, idx: usize) -> Option<&TraceEvent> {
        if idx >= TRACE_BUFFER_SIZE {
            return None;
        }
        let event = &self.entries[idx];
        if event.is_empty() {
            None
        } else {
            Some(event)
        }
    }

    /// Get a raw pointer to the entries array.
    ///
    /// This is provided for GDB inspection and low-level access.
    ///
    /// # Safety
    ///
    /// The caller must ensure no writes are occurring (disable tracing first).
    #[allow(dead_code)]
    pub fn entries_ptr(&self) -> *const TraceEvent {
        self.entries.as_ptr()
    }

    /// Iterate over recent events in chronological order.
    ///
    /// Returns an iterator that yields events from oldest to newest.
    /// If the buffer has wrapped, starts from the oldest non-overwritten event.
    ///
    /// # Warning
    ///
    /// Disable tracing before calling this to avoid races.
    #[allow(dead_code)]
    pub fn iter_events(&self) -> impl Iterator<Item = &TraceEvent> {
        let write = self.write_idx.load(Ordering::Relaxed);
        let count = core::cmp::min(write, TRACE_BUFFER_SIZE);
        let start = if write > TRACE_BUFFER_SIZE {
            write & TRACE_BUFFER_MASK
        } else {
            0
        };

        (0..count).map(move |i| {
            let idx = (start + i) & TRACE_BUFFER_MASK;
            &self.entries[idx]
        })
    }
}

impl Default for TraceCpuBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// Verify the buffer has the expected size
const _: () = {
    // entries: 1024 * 16 = 16384 bytes
    // write_idx: 8 bytes (AtomicUsize on 64-bit)
    // read_idx: 8 bytes
    // dropped: 8 bytes
    // _padding: 24 bytes
    // Total: 16432 bytes, rounded up to 64-byte alignment
    let expected_min = 16384 + 8 + 8 + 8 + 24; // 16432
    let actual = core::mem::size_of::<TraceCpuBuffer>();
    assert!(actual >= expected_min, "TraceCpuBuffer too small");
    // With 64-byte alignment, actual size should be a multiple of 64
    assert!(actual % 64 == 0, "TraceCpuBuffer not 64-byte aligned");
};
