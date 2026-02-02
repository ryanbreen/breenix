//! Lock-free trace buffer for debugging ARM64 exec() hang.
//!
//! This module provides a minimal, lock-free tracing system for debugging
//! code paths where any locking or serial output would perturb timing.
//!
//! Usage:
//!   trace_exec(b'E');  // Write single byte marker
//!
//! Then inspect via GDB:
//!   x/64xb &EXEC_TRACE_BUF
//!
//! Markers used:
//!   'E' (0x45) = exec syscall handler entered
//!   'N' (0x4E) = program name parsed
//!   'O' (0x4F) = open file for ELF (from ext2)
//!   'L' (0x4C) = ELF parsing/loading started
//!   'M' (0x4D) = memory mapping done (page table created)
//!   'S' (0x53) = stack setup complete
//!   'A' (0x41) = argv setup on stack
//!   'T' (0x54) = thread context updated
//!   'P' (0x50) = page table switch (TTBR0)
//!   'F' (0x46) = exception frame setup
//!   'R' (0x52) = about to return from exec syscall
//!   'X' (0x58) = exec error path
//!   'H' (0x48) = syscall handler post-exec check
//!   'C' (0x43) = context switch check
//!   'D' (0x44) = signals delivered check
//!   'B' (0x42) = before ERET (assembly)
//!   'U' (0x55) = userspace reached (put in test binary)

use core::sync::atomic::{AtomicUsize, Ordering};

/// Trace buffer - 256 bytes at known address for GDB inspection.
/// NO locks, NO heap - just a static array.
#[no_mangle]
pub static mut EXEC_TRACE_BUF: [u8; 256] = [0; 256];

/// Current index into trace buffer.
/// Uses atomic increment for thread safety without locks.
#[no_mangle]
pub static EXEC_TRACE_IDX: AtomicUsize = AtomicUsize::new(0);

/// Write a single-byte marker to the trace buffer.
///
/// This function is:
/// - Lock-free (uses atomic increment)
/// - No allocation (uses static buffer)
/// - No serial output (just memory writes)
/// - Safe to call from any context including interrupts
///
/// # Arguments
/// * `marker` - Single byte marker (e.g., b'E' for exec entry)
#[inline(always)]
pub fn trace_exec(marker: u8) {
    let idx = EXEC_TRACE_IDX.fetch_add(1, Ordering::Relaxed);
    if idx < 256 {
        // SAFETY: We're the only writer to this index (atomic increment ensures uniqueness)
        // and buffer is 'static so always valid.
        unsafe {
            EXEC_TRACE_BUF[idx] = marker;
        }
    }
}

/// Reset the trace buffer (for starting a fresh trace).
#[inline(always)]
#[allow(dead_code)]
pub fn trace_reset() {
    EXEC_TRACE_IDX.store(0, Ordering::Relaxed);
    // Clear buffer for clean GDB output
    unsafe {
        for i in 0..256 {
            EXEC_TRACE_BUF[i] = 0;
        }
    }
}

/// Get current trace index (for diagnostics).
#[inline(always)]
#[allow(dead_code)]
pub fn trace_count() -> usize {
    EXEC_TRACE_IDX.load(Ordering::Relaxed)
}
