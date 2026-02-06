//! Built-in trace counters for kernel statistics.
//!
//! This module defines the core kernel counters that track fundamental
//! operations like syscalls, interrupts, and context switches.
//!
//! # Available Counters
//!
//! - `SYSCALL_TOTAL`: Total syscall invocations across all CPUs
//! - `IRQ_TOTAL`: Total interrupt invocations
//! - `CTX_SWITCH_TOTAL`: Total context switches
//! - `TIMER_TICK_TOTAL`: Total timer tick interrupts
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::counters::{SYSCALL_TOTAL, trace_count};
//!
//! // Increment in hot path (compiles to single atomic add)
//! trace_count!(SYSCALL_TOTAL);
//!
//! // Query aggregated value
//! let total = SYSCALL_TOTAL.aggregate();
//! ```
//!
//! # GDB Inspection
//!
//! ```gdb
//! # View all counter values
//! print SYSCALL_TOTAL
//! print SYSCALL_TOTAL.per_cpu[0].value
//!
//! # View aggregated total (requires helper script)
//! # Or manually sum: p SYSCALL_TOTAL.per_cpu[0].value + SYSCALL_TOTAL.per_cpu[1].value + ...
//! ```

use crate::tracing::counter::{register_counter, TraceCounter};

// =============================================================================
// Built-in Counter Definitions
// =============================================================================

/// Total syscall invocations across all CPUs.
///
/// Incremented at syscall entry, before dispatching to the handler.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print SYSCALL_TOTAL`
#[no_mangle]
pub static SYSCALL_TOTAL: TraceCounter = TraceCounter::new(
    "SYSCALL_TOTAL",
    "Total syscall invocations",
);

/// Total interrupt invocations across all CPUs.
///
/// Incremented at interrupt entry, for all interrupt types.
/// Includes timer, keyboard, disk, network, etc.
///
/// GDB: `print IRQ_TOTAL`
#[no_mangle]
pub static IRQ_TOTAL: TraceCounter = TraceCounter::new(
    "IRQ_TOTAL",
    "Total interrupt invocations",
);

/// Total context switches across all CPUs.
///
/// Incremented when switching from one thread/process to another.
/// Does not count switches to/from idle.
///
/// GDB: `print CTX_SWITCH_TOTAL`
#[no_mangle]
pub static CTX_SWITCH_TOTAL: TraceCounter = TraceCounter::new(
    "CTX_SWITCH_TOTAL",
    "Total context switches",
);

/// Total timer tick interrupts across all CPUs.
///
/// Incremented in the timer interrupt handler.
/// Useful for measuring uptime and verifying timer frequency.
///
/// GDB: `print TIMER_TICK_TOTAL`
#[no_mangle]
pub static TIMER_TICK_TOTAL: TraceCounter = TraceCounter::new(
    "TIMER_TICK_TOTAL",
    "Total timer tick interrupts",
);

/// Total fork operations across all CPUs.
///
/// Incremented at fork entry, before the fork is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print FORK_TOTAL`
#[no_mangle]
pub static FORK_TOTAL: TraceCounter = TraceCounter::new(
    "FORK_TOTAL",
    "Total fork operations",
);

/// Total exec operations across all CPUs.
///
/// Incremented at exec entry, before the exec is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print EXEC_TOTAL`
#[no_mangle]
pub static EXEC_TOTAL: TraceCounter = TraceCounter::new(
    "EXEC_TOTAL",
    "Total exec operations",
);

/// Total CoW fault operations across all CPUs.
///
/// Incremented when a copy-on-write fault is triggered.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print COW_FAULT_TOTAL`
#[no_mangle]
pub static COW_FAULT_TOTAL: TraceCounter = TraceCounter::new(
    "COW_FAULT_TOTAL",
    "Total CoW fault operations",
);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize all built-in counters.
///
/// Registers counters with the global registry for enumeration and lookup.
pub fn init() {
    register_counter(&SYSCALL_TOTAL);
    register_counter(&IRQ_TOTAL);
    register_counter(&CTX_SWITCH_TOTAL);
    register_counter(&TIMER_TICK_TOTAL);
    register_counter(&FORK_TOTAL);
    register_counter(&EXEC_TOTAL);
    register_counter(&COW_FAULT_TOTAL);

    log::info!(
        "Tracing counters initialized: SYSCALL_TOTAL, IRQ_TOTAL, CTX_SWITCH_TOTAL, TIMER_TICK_TOTAL, FORK_TOTAL, EXEC_TOTAL, COW_FAULT_TOTAL"
    );
}

// =============================================================================
// Convenience Functions
// =============================================================================

/// Increment the syscall counter.
///
/// This is an inline function for use in the syscall hot path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_syscall() {
    SYSCALL_TOTAL.increment();
}

/// Increment the interrupt counter.
///
/// This is an inline function for use in interrupt handlers.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_irq() {
    IRQ_TOTAL.increment();
}

/// Increment the context switch counter.
///
/// This is an inline function for use in the scheduler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_ctx_switch() {
    CTX_SWITCH_TOTAL.increment();
}

/// Increment the timer tick counter.
///
/// This is an inline function for use in the timer handler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_timer_tick() {
    TIMER_TICK_TOTAL.increment();
}

/// Increment the fork counter.
///
/// This is an inline function for use in the fork path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_fork() {
    FORK_TOTAL.increment();
}

/// Increment the exec counter.
///
/// This is an inline function for use in the exec path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_exec() {
    EXEC_TOTAL.increment();
}

/// Increment the CoW fault counter.
///
/// This is an inline function for use in the CoW fault handler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_cow_fault() {
    COW_FAULT_TOTAL.increment();
}

/// Get all counter values as a summary.
///
/// Returns a tuple of (syscall_total, irq_total, ctx_switch_total, timer_tick_total).
pub fn get_all_counters() -> (u64, u64, u64, u64) {
    (
        SYSCALL_TOTAL.aggregate(),
        IRQ_TOTAL.aggregate(),
        CTX_SWITCH_TOTAL.aggregate(),
        TIMER_TICK_TOTAL.aggregate(),
    )
}

/// Get process-related counter values.
///
/// Returns a tuple of (fork_total, exec_total, cow_fault_total).
pub fn get_process_counters() -> (u64, u64, u64) {
    (
        FORK_TOTAL.aggregate(),
        EXEC_TOTAL.aggregate(),
        COW_FAULT_TOTAL.aggregate(),
    )
}

/// Reset all built-in counters to zero.
///
/// This is not atomic across counters - some increments may be
/// recorded between individual counter resets.
pub fn reset_all() {
    SYSCALL_TOTAL.reset();
    IRQ_TOTAL.reset();
    CTX_SWITCH_TOTAL.reset();
    TIMER_TICK_TOTAL.reset();
    FORK_TOTAL.reset();
    EXEC_TOTAL.reset();
    COW_FAULT_TOTAL.reset();
}
