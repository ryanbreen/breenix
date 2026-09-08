//! Softirq (Software Interrupt) Subsystem
//!
//! Softirqs are deferred work that runs in interrupt context but outside
//! the hardware interrupt handler. They provide a way to defer time-consuming
//! work from hardware interrupt handlers while still running with high priority.
//!
//! When softirq load is too high (> MAX_SOFTIRQ_RESTART iterations), work is
//! deferred to per-CPU ksoftirqd kernel threads to prevent userspace starvation.
//!
//! This implementation follows Linux's softirq architecture:
//! - Fixed set of softirq types (not dynamically registered)
//! - Per-CPU softirq pending bitmap (in per_cpu.rs)
//! - Per-CPU ksoftirqd threads for overflow processing
//! - Iteration limit in do_softirq() to prevent softirq storms

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

#[cfg(target_arch = "x86_64")]
use crate::arch_impl::PerCpuOps;
use spin::Once;

use super::kthread::{
    kthread_park_if, kthread_run_on_cpu, kthread_should_stop, kthread_unpark, KthreadHandle,
};
use crate::per_cpu;

/// Architecture-specific enable interrupts
#[inline(always)]
unsafe fn arch_enable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    x86_64::instructions::interrupts::enable();

    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("msr daifclr, #3", options(nomem, nostack));
}

/// Maximum number of softirq restarts before deferring to ksoftirqd
/// Linux uses 10, we match that
const MAX_SOFTIRQ_RESTART: u32 = 10;

/// Maximum number of softirq types (matches Linux)
pub const NR_SOFTIRQS: usize = 10;

/// Softirq types - fixed set matching Linux priorities
/// Lower numbers = higher priority (processed first)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SoftirqType {
    /// High priority tasklets (rarely used)
    HiTasklet = 0,
    /// Timer callbacks (hrtimers, etc.)
    Timer = 1,
    /// Network transmit completion
    NetTx = 2,
    /// Network receive processing
    NetRx = 3,
    /// Block device completion
    Block = 4,
    /// IRQ polling
    IrqPoll = 5,
    /// Normal tasklets
    Tasklet = 6,
    /// Scheduler load balancing
    Sched = 7,
    /// High-resolution timer callbacks
    Hrtimer = 8,
    /// Read-copy-update callbacks
    Rcu = 9,
}

impl SoftirqType {
    /// Convert from u32 bit position to SoftirqType
    pub fn from_nr(nr: u32) -> Option<Self> {
        match nr {
            0 => Some(SoftirqType::HiTasklet),
            1 => Some(SoftirqType::Timer),
            2 => Some(SoftirqType::NetTx),
            3 => Some(SoftirqType::NetRx),
            4 => Some(SoftirqType::Block),
            5 => Some(SoftirqType::IrqPoll),
            6 => Some(SoftirqType::Tasklet),
            7 => Some(SoftirqType::Sched),
            8 => Some(SoftirqType::Hrtimer),
            9 => Some(SoftirqType::Rcu),
            _ => None,
        }
    }

    /// Get the bit position for this softirq type
    pub fn as_nr(&self) -> u32 {
        *self as u32
    }
}

/// Softirq handler function type
/// Handler receives the softirq type that triggered it
pub type SoftirqHandler = fn(SoftirqType);

/// Static array of softirq handlers
/// Index corresponds to SoftirqType ordinal
static SOFTIRQ_HANDLERS: [AtomicPtr<()>; NR_SOFTIRQS] = [
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
];

// Static ownership keeps handles valid for lock-free IRQ readers.
// CPU hot-unplug is not implemented; these daemons live for the kernel lifetime.
static KSOFTIRQD: [Once<KthreadHandle>; 8] = [const { Once::new() }; 8];

pub(crate) fn current_cpu() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch_impl::x86_64::percpu::X86PerCpu::cpu_id() as usize
    }
}

#[cfg(any(feature = "testing", feature = "boot_tests"))]
pub(crate) static KSOFTIRQD_TASKLET_DISPATCHES: crate::tracing::counter::TraceCounter =
    crate::tracing::counter::TraceCounter::new(
        "KSOFTIRQD_TASKLET_DISPATCHES",
        "Daemon tasklet callbacks",
    );

fn local_handle() -> Option<&'static KthreadHandle> {
    KSOFTIRQD.get(current_cpu())?.get()
}

/// Flag indicating softirq system is initialized
static SOFTIRQ_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Get the ksoftirqd thread ID (if initialized)
/// Returns None if softirq system is not initialized
#[allow(dead_code)] // Part of public softirq API surface
pub fn ksoftirqd_tid() -> Option<u64> {
    local_handle().map(|h| h.tid())
}

/// Register a softirq handler
///
/// # Safety
/// Handler must be safe to call from interrupt context (cannot sleep/block)
pub fn register_softirq_handler(softirq: SoftirqType, handler: SoftirqHandler) {
    let nr = softirq.as_nr() as usize;
    if nr >= NR_SOFTIRQS {
        log::error!("Invalid softirq number: {}", nr);
        return;
    }

    // Store handler as function pointer
    SOFTIRQ_HANDLERS[nr].store(handler as *mut (), Ordering::Release);
    log::info!("SOFTIRQ_REGISTER: {:?} handler registered", softirq);
}

/// Raise a softirq (mark it as pending)
/// This is typically called from hardware interrupt handlers
pub fn raise_softirq(softirq: SoftirqType) {
    per_cpu::raise_softirq(softirq.as_nr());
}

/// Raise a softirq by number
#[allow(dead_code)] // Part of public API for interrupt handlers
pub fn raise_softirq_nr(nr: u32) {
    if nr < NR_SOFTIRQS as u32 {
        per_cpu::raise_softirq(nr);
    }
}

/// Check if a specific softirq is pending
#[allow(dead_code)] // Part of public API for subsystems to query softirq state
pub fn softirq_pending(softirq: SoftirqType) -> bool {
    let pending = per_cpu::softirq_pending();
    (pending & (1 << softirq.as_nr())) != 0
}

/// Process pending softirqs with iteration limit
///
/// Called from irq_exit() when returning to non-interrupt context.
/// If too many softirqs fire (> MAX_SOFTIRQ_RESTART), defers remaining
/// work to ksoftirqd to prevent userspace starvation.
///
/// Returns true if softirqs were processed, false if deferred to ksoftirqd
pub fn do_softirq() -> bool {
    // Don't process softirqs if we're in interrupt context (nested)
    if per_cpu::in_interrupt() || per_cpu::in_softirq() {
        return false;
    }

    let mut restart_count: u32 = 0;

    loop {
        // Get pending bitmap and exit if none pending
        let pending = per_cpu::softirq_pending();
        if pending == 0 {
            return restart_count > 0;
        }

        // Enter softirq context
        per_cpu::softirq_enter();

        // Process all pending softirqs in priority order (lowest bit first)
        let mut pending_work = pending;
        while pending_work != 0 {
            let nr = pending_work.trailing_zeros();
            if nr >= NR_SOFTIRQS as u32 {
                break;
            }

            // Clear the pending bit BEFORE calling handler
            // This allows handler to re-raise itself if needed
            per_cpu::clear_softirq(nr);

            // Call the handler if registered
            let handler_ptr = SOFTIRQ_HANDLERS[nr as usize].load(Ordering::Acquire);
            if !handler_ptr.is_null() {
                let handler: SoftirqHandler = unsafe { core::mem::transmute(handler_ptr) };
                if let Some(softirq_type) = SoftirqType::from_nr(nr) {
                    handler(softirq_type);
                }
            }

            // Clear bit from local pending copy
            pending_work &= !(1 << nr);
        }

        // Exit softirq context
        per_cpu::softirq_exit();

        // Check iteration limit
        restart_count += 1;
        if restart_count >= MAX_SOFTIRQ_RESTART {
            // Too many iterations - defer to ksoftirqd
            if per_cpu::softirq_pending() != 0 {
                wakeup_ksoftirqd();
            }
            return true;
        }

        // Check if more softirqs were raised during processing
        if per_cpu::softirq_pending() == 0 {
            break;
        }
    }

    restart_count > 0
}

/// Wake up ksoftirqd to process remaining softirqs
fn wakeup_ksoftirqd() {
    if let Some(handle) = local_handle() {
        kthread_unpark(handle);
    }
}

/// ksoftirqd kernel thread function
/// Processes softirqs that were deferred due to high load
fn ksoftirqd_fn() {
    // Enable interrupts so timer can preempt us and switch to other threads
    unsafe {
        arch_enable_interrupts();
    }

    while !kthread_should_stop() {
        // Check for pending softirqs
        let pending = per_cpu::softirq_pending();

        if pending != 0 {
            // Process softirqs (no iteration limit in thread context)
            // We still use softirq_enter/exit for proper context tracking
            per_cpu::softirq_enter();

            let mut work = pending;
            while work != 0 {
                let nr = work.trailing_zeros();
                if nr >= NR_SOFTIRQS as u32 {
                    break;
                }

                per_cpu::clear_softirq(nr);

                let handler_ptr = SOFTIRQ_HANDLERS[nr as usize].load(Ordering::Acquire);
                if !handler_ptr.is_null() {
                    let handler: SoftirqHandler = unsafe { core::mem::transmute(handler_ptr) };
                    if let Some(softirq_type) = SoftirqType::from_nr(nr) {
                        #[cfg(any(feature = "testing", feature = "boot_tests"))]
                        if softirq_type == SoftirqType::Tasklet {
                            KSOFTIRQD_TASKLET_DISPATCHES.increment();
                        }
                        handler(softirq_type);
                    }
                }

                work &= !(1 << nr);
            }

            per_cpu::softirq_exit();

            // After processing, check if we should continue or park
            // This handles handlers that re-raised softirqs
        } else {
            // No work - park until woken by wakeup_ksoftirqd()
            kthread_park_if(|| per_cpu::softirq_pending() == 0);
        }
    }
}

/// Initialize the softirq subsystem
/// Must be called after kthread infrastructure is ready
pub fn init_softirq() {
    if SOFTIRQ_INITIALIZED.swap(true, Ordering::AcqRel) {
        log::warn!("Softirq system already initialized");
        return;
    }

    log::info!("SOFTIRQ_INIT: Initializing softirq subsystem");

    init_online_daemons();

    log::info!("SOFTIRQ_INIT: Softirq subsystem initialized");
}

/// Called by the boot CPU at initial setup and after secondary CPU bringup.
/// The boot coordinator serializes these calls; published slots are retained.
pub fn init_online_daemons() {
    #[cfg(target_arch = "aarch64")]
    let online = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    #[cfg(target_arch = "x86_64")]
    let online = 1;
    for cpu in 0..online.min(KSOFTIRQD.len()) {
        // CPU0's boot stack cannot dispatch pinned workers until init handoff.
        // Publishing it earlier strands a Ready thread through the boot tests.
        #[cfg(target_arch = "aarch64")]
        if cpu == 0 && crate::per_cpu_aarch64::preempt_count() != 0 {
            continue;
        }
        if KSOFTIRQD[cpu].get().is_some() {
            continue;
        }
        let handle = KSOFTIRQD[cpu].call_once(|| {
            let name = alloc::format!("ksoftirqd/{cpu}");
            kthread_run_on_cpu(ksoftirqd_fn, &name, cpu)
                .expect("could not spawn CPU-local ksoftirqd")
        });
        // The worker may have parked before its handle became visible to IRQs.
        // Wake once after publication to cover work raised in that interval.
        kthread_unpark(handle);
    }
}

/// Check if softirq system is initialized
pub fn is_initialized() -> bool {
    SOFTIRQ_INITIALIZED.load(Ordering::Acquire)
}

// ============================================================================
// Default softirq handlers (stubs that can be replaced)
// These are intentionally available for subsystems to use but may not be
// registered yet. They provide a template for common softirq handlers.
// ============================================================================

/// Default timer softirq handler
#[allow(dead_code)]
pub fn timer_softirq_handler(_softirq: SoftirqType) {
    // Timer callbacks would be processed here
    // For now this is a stub - timer subsystem will register real handler
}

/// Default network RX softirq handler
#[allow(dead_code)]
pub fn net_rx_softirq_handler(_softirq: SoftirqType) {
    // Network packet processing would happen here
    // Network stack will register real handler
}

/// Default network TX softirq handler
#[allow(dead_code)]
pub fn net_tx_softirq_handler(_softirq: SoftirqType) {
    // Network transmit completion would happen here
    // Network stack will register real handler
}

/// Default tasklet softirq handler
#[allow(dead_code)]
pub fn tasklet_softirq_handler(_softirq: SoftirqType) {
    // Tasklet processing would happen here
    // Tasklet subsystem will register real handler
}

/// Default scheduler softirq handler
#[allow(dead_code)]
pub fn sched_softirq_handler(_softirq: SoftirqType) {
    // Scheduler load balancing would happen here
    // Scheduler will register real handler
}

/// Default RCU softirq handler
#[allow(dead_code)]
pub fn rcu_softirq_handler(_softirq: SoftirqType) {
    // RCU callback processing would happen here
    // RCU subsystem will register real handler
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softirq_type_conversion() {
        assert_eq!(SoftirqType::from_nr(0), Some(SoftirqType::HiTasklet));
        assert_eq!(SoftirqType::from_nr(3), Some(SoftirqType::NetRx));
        assert_eq!(SoftirqType::from_nr(9), Some(SoftirqType::Rcu));
        assert_eq!(SoftirqType::from_nr(10), None);
        assert_eq!(SoftirqType::from_nr(255), None);
    }

    #[test]
    fn test_softirq_type_as_nr() {
        assert_eq!(SoftirqType::HiTasklet.as_nr(), 0);
        assert_eq!(SoftirqType::Timer.as_nr(), 1);
        assert_eq!(SoftirqType::NetRx.as_nr(), 3);
        assert_eq!(SoftirqType::Rcu.as_nr(), 9);
    }
}
