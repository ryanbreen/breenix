//! Per-CPU data support for ARM64 using TPIDR_EL1.
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the TPIDR_EL1 register without locks.
//!
//! Unlike the x86_64 version, this is a simpler implementation that
//! delegates most operations directly to the HAL layer since the task
//! and scheduling subsystems are not yet ported to ARM64.

use core::sync::atomic::{AtomicBool, Ordering};

// Import HAL per-CPU operations
use crate::arch_impl::current::percpu as hal_percpu;
use crate::arch_impl::PerCpuOps;

/// Per-CPU data structure for ARM64 (simplified version).
/// The full PerCpuData structure layout is defined in the HAL constants
/// and the actual storage is accessed via TPIDR_EL1.
#[repr(C, align(64))]
pub struct PerCpuData {
    /// CPU ID (offset 0)
    pub cpu_id: u64,
    /// Current thread pointer (offset 8) - unused on ARM64 currently
    pub current_thread: *mut u8,
    /// Kernel stack pointer (offset 16)
    pub kernel_stack_top: u64,
    /// Idle thread pointer (offset 24) - unused on ARM64 currently
    pub idle_thread: *mut u8,
    /// Preempt count (offset 32)
    pub preempt_count: u32,
    /// Need resched flag (offset 36)
    pub need_resched: u8,
    /// Padding
    _pad: [u8; 3],
    /// User SP scratch space (offset 40)
    pub user_sp_scratch: u64,
    /// TSS-equivalent pointer (offset 48) - unused on ARM64
    pub tss: *mut u8,
    /// Softirq pending bitmap (offset 56)
    pub softirq_pending: u32,
    /// Padding
    _pad2: u32,
    /// Next TTBR0 (offset 64) - equivalent to x86 next_cr3
    pub next_ttbr0: u64,
    /// Kernel TTBR0 (offset 72)
    pub kernel_ttbr0: u64,
    /// Saved process TTBR0 (offset 80)
    pub saved_process_ttbr0: u64,
    /// Exception cleanup context flag (offset 88)
    pub exception_cleanup_context: u8,
    /// Padding to match x86_64 layout
    _pad3: [u8; 103],
}

const _: () = assert!(core::mem::size_of::<PerCpuData>() == 192, "PerCpuData must be 192 bytes");

impl PerCpuData {
    /// Create a new per-CPU data structure
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id: cpu_id as u64,
            current_thread: core::ptr::null_mut(),
            kernel_stack_top: 0,
            idle_thread: core::ptr::null_mut(),
            preempt_count: 0,
            need_resched: 0,
            _pad: [0; 3],
            user_sp_scratch: 0,
            tss: core::ptr::null_mut(),
            softirq_pending: 0,
            _pad2: 0,
            next_ttbr0: 0,
            kernel_ttbr0: 0,
            saved_process_ttbr0: 0,
            exception_cleanup_context: 0,
            _pad3: [0; 103],
        }
    }
}

/// Static per-CPU data for CPU 0 (BSP)
static mut CPU0_DATA: PerCpuData = PerCpuData::new(0);

/// Flag to indicate whether per-CPU data is initialized
static PER_CPU_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Check if per-CPU data has been initialized
pub fn is_initialized() -> bool {
    PER_CPU_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize per-CPU data for the current CPU
pub fn init() {
    log::info!("Initializing per-CPU data via TPIDR_EL1");

    // Get pointer to CPU0's per-CPU data
    let cpu_data_ptr = &raw mut CPU0_DATA as *mut PerCpuData;
    let cpu_data_addr = cpu_data_ptr as u64;

    // Initialize via HAL
    unsafe {
        hal_percpu::init_percpu(cpu_data_addr, 0);
    }

    log::info!("Per-CPU data initialized at {:#x}", cpu_data_addr);
    log::debug!("  TPIDR_EL1 = {:#x}", hal_percpu::percpu_base());

    // Verification
    let read_cpu_id = hal_percpu::Aarch64PerCpu::cpu_id();
    if read_cpu_id != 0 {
        panic!("HAL verification failed: cpu_id read-back mismatch (expected 0, got {})", read_cpu_id);
    }
    log::info!("HAL read-back verification passed: TPIDR_EL1-relative operations working");

    // Mark per-CPU data as initialized
    PER_CPU_INITIALIZED.store(true, Ordering::Release);
    log::info!("Per-CPU data marked as initialized");
}

/// Get the current thread pointer (raw)
pub fn current_thread_ptr() -> *mut u8 {
    hal_percpu::Aarch64PerCpu::current_thread_ptr()
}

/// Get the current thread from per-CPU data
pub fn current_thread() -> Option<&'static mut crate::task::thread::Thread> {
    let thread_ptr = hal_percpu::Aarch64PerCpu::current_thread_ptr() as *mut crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some(&mut *thread_ptr) }
    }
}

/// Set the current thread in per-CPU data
pub fn set_current_thread(thread: *mut crate::task::thread::Thread) {
    unsafe {
        hal_percpu::Aarch64PerCpu::set_current_thread_ptr(thread as *mut u8);
    }
}

/// Set the current thread pointer
pub fn set_current_thread_ptr(ptr: *mut u8) {
    unsafe {
        hal_percpu::Aarch64PerCpu::set_current_thread_ptr(ptr);
    }
}

/// Get the kernel stack top
pub fn kernel_stack_top() -> u64 {
    hal_percpu::Aarch64PerCpu::kernel_stack_top()
}

/// Set the kernel stack top
pub fn set_kernel_stack_top(stack_top: u64) {
    unsafe {
        hal_percpu::Aarch64PerCpu::set_kernel_stack_top(stack_top);
    }
}

/// Check if we need to reschedule
pub fn need_resched() -> bool {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        hal_percpu::Aarch64PerCpu::need_resched()
    } else {
        false
    }
}

/// Set the reschedule needed flag
pub fn set_need_resched(need: bool) {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        unsafe {
            hal_percpu::Aarch64PerCpu::set_need_resched(need);
        }
    }
}

/// Check if we're in any interrupt context
pub fn in_interrupt() -> bool {
    hal_percpu::Aarch64PerCpu::in_interrupt()
}

/// Check if we're in hardware interrupt context
pub fn in_hardirq() -> bool {
    hal_percpu::Aarch64PerCpu::in_hardirq()
}

/// Check if we're in softirq context
pub fn in_softirq() -> bool {
    hal_percpu::Aarch64PerCpu::in_softirq()
}

/// Check if we're in NMI/FIQ context
pub fn in_nmi() -> bool {
    hal_percpu::Aarch64PerCpu::in_nmi()
}

/// Enter hardware IRQ context
pub fn irq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_enter called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::irq_enter();
    }
}

/// Exit hardware IRQ context
pub fn irq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_exit called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::irq_exit();
    }
}

/// Enter NMI context
pub fn nmi_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "nmi_enter called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::nmi_enter();
    }
}

/// Exit NMI context
pub fn nmi_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "nmi_exit called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::nmi_exit();
    }
}

/// Enter softirq context
pub fn softirq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "softirq_enter called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::softirq_enter();
    }
}

/// Exit softirq context
pub fn softirq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "softirq_exit called before per-CPU initialization");
    unsafe {
        hal_percpu::Aarch64PerCpu::softirq_exit();
    }
}

/// Increment preempt count (disable kernel preemption)
pub fn preempt_disable() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_disable called before per-CPU initialization");
    hal_percpu::Aarch64PerCpu::preempt_disable();
}

/// Decrement preempt count (enable kernel preemption)
pub fn preempt_enable() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_enable called before per-CPU initialization");
    hal_percpu::Aarch64PerCpu::preempt_enable();
}

/// Get current preempt count
pub fn preempt_count() -> u32 {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_count called before per-CPU initialization");
    hal_percpu::Aarch64PerCpu::preempt_count()
}

/// Clear PREEMPT_ACTIVE bit
pub fn clear_preempt_active() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::clear_preempt_active();
    }
}

/// Get pending softirq bitmap
pub fn softirq_pending() -> u32 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    hal_percpu::Aarch64PerCpu::softirq_pending()
}

/// Set softirq pending bit
pub fn raise_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::raise_softirq(nr);
    }
}

/// Clear softirq pending bit
pub fn clear_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::clear_softirq(nr);
    }
}

/// Process pending softirqs (minimal implementation)
pub fn do_softirq() {
    if in_interrupt() {
        return;
    }
    softirq_enter();
    let pending = softirq_pending();
    if pending != 0 {
        for nr in 0..32 {
            if (pending & (1 << nr)) != 0 {
                clear_softirq(nr);
            }
        }
    }
    softirq_exit();
}

/// Get the target TTBR0 for next exception return
pub fn get_next_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    hal_percpu::Aarch64PerCpu::next_cr3()
}

/// Set the target TTBR0 for next exception return
pub fn set_next_cr3(ttbr0: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::set_next_cr3(ttbr0);
    }
}

/// Get the kernel TTBR0
pub fn get_kernel_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    hal_percpu::Aarch64PerCpu::kernel_cr3()
}

/// Set the kernel TTBR0
pub fn set_kernel_cr3(ttbr0: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        log::warn!("set_kernel_cr3 called before per-CPU init");
        return;
    }
    log::info!("Setting kernel_ttbr0 in per-CPU data to {:#x}", ttbr0);
    unsafe {
        hal_percpu::Aarch64PerCpu::set_kernel_cr3(ttbr0);
    }
}

/// Set the exception cleanup context flag
pub fn set_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::set_exception_cleanup_context(true);
    }
}

/// Clear the exception cleanup context flag
pub fn clear_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::Aarch64PerCpu::set_exception_cleanup_context(false);
    }
}

/// Check if we're in exception cleanup context
pub fn in_exception_cleanup_context() -> bool {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return false;
    }
    hal_percpu::Aarch64PerCpu::exception_cleanup_context()
}

/// Get per-CPU base address and size for logging
pub fn get_percpu_info() -> (u64, usize) {
    let cpu_data_ptr = &raw mut CPU0_DATA as *mut PerCpuData;
    let base = cpu_data_ptr as u64;
    let size = core::mem::size_of::<PerCpuData>();
    (base, size)
}
