//! Process lifecycle trace provider.
//!
//! This provider traces process fork, exec, CoW fault, stack mapping,
//! and data abort events for debugging process management.
//!
//! # Event Types
//!
//! - `FORK_ENTRY` (0x0600): Fork started, payload = parent_pid
//! - `FORK_EXIT` (0x0601): Fork complete, payload = child_pid (0 on failure)
//! - `EXEC_ENTRY` (0x0602): Exec started, payload = pid
//! - `EXEC_EXIT` (0x0603): Exec complete, payload = pid (0 on failure)
//! - `COW_FAULT` (0x0604): CoW fault triggered, payload = packed(pid, page_idx)
//! - `COW_COPY` (0x0605): CoW page copied, payload = packed(pid, page_idx)
//! - `STACK_MAP` (0x0606): Stack mapped for process, payload = pid
//! - `DATA_ABORT` (0x0607): Data abort from EL0, payload = packed(pid, dfsc)
//! - `PROCESS_EXIT` (0x0608): Process exiting, payload = packed(pid, exit_code)
//! - `COW_LOCK_FAIL` (0x0609): CoW handler couldn't acquire manager lock, payload = pid
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::process::{PROCESS_PROVIDER, FORK_ENTRY};
//! use kernel::trace_event;
//!
//! // Enable all process tracing
//! PROCESS_PROVIDER.enable_all();
//!
//! // In fork path:
//! trace_event!(PROCESS_PROVIDER, FORK_ENTRY, parent_pid as u32);
//! ```

use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::{COW_FAULT_TOTAL, EXEC_TOTAL, FORK_TOTAL};
use core::sync::atomic::AtomicU64;

/// Provider ID for process events (0x06xx range).
pub const PROVIDER_ID: u8 = 0x06;

/// Process trace provider.
///
/// GDB: `print PROCESS_PROVIDER`
#[no_mangle]
pub static PROCESS_PROVIDER: TraceProvider = TraceProvider {
    name: "process",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Probe Definitions
// =============================================================================

/// Probe ID for fork entry.
pub const PROBE_FORK_ENTRY: u8 = 0x00;

/// Probe ID for fork exit.
pub const PROBE_FORK_EXIT: u8 = 0x01;

/// Probe ID for exec entry.
pub const PROBE_EXEC_ENTRY: u8 = 0x02;

/// Probe ID for exec exit.
pub const PROBE_EXEC_EXIT: u8 = 0x03;

/// Probe ID for CoW fault.
pub const PROBE_COW_FAULT: u8 = 0x04;

/// Probe ID for CoW page copy.
pub const PROBE_COW_COPY: u8 = 0x05;

/// Probe ID for stack mapping.
pub const PROBE_STACK_MAP: u8 = 0x06;

/// Probe ID for data abort from EL0.
pub const PROBE_DATA_ABORT: u8 = 0x07;

/// Probe ID for process exit.
pub const PROBE_PROCESS_EXIT: u8 = 0x08;

/// Probe ID for CoW lock acquisition failure.
pub const PROBE_COW_LOCK_FAIL: u8 = 0x09;

/// Event type for fork entry.
/// Payload: parent_pid.
pub const FORK_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_FORK_ENTRY as u16);

/// Event type for fork exit.
/// Payload: child_pid (0 on failure).
pub const FORK_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_FORK_EXIT as u16);

/// Event type for exec entry.
/// Payload: pid.
pub const EXEC_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_EXEC_ENTRY as u16);

/// Event type for exec exit.
/// Payload: pid (0 on failure).
pub const EXEC_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_EXEC_EXIT as u16);

/// Event type for CoW fault.
/// Payload: packed(pid, page_idx).
pub const COW_FAULT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_COW_FAULT as u16);

/// Event type for CoW page copy.
/// Payload: packed(pid, page_idx).
pub const COW_COPY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_COW_COPY as u16);

/// Event type for stack mapping.
/// Payload: pid.
pub const STACK_MAP: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_STACK_MAP as u16);

/// Event type for data abort from EL0.
/// Payload: packed(pid, dfsc).
pub const DATA_ABORT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_DATA_ABORT as u16);

/// Event type for process exit.
/// Payload: packed(pid, exit_code).
pub const PROCESS_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_PROCESS_EXIT as u16);

/// Event type for CoW lock acquisition failure.
/// Payload: pid.
pub const COW_LOCK_FAIL: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_COW_LOCK_FAIL as u16);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the process provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&PROCESS_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace fork entry (inline for minimal overhead).
///
/// Also increments the FORK_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `parent_pid`: The PID of the forking parent process
#[inline(always)]
#[allow(dead_code)]
pub fn trace_fork_entry(parent_pid: u32) {
    // Always increment the counter (single atomic add, ~3 cycles)
    FORK_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(FORK_ENTRY, 0, parent_pid);
    }
}

/// Trace fork exit (inline for minimal overhead).
///
/// # Parameters
///
/// - `child_pid`: The PID of the newly created child process (0 on failure)
#[inline(always)]
#[allow(dead_code)]
pub fn trace_fork_exit(child_pid: u32) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(FORK_EXIT, 0, child_pid);
    }
}

/// Trace exec entry (inline for minimal overhead).
///
/// Also increments the EXEC_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `pid`: The PID of the process calling exec
#[inline(always)]
#[allow(dead_code)]
pub fn trace_exec_entry(pid: u32) {
    // Always increment the counter (single atomic add, ~3 cycles)
    EXEC_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(EXEC_ENTRY, 0, pid);
    }
}

/// Trace exec exit (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the process (0 on failure)
#[inline(always)]
#[allow(dead_code)]
pub fn trace_exec_exit(pid: u32) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(EXEC_EXIT, 0, pid);
    }
}

/// Trace CoW fault (inline for minimal overhead).
///
/// Also increments the COW_FAULT_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `pid`: The PID of the faulting process
/// - `page_idx`: The page index that triggered the fault
#[inline(always)]
#[allow(dead_code)]
pub fn trace_cow_fault(pid: u16, page_idx: u16) {
    // Always increment the counter (single atomic add, ~3 cycles)
    COW_FAULT_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(COW_FAULT, pid, page_idx);
    }
}

/// Trace CoW page copy (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the process
/// - `page_idx`: The page index being copied
#[inline(always)]
#[allow(dead_code)]
pub fn trace_cow_copy(pid: u16, page_idx: u16) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(COW_COPY, pid, page_idx);
    }
}

/// Trace stack mapping (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the process whose stack was mapped
#[inline(always)]
#[allow(dead_code)]
pub fn trace_stack_map(pid: u32) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(STACK_MAP, 0, pid);
    }
}

/// Trace data abort from EL0 (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the faulting process
/// - `dfsc`: The Data Fault Status Code
#[inline(always)]
#[allow(dead_code)]
pub fn trace_data_abort(pid: u16, dfsc: u16) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(DATA_ABORT, pid, dfsc);
    }
}

/// Trace process exit (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the exiting process
/// - `exit_code`: The exit code of the process
#[inline(always)]
#[allow(dead_code)]
pub fn trace_process_exit(pid: u16, exit_code: u16) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(PROCESS_EXIT, pid, exit_code);
    }
}

/// Trace CoW lock acquisition failure (inline for minimal overhead).
///
/// # Parameters
///
/// - `pid`: The PID of the process that failed to acquire the lock
#[inline(always)]
#[allow(dead_code)]
pub fn trace_cow_lock_fail(pid: u32) {
    if PROCESS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(COW_LOCK_FAIL, 0, pid);
    }
}
