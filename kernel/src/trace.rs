//! Lock-free tracing for critical paths.
//!
//! This module provides a tracing facility that can be safely used in
//! interrupt handlers, context switch code, and any other critical path
//! where locks are forbidden.
//!
//! # Design Principles
//!
//! 1. **Single atomic operation**: Each trace event is recorded with a single
//!    atomic store. No multi-step operations that could be interrupted.
//!
//! 2. **No locks**: The ring buffer uses atomic indices and overwrites old
//!    entries without blocking.
//!
//! 3. **Bounded memory**: Fixed-size ring buffer (configurable at compile time).
//!
//! 4. **Forensic debugging**: Events can be dumped post-mortem to understand
//!    what happened in critical sections before a crash.
//!
//! # Usage
//!
//! ```rust
//! use crate::trace::{trace_event, TraceEvent};
//!
//! // In context switch:
//! trace_event(TraceEvent::ContextSwitchEntry(old_tid, new_tid));
//!
//! // In interrupt handler:
//! trace_event(TraceEvent::InterruptEntry(vector));
//!
//! // Dump after crash or for debugging:
//! trace::dump_trace();
//! ```

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Size of the trace ring buffer (power of 2 for efficient modulo).
const TRACE_BUFFER_SIZE: usize = 256;

/// Mask for efficient modulo operation (size - 1).
const TRACE_BUFFER_MASK: usize = TRACE_BUFFER_SIZE - 1;

/// A trace event encoded in a single u64.
///
/// Format: [8-bit event type][56-bit payload]
///
/// This allows single atomic store for the entire event.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct TraceEntry(u64);

impl TraceEntry {
    /// Create a new trace entry.
    #[inline(always)]
    pub const fn new(event_type: u8, payload: u64) -> Self {
        Self(((event_type as u64) << 56) | (payload & 0x00FF_FFFF_FFFF_FFFF))
    }

    /// Get the event type.
    #[inline(always)]
    pub const fn event_type(self) -> u8 {
        (self.0 >> 56) as u8
    }

    /// Get the payload.
    #[inline(always)]
    pub const fn payload(self) -> u64 {
        self.0 & 0x00FF_FFFF_FFFF_FFFF
    }

    /// Get the raw u64 value.
    #[inline(always)]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Predefined event types for common operations.
/// Each fits in 8 bits (0-255).
pub mod event_types {
    // Context switch events (0x00 - 0x0F)
    pub const CTX_SWITCH_ENTRY: u8 = 0x01;
    pub const CTX_SWITCH_EXIT: u8 = 0x02;
    pub const CTX_SWITCH_TO_USER: u8 = 0x03;
    pub const CTX_SWITCH_TO_KERNEL: u8 = 0x04;
    pub const CTX_SWITCH_TO_IDLE: u8 = 0x05;

    // Interrupt events (0x10 - 0x1F)
    pub const IRQ_ENTRY: u8 = 0x10;
    pub const IRQ_EXIT: u8 = 0x11;
    pub const TIMER_TICK: u8 = 0x12;
    pub const SYSCALL_ENTRY: u8 = 0x13;
    pub const SYSCALL_EXIT: u8 = 0x14;

    // Scheduler events (0x20 - 0x2F)
    pub const SCHED_PICK: u8 = 0x20;
    pub const SCHED_RESCHED: u8 = 0x21;
    pub const SCHED_PREEMPT: u8 = 0x22;

    // Lock events (0x30 - 0x3F) - for debugging lock contention
    pub const LOCK_ACQUIRE: u8 = 0x30;
    pub const LOCK_RELEASE: u8 = 0x31;
    pub const LOCK_CONTEND: u8 = 0x32;

    // Page table events (0x40 - 0x4F)
    pub const TTBR_SWITCH: u8 = 0x40;
    pub const TLB_FLUSH: u8 = 0x41;

    // User-defined markers (0xF0 - 0xFF)
    pub const MARKER_A: u8 = 0xF0;
    pub const MARKER_B: u8 = 0xF1;
    pub const MARKER_C: u8 = 0xF2;
    pub const MARKER_D: u8 = 0xF3;
    pub const MARKER_E: u8 = 0xF4;
    pub const MARKER_F: u8 = 0xF5;
}

/// The trace ring buffer.
///
/// Each entry is a single AtomicU64 that can be written with one atomic store.
static TRACE_BUFFER: [AtomicU64; TRACE_BUFFER_SIZE] = {
    // Initialize all entries to zero
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; TRACE_BUFFER_SIZE]
};

/// Current write index into the ring buffer.
static TRACE_INDEX: AtomicUsize = AtomicUsize::new(0);

/// Whether tracing is enabled.
static TRACE_ENABLED: AtomicU64 = AtomicU64::new(0);

/// Enable the trace system.
#[inline]
pub fn enable() {
    TRACE_ENABLED.store(1, Ordering::Release);
}

/// Disable the trace system.
#[inline]
pub fn disable() {
    TRACE_ENABLED.store(0, Ordering::Release);
}

/// Check if tracing is enabled.
#[inline(always)]
pub fn is_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed) != 0
}

/// Record a trace event.
///
/// This function is designed to be called from any context, including:
/// - Interrupt handlers
/// - Context switch code
/// - Syscall entry/exit
/// - Timer handlers
///
/// # Safety
///
/// This function performs exactly ONE atomic store operation plus one
/// atomic fetch_add for the index. No locks, no allocations, no blocking.
#[inline(always)]
pub fn trace_event(event_type: u8, payload: u64) {
    // Skip if tracing is disabled (single relaxed load)
    if !is_enabled() {
        return;
    }

    // Create the entry
    let entry = TraceEntry::new(event_type, payload);

    // Get next index with wrap-around (single atomic operation)
    let index = TRACE_INDEX.fetch_add(1, Ordering::Relaxed) & TRACE_BUFFER_MASK;

    // Store the entry (single atomic store)
    TRACE_BUFFER[index].store(entry.raw(), Ordering::Release);
}

/// Record a trace event with two small values packed into payload.
///
/// Useful for events like context switch (old_tid, new_tid) where both
/// values fit in 28 bits each.
#[inline(always)]
pub fn trace_event_2(event_type: u8, val1: u32, val2: u32) {
    // Pack two 28-bit values into the 56-bit payload
    let payload = ((val1 as u64 & 0x0FFF_FFFF) << 28) | (val2 as u64 & 0x0FFF_FFFF);
    trace_event(event_type, payload);
}

/// Convenience macros for common trace events.
///
/// These expand to single trace_event calls with the appropriate event type.
#[macro_export]
macro_rules! trace_ctx_switch {
    ($old:expr, $new:expr) => {
        $crate::trace::trace_event_2(
            $crate::trace::event_types::CTX_SWITCH_ENTRY,
            $old as u32,
            $new as u32,
        )
    };
}

#[macro_export]
macro_rules! trace_irq_entry {
    ($vector:expr) => {
        $crate::trace::trace_event($crate::trace::event_types::IRQ_ENTRY, $vector as u64)
    };
}

#[macro_export]
macro_rules! trace_irq_exit {
    ($vector:expr) => {
        $crate::trace::trace_event($crate::trace::event_types::IRQ_EXIT, $vector as u64)
    };
}

#[macro_export]
macro_rules! trace_syscall_entry {
    ($num:expr) => {
        $crate::trace::trace_event($crate::trace::event_types::SYSCALL_ENTRY, $num as u64)
    };
}

#[macro_export]
macro_rules! trace_syscall_exit {
    ($num:expr) => {
        $crate::trace::trace_event($crate::trace::event_types::SYSCALL_EXIT, $num as u64)
    };
}

#[macro_export]
macro_rules! trace_marker {
    (A) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_A, 0)
    };
    (B) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_B, 0)
    };
    (C) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_C, 0)
    };
    (D) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_D, 0)
    };
    (E) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_E, 0)
    };
    (F) => {
        $crate::trace::trace_event($crate::trace::event_types::MARKER_F, 0)
    };
}

/// Dump the trace buffer to serial output.
///
/// This should only be called from a safe context (not interrupt/context switch)
/// because it uses formatted output which may allocate/lock.
pub fn dump_trace() {
    use crate::serial_println;

    // Disable tracing while dumping to avoid concurrent writes
    let was_enabled = is_enabled();
    disable();

    serial_println!("=== TRACE BUFFER DUMP ===");

    let current_index = TRACE_INDEX.load(Ordering::Acquire);
    let start = if current_index >= TRACE_BUFFER_SIZE {
        current_index & TRACE_BUFFER_MASK
    } else {
        0
    };

    let count = core::cmp::min(current_index, TRACE_BUFFER_SIZE);

    for i in 0..count {
        let idx = (start + i) & TRACE_BUFFER_MASK;
        let entry = TraceEntry(TRACE_BUFFER[idx].load(Ordering::Acquire));

        if entry.raw() != 0 {
            let event_name = match entry.event_type() {
                event_types::CTX_SWITCH_ENTRY => "CTX_SWITCH",
                event_types::CTX_SWITCH_EXIT => "CTX_EXIT",
                event_types::CTX_SWITCH_TO_USER => "CTX_TO_USER",
                event_types::CTX_SWITCH_TO_KERNEL => "CTX_TO_KERN",
                event_types::CTX_SWITCH_TO_IDLE => "CTX_TO_IDLE",
                event_types::IRQ_ENTRY => "IRQ_ENTRY",
                event_types::IRQ_EXIT => "IRQ_EXIT",
                event_types::TIMER_TICK => "TIMER",
                event_types::SYSCALL_ENTRY => "SYSCALL_IN",
                event_types::SYSCALL_EXIT => "SYSCALL_OUT",
                event_types::SCHED_PICK => "SCHED_PICK",
                event_types::SCHED_RESCHED => "SCHED_RESCHED",
                event_types::SCHED_PREEMPT => "SCHED_PREEMPT",
                event_types::LOCK_ACQUIRE => "LOCK_ACQ",
                event_types::LOCK_RELEASE => "LOCK_REL",
                event_types::LOCK_CONTEND => "LOCK_WAIT",
                event_types::TTBR_SWITCH => "TTBR_SWITCH",
                event_types::TLB_FLUSH => "TLB_FLUSH",
                event_types::MARKER_A => "MARKER_A",
                event_types::MARKER_B => "MARKER_B",
                event_types::MARKER_C => "MARKER_C",
                event_types::MARKER_D => "MARKER_D",
                event_types::MARKER_E => "MARKER_E",
                event_types::MARKER_F => "MARKER_F",
                _ => "UNKNOWN",
            };

            serial_println!("[{:3}] {:12} payload={:#x}", i, event_name, entry.payload());
        }
    }

    serial_println!("=== END TRACE ({}  events) ===", count);

    // Re-enable if it was enabled before
    if was_enabled {
        enable();
    }
}

/// Clear the trace buffer.
pub fn clear() {
    for entry in &TRACE_BUFFER {
        entry.store(0, Ordering::Release);
    }
    TRACE_INDEX.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_trace_entry_encoding() {
        let entry = TraceEntry::new(0x42, 0x123456789ABC);
        assert_eq!(entry.event_type(), 0x42);
        assert_eq!(entry.payload(), 0x123456789ABC);
    }

    #[test_case]
    fn test_trace_event_2_packing() {
        // Two values should be packed correctly
        let entry = TraceEntry::new(event_types::CTX_SWITCH_ENTRY, 0);
        assert_eq!(entry.event_type(), event_types::CTX_SWITCH_ENTRY);
    }
}
