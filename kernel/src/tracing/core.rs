//! Core tracing types and global state.
//!
//! This module provides the fundamental building blocks for the tracing framework:
//! - TraceEvent: 16-byte aligned event structure
//! - Global enable/disable control
//! - Event recording functions
//! - GDB-inspectable static symbols

use core::sync::atomic::{AtomicU64, Ordering};

use super::buffer::TraceCpuBuffer;
use super::timestamp::trace_timestamp;

// =============================================================================
// Configuration Constants
// =============================================================================

/// Maximum number of CPUs supported.
/// This should match MAX_CPUS in the memory layout module.
pub const MAX_CPUS: usize = 16;

// =============================================================================
// Trace Event Structure
// =============================================================================

/// A trace event stored in the ring buffer.
///
/// This structure is optimized for single atomic store operations (16 bytes, aligned).
/// The event captures:
/// - High-resolution timestamp (TSC on x86-64, CNTVCT on ARM64)
/// - Event type identifier (syscall, scheduler, interrupt, etc.)
/// - CPU ID that generated the event
/// - Flags for event-specific metadata
/// - 32-bit payload for event-specific data
///
/// # Memory Layout
///
/// ```text
/// +----------+----------+----------+----------+
/// | timestamp (8 bytes)                       |
/// +----------+----------+----------+----------+
/// | event_type | cpu_id | flags | payload    |
/// | (2 bytes)  | (1)    | (1)   | (4 bytes)  |
/// +----------+----------+----------+----------+
/// ```
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct TraceEvent {
    /// Timestamp in CPU cycles (TSC on x86-64, CNTVCT on ARM64).
    /// This provides nanosecond-resolution timing without conversion overhead.
    pub timestamp: u64,
    /// Event type identifier (see TraceEventType).
    pub event_type: u16,
    /// CPU ID that generated this event.
    pub cpu_id: u8,
    /// Event-specific flags.
    pub flags: u8,
    /// Event-specific payload (32-bit value).
    pub payload: u32,
}

impl TraceEvent {
    /// Create a new empty trace event.
    #[inline(always)]
    pub const fn empty() -> Self {
        Self {
            timestamp: 0,
            event_type: 0,
            cpu_id: 0,
            flags: 0,
            payload: 0,
        }
    }

    /// Create a new trace event with the specified parameters.
    #[inline(always)]
    pub const fn new(event_type: u16, cpu_id: u8, flags: u8, payload: u32) -> Self {
        Self {
            timestamp: 0, // Filled at record time
            event_type,
            cpu_id,
            flags,
            payload,
        }
    }

    /// Check if this event is empty (timestamp == 0).
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.timestamp == 0
    }

    /// Pack two 16-bit values into the payload.
    /// Useful for events like context switch (old_tid, new_tid).
    #[inline(always)]
    pub const fn payload_packed(val1: u16, val2: u16) -> u32 {
        ((val1 as u32) << 16) | (val2 as u32)
    }

    /// Unpack the first 16-bit value from payload.
    #[inline(always)]
    pub const fn payload_val1(&self) -> u16 {
        (self.payload >> 16) as u16
    }

    /// Unpack the second 16-bit value from payload.
    #[inline(always)]
    pub const fn payload_val2(&self) -> u16 {
        self.payload as u16
    }
}

// Verify TraceEvent is exactly 16 bytes
const _: () = assert!(
    core::mem::size_of::<TraceEvent>() == 16,
    "TraceEvent must be exactly 16 bytes"
);

// =============================================================================
// Event Type Definitions
// =============================================================================

/// Predefined event types for common kernel operations.
///
/// Event types are organized by subsystem:
/// - 0x00xx: Context switch events
/// - 0x01xx: Interrupt events
/// - 0x02xx: Scheduler events
/// - 0x03xx: Syscall events
/// - 0x04xx: Memory events
/// - 0xFFxx: Markers and debug events
pub struct TraceEventType;

impl TraceEventType {
    // Context switch events (0x00xx)
    pub const CTX_SWITCH_ENTRY: u16 = 0x0001;
    pub const CTX_SWITCH_EXIT: u16 = 0x0002;
    pub const CTX_SWITCH_TO_USER: u16 = 0x0003;
    pub const CTX_SWITCH_TO_KERNEL: u16 = 0x0004;
    pub const CTX_SWITCH_TO_IDLE: u16 = 0x0005;

    // Interrupt events (0x01xx)
    pub const IRQ_ENTRY: u16 = 0x0100;
    pub const IRQ_EXIT: u16 = 0x0101;
    pub const TIMER_TICK: u16 = 0x0102;

    // Scheduler events (0x02xx)
    pub const SCHED_PICK: u16 = 0x0200;
    pub const SCHED_RESCHED: u16 = 0x0201;
    pub const SCHED_PREEMPT: u16 = 0x0202;

    // Syscall events (0x03xx)
    pub const SYSCALL_ENTRY: u16 = 0x0300;
    pub const SYSCALL_EXIT: u16 = 0x0301;

    // Memory events (0x04xx)
    pub const PAGE_FAULT: u16 = 0x0400;
    pub const TLB_FLUSH: u16 = 0x0401;
    pub const CR3_SWITCH: u16 = 0x0402;

    // Lock events (0x05xx)
    pub const LOCK_ACQUIRE: u16 = 0x0500;
    pub const LOCK_RELEASE: u16 = 0x0501;
    pub const LOCK_CONTEND: u16 = 0x0502;

    // Debug markers (0xFFxx)
    pub const MARKER_A: u16 = 0xFF00;
    pub const MARKER_B: u16 = 0xFF01;
    pub const MARKER_C: u16 = 0xFF02;
    pub const MARKER_D: u16 = 0xFF03;
}

// =============================================================================
// Global State (GDB-Inspectable)
// =============================================================================

/// Global tracing enable flag.
/// 0 = disabled, non-zero = enabled.
///
/// GDB: `print TRACE_ENABLED`
#[no_mangle]
pub static TRACE_ENABLED: AtomicU64 = AtomicU64::new(0);

/// Per-CPU trace buffers.
/// These are statically allocated for simplicity and GDB visibility.
///
/// GDB: `x/1024xg &TRACE_CPU0_BUFFER`
#[no_mangle]
pub static mut TRACE_BUFFERS: [TraceCpuBuffer; MAX_CPUS] = {
    const INIT: TraceCpuBuffer = TraceCpuBuffer::new();
    [INIT; MAX_CPUS]
};

/// CPU 0 buffer write index (for easy GDB inspection).
/// This is a separate symbol for convenience.
///
/// GDB: `print TRACE_CPU0_WRITE_IDX`
#[no_mangle]
pub static TRACE_CPU0_WRITE_IDX: AtomicU64 = AtomicU64::new(0);

/// Flag indicating whether tracing has been initialized.
static TRACE_INITIALIZED: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the tracing subsystem.
///
/// This function should be called early in kernel initialization,
/// after per-CPU data is set up but before any tracing is needed.
///
/// # Safety
///
/// This function is safe to call multiple times; subsequent calls are no-ops.
pub fn init() {
    // Only initialize once
    if TRACE_INITIALIZED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }

    // Initialize all per-CPU buffers
    // SAFETY: We're the only writer during init, and subsequent accesses
    // are lock-free with atomic operations. Using raw pointers to avoid
    // mutable reference to static mutable (Rust 2024 deprecation).
    unsafe {
        let buffers_ptr = core::ptr::addr_of_mut!(TRACE_BUFFERS);
        for i in 0..MAX_CPUS {
            (*buffers_ptr)[i].reset();
        }
    }

    log::info!("Tracing subsystem initialized ({} per-CPU buffers)", MAX_CPUS);
}

// =============================================================================
// Enable/Disable Control
// =============================================================================

/// Enable the tracing subsystem globally.
#[inline]
pub fn enable() {
    TRACE_ENABLED.store(1, Ordering::Release);
}

/// Disable the tracing subsystem globally.
#[inline]
pub fn disable() {
    TRACE_ENABLED.store(0, Ordering::Release);
}

/// Check if tracing is currently enabled.
#[inline(always)]
pub fn is_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed) != 0
}

// =============================================================================
// Event Recording
// =============================================================================

/// Get the current CPU ID.
///
/// This function uses architecture-specific mechanisms:
/// - x86-64: GS-relative access to per-CPU data
/// - ARM64: TPIDR_EL1 or MPIDR_EL1 fallback
#[inline(always)]
#[allow(dead_code)] // Used by record_event when tracing is integrated
fn current_cpu_id() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch_impl::current::percpu::X86PerCpu;
        use crate::arch_impl::PerCpuOps;
        X86PerCpu::cpu_id() as usize
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::current::percpu::Aarch64PerCpu;
        use crate::arch_impl::PerCpuOps;
        Aarch64PerCpu::cpu_id() as usize
    }
}

/// Record a trace event to the current CPU's buffer.
///
/// This function is designed to be called from any context, including:
/// - Interrupt handlers
/// - Context switch code
/// - Syscall entry/exit
/// - Timer handlers
///
/// # Safety
///
/// This function is lock-free and safe to call from any context.
/// It performs:
/// - 1 relaxed load (enabled check)
/// - 1 timestamp read (RDTSC/CNTVCT)
/// - 1 atomic fetch_add (index increment)
/// - 1 atomic store (event write)
///
/// # Parameters
///
/// - `event_type`: Event type identifier (see TraceEventType)
/// - `flags`: Event-specific flags
/// - `payload`: Event-specific 32-bit payload
#[inline(always)]
#[allow(dead_code)] // Public API - used when tracing is integrated into subsystems
pub fn record_event(event_type: u16, flags: u8, payload: u32) {
    // Fast path: skip if tracing is disabled
    if !is_enabled() {
        return;
    }

    // Get current CPU ID and timestamp
    let cpu_id = current_cpu_id();
    let timestamp = trace_timestamp();

    // Bounds check CPU ID
    if cpu_id >= MAX_CPUS {
        return;
    }

    // Get the per-CPU buffer and record the event
    // SAFETY: cpu_id is bounds-checked, and TraceCpuBuffer uses atomic operations
    unsafe {
        TRACE_BUFFERS[cpu_id].record(TraceEvent {
            timestamp,
            event_type,
            cpu_id: cpu_id as u8,
            flags,
            payload,
        });

        // Update the convenience symbol for CPU 0
        if cpu_id == 0 {
            TRACE_CPU0_WRITE_IDX.store(
                TRACE_BUFFERS[0].write_index() as u64,
                Ordering::Relaxed,
            );
        }
    }
}

/// Record a trace event with two packed 16-bit values.
///
/// This is a convenience function for events that need two small values,
/// such as context switch (old_tid, new_tid).
///
/// # Parameters
///
/// - `event_type`: Event type identifier
/// - `val1`: First 16-bit value (stored in upper 16 bits of payload)
/// - `val2`: Second 16-bit value (stored in lower 16 bits of payload)
#[inline(always)]
#[allow(dead_code)] // Public API - used for context switch events
pub fn record_event_2(event_type: u16, val1: u16, val2: u16) {
    record_event(event_type, 0, TraceEvent::payload_packed(val1, val2));
}
