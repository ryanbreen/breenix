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
pub static SYSCALL_TOTAL: TraceCounter =
    TraceCounter::new("SYSCALL_TOTAL", "Total syscall invocations");

/// Total interrupt invocations across all CPUs.
///
/// Incremented at interrupt entry, for all interrupt types.
/// Includes timer, keyboard, disk, network, etc.
///
/// GDB: `print IRQ_TOTAL`
#[no_mangle]
pub static IRQ_TOTAL: TraceCounter = TraceCounter::new("IRQ_TOTAL", "Total interrupt invocations");

/// Total context switches across all CPUs.
///
/// Incremented when switching from one thread/process to another.
/// Does not count switches to/from idle.
///
/// GDB: `print CTX_SWITCH_TOTAL`
#[no_mangle]
pub static CTX_SWITCH_TOTAL: TraceCounter =
    TraceCounter::new("CTX_SWITCH_TOTAL", "Total context switches");

/// Total timer tick interrupts across all CPUs.
///
/// Incremented in the timer interrupt handler.
/// Useful for measuring uptime and verifying timer frequency.
///
/// GDB: `print TIMER_TICK_TOTAL`
#[no_mangle]
pub static TIMER_TICK_TOTAL: TraceCounter =
    TraceCounter::new("TIMER_TICK_TOTAL", "Total timer tick interrupts");

/// Total fork operations across all CPUs.
///
/// Incremented at fork entry, before the fork is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print FORK_TOTAL`
#[no_mangle]
pub static FORK_TOTAL: TraceCounter = TraceCounter::new("FORK_TOTAL", "Total fork operations");

/// Total exec operations across all CPUs.
///
/// Incremented at exec entry, before the exec is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print EXEC_TOTAL`
#[no_mangle]
pub static EXEC_TOTAL: TraceCounter = TraceCounter::new("EXEC_TOTAL", "Total exec operations");

/// Total CoW fault operations across all CPUs.
///
/// Incremented when a copy-on-write fault is triggered.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print COW_FAULT_TOTAL`
#[no_mangle]
pub static COW_FAULT_TOTAL: TraceCounter =
    TraceCounter::new("COW_FAULT_TOTAL", "Total CoW fault operations");

/// Total idle timer ticks across all CPUs.
///
/// Incremented in the timer interrupt handler when the CPU is running
/// its idle thread. Per-CPU utilization = 1 - (idle_ticks / timer_ticks).
///
/// GDB: `print IDLE_TICK_TOTAL`
#[no_mangle]
pub static IDLE_TICK_TOTAL: TraceCounter =
    TraceCounter::new("IDLE_TICK_TOTAL", "Total idle timer ticks");

/// GPU compositor: total bytes uploaded to VRAM.
#[no_mangle]
pub static GPU_BYTES_UPLOADED: TraceCounter =
    TraceCounter::new("GPU_BYTES_UPLOADED", "GPU bytes uploaded to VRAM");

/// GPU compositor: full-screen uploads (4.9MB each).
#[no_mangle]
pub static GPU_FULL_UPLOADS: TraceCounter =
    TraceCounter::new("GPU_FULL_UPLOADS", "Full-screen GPU uploads");

/// GPU compositor: partial rect uploads.
#[no_mangle]
pub static GPU_PARTIAL_UPLOADS: TraceCounter =
    TraceCounter::new("GPU_PARTIAL_UPLOADS", "Partial rect GPU uploads");

/// Network RX softirq polls that exhausted their packet budget.
#[no_mangle]
pub static NET_RX_BUDGET_EXHAUSTED: TraceCounter =
    TraceCounter::new("NET_RX_BUDGET_EXHAUSTED", "NetRx softirq budget exhausted");

/// VirtIO PCI net IRQs that raised NetRx softirq work.
#[no_mangle]
pub static NET_PCI_IRQ_RAISED_NETRX: TraceCounter =
    TraceCounter::new("NET_PCI_IRQ_RAISED_NETRX", "PCI net IRQ raised NetRx");

/// GIC acknowledgements for VirtIO-net's GICv2m MSI-X SPI 55.
#[no_mangle]
pub static GIC_SPI55_ACK_TOTAL: TraceCounter =
    TraceCounter::new("GIC_SPI55_ACK_TOTAL", "GIC acknowledged SPI 55");

// =============================================================================
// Socket recv wait-loop wasted-turn counters (#772 instrumentation)
// =============================================================================

/// TCP recv wait loop (`sys_read`, `FdKind::Socket` blocking path in
/// `kernel/src/syscall/handlers.rs`) observed `ThreadState::Blocked` after a
/// dispatch + `yield_current()` + halt cycle and went back to sleep without
/// making progress.
///
/// This is the direct signal for issue #772's "wasted turn": the reader was
/// scheduled, ran the loop body, and found itself still `Blocked` even though
/// it had been given a CPU turn. A nonzero count here, correlated with
/// `unblock()` having already run `set_ready()` for the same thread before
/// this check executed, supports RCA reading (a) (the loop observes a state
/// that is stale relative to `unblock()`). Per-CPU aggregate only — see
/// `RECV_WAIT_STILL_BLOCKED_FALSE` doc comment for the per-tid caveat.
///
/// GDB: `print RECV_WAIT_STILL_BLOCKED_TRUE`
#[no_mangle]
pub static RECV_WAIT_STILL_BLOCKED_TRUE: TraceCounter = TraceCounter::new(
    "RECV_WAIT_STILL_BLOCKED_TRUE",
    "recv wait loop observed Blocked, looped back to sleep (#772)",
);

/// TCP recv wait loop observed a state other than `ThreadState::Blocked`
/// (i.e. `unblock()`'s `set_ready()` was visible to this thread) and
/// proceeded to clear `blocked_in_syscall` and return data to the caller.
///
/// Comparing this counter's growth against `RECV_WAIT_STILL_BLOCKED_TRUE`
/// and against `CTX_SWITCH_TOTAL` / `SCHED_PICK` occurrences for the same
/// thread distinguishes RCA reading (a) — the loop re-observes Blocked one
/// or more times before this fires — from reading (b) — the thread is
/// dispatched but this check (and everything else in the loop body) does
/// not run before being switched out again, so that turn increments
/// neither branch of this pair.
///
/// Per-tid tagging: `TraceCounter` (`kernel/src/tracing/counter.rs`) only
/// carries a per-CPU dimension (`per_cpu: [CpuCounterSlot; MAX_CPUS]`); there
/// is no per-tid slot in the counter primitive, and adding one would require
/// either a lock or a fixed-size tid-indexed array allocated statically (the
/// same pattern the scheduler's separate, non-tracing-framework
/// `WAKE_LAST_READY_SITE: [AtomicU64; WAKE_ATTRIB_MAX_TIDS]` in
/// `kernel/src/task/scheduler.rs` already uses for wake-site attribution).
/// That mechanism exists but is outside `kernel/src/tracing/`'s counter
/// registration, so these two counters stay global per-CPU aggregates as
/// instructed when no cheap per-tid tag is available in the framework itself.
///
/// GDB: `print RECV_WAIT_STILL_BLOCKED_FALSE`
#[no_mangle]
pub static RECV_WAIT_STILL_BLOCKED_FALSE: TraceCounter = TraceCounter::new(
    "RECV_WAIT_STILL_BLOCKED_FALSE",
    "recv wait loop observed not-Blocked, proceeded (#772)",
);

// =============================================================================
// Boot Test Counters (BTRT feature)
// =============================================================================

/// Total boot tests recorded.
///
/// GDB: `print BOOT_TEST_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_TOTAL", "Total boot tests recorded");

/// Total boot tests passed.
///
/// GDB: `print BOOT_TEST_PASS_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_PASS_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_PASS_TOTAL", "Total boot tests passed");

/// Total boot tests failed.
///
/// GDB: `print BOOT_TEST_FAIL_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_FAIL_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_FAIL_TOTAL", "Total boot tests failed");

/// Total boot tests skipped.
///
/// GDB: `print BOOT_TEST_SKIP_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_SKIP_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_SKIP_TOTAL", "Total boot tests skipped");

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
    register_counter(&IDLE_TICK_TOTAL);
    register_counter(&GPU_BYTES_UPLOADED);
    register_counter(&GPU_FULL_UPLOADS);
    register_counter(&GPU_PARTIAL_UPLOADS);
    register_counter(&NET_RX_BUDGET_EXHAUSTED);
    register_counter(&NET_PCI_IRQ_RAISED_NETRX);
    register_counter(&GIC_SPI55_ACK_TOTAL);
    register_counter(&RECV_WAIT_STILL_BLOCKED_TRUE);
    register_counter(&RECV_WAIT_STILL_BLOCKED_FALSE);

    #[cfg(feature = "btrt")]
    {
        register_counter(&BOOT_TEST_TOTAL);
        register_counter(&BOOT_TEST_PASS_TOTAL);
        register_counter(&BOOT_TEST_FAIL_TOTAL);
        register_counter(&BOOT_TEST_SKIP_TOTAL);
    }

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

/// Increment the GIC SPI 55 acknowledgement counter.
#[inline(always)]
pub fn count_gic_spi55_ack() {
    crate::trace_count!(GIC_SPI55_ACK_TOTAL);
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
