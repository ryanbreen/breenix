//! Per-CPU data support using GS segment
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the GS segment register without locks.
//!
//! Architecture-specific operations (GS-relative memory access, MSR operations)
//! are delegated to the HAL's per-CPU module.

use core::mem::offset_of;
use core::ptr;
use core::sync::atomic::{compiler_fence, Ordering, AtomicU64};
use x86_64::VirtAddr;

// Import HAL per-CPU operations and traits
use crate::arch_impl::current::percpu as hal_percpu;
use crate::arch_impl::PerCpuOps;

// Import HAL constants - single source of truth for GS-relative offsets
use crate::arch_impl::x86_64::constants::{
    PERCPU_CPU_ID_OFFSET, PERCPU_CURRENT_THREAD_OFFSET, PERCPU_KERNEL_STACK_TOP_OFFSET,
    PERCPU_IDLE_THREAD_OFFSET, PERCPU_PREEMPT_COUNT_OFFSET, PERCPU_NEED_RESCHED_OFFSET,
    PERCPU_USER_RSP_SCRATCH_OFFSET, PERCPU_TSS_OFFSET, PERCPU_SOFTIRQ_PENDING_OFFSET,
    PERCPU_NEXT_CR3_OFFSET, PERCPU_KERNEL_CR3_OFFSET, PERCPU_SAVED_PROCESS_CR3_OFFSET,
    PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
};

// Global tracking counters for irq_enter/irq_exit balance analysis
static IRQ_ENTER_COUNT: AtomicU64 = AtomicU64::new(0);
static IRQ_EXIT_COUNT: AtomicU64 = AtomicU64::new(0);
static MAX_PREEMPT_IMBALANCE: AtomicU64 = AtomicU64::new(0);

/// Per-CPU data structure with cache-line alignment and stable ABI
/// This structure is accessed from assembly code, so field order and offsets must be stable
/// CRITICAL: The repr(C) attribute ensures field ordering matches declaration order
#[repr(C, align(64))]
pub struct PerCpuData {
    /// CPU ID (offset 0) - for multi-processor support
    pub cpu_id: u64,

    /// Current thread pointer (offset 8)
    pub current_thread: *mut crate::task::thread::Thread,

    /// Kernel stack pointer for syscalls/interrupts (offset 16) - TSS.RSP0
    pub kernel_stack_top: u64,

    /// Idle thread pointer (offset 24)
    pub idle_thread: *mut crate::task::thread::Thread,

    /// Preempt count for kernel preemption control (offset 32) - properly aligned u32
    /// Linux-style bit layout:
    /// Bits 0-7:   PREEMPT count (8 bits, nested preempt_disable calls)
    /// Bits 8-15:  SOFTIRQ count (8 bits, nested softirq handlers)
    /// Bits 16-25: HARDIRQ count (10 bits, nested hardware interrupts)
    /// Bit 26:     NMI flag (1 bit, in NMI context)
    /// Bit 27:     Reserved
    /// Bit 28:     PREEMPT_ACTIVE flag
    /// Bits 29-31: Reserved
    pub preempt_count: u32,

    /// Reschedule needed flag (offset 36) - u8 for compact layout
    pub need_resched: u8,

    /// Explicit padding to maintain alignment (offset 37-39)
    _pad: [u8; 3],

    /// User RSP scratch space for syscall entry (offset 40)
    pub user_rsp_scratch: u64,

    /// TSS pointer for this CPU (offset 48)
    pub tss: *mut x86_64::structures::tss::TaskStateSegment,

    /// Softirq pending bitmap (offset 56) - 32 bits for different softirq types
    pub softirq_pending: u32,

    /// Padding to align next_cr3 (offset 60-63)
    _pad2: u32,

    /// Target CR3 for next IRETQ (offset 64) - set before context switch
    /// 0 means no CR3 switch needed
    pub next_cr3: u64,

    /// Kernel CR3 (offset 72) - the master kernel page table
    /// Used by interrupt/syscall entry to switch to kernel page tables
    pub kernel_cr3: u64,

    /// Saved process CR3 (offset 80) - saved on interrupt entry from userspace
    /// Used to restore process page tables on interrupt exit if no context switch
    pub saved_process_cr3: u64,

    /// Exception cleanup context flag (offset 88) - allows scheduling from kernel mode
    /// Set by exception handlers (GPF, page fault) when they terminate a process
    /// and need to allow scheduling from kernel mode
    pub exception_cleanup_context: u8,

    /// Padding to align diagnostic fields (offset 89-95)
    _pad3: [u8; 7],

    // === Context Switch Diagnostics (Ultra-low overhead) ===
    // These fields detect state corruption during context switches without
    // adding logging overhead to the hot path. Based on seL4/Linux patterns.

    /// Pre-switch canary (offset 96): RSP ^ CR3 | MAGIC_PRE
    /// Set before context switch, verified after to detect corruption
    pub switch_pre_canary: u64,

    /// Post-switch canary (offset 104): RSP ^ CR3 | MAGIC_POST
    /// Set after context switch for comparison with pre-canary
    pub switch_post_canary: u64,

    /// TSC timestamp (offset 112): rdtsc value when context switch started
    /// Used to detect stuck transitions (timeout detection)
    pub switch_tsc: u64,

    /// Switch violation count (offset 120): Number of detected violations
    /// Incremented atomically on canary mismatch
    pub switch_violations: u64,

    /// Padding to reach 192 bytes (align(64) boundary)
    /// (offset 128-191): 64 bytes of padding
    _pad_final: [u8; 64],
}

// Linux-style preempt_count bit layout constants
// Matches Linux kernel's exact bit partitioning
#[allow(dead_code)]
const PREEMPT_BITS: u32 = 8;
#[allow(dead_code)]
const SOFTIRQ_BITS: u32 = 8;
#[allow(dead_code)]
const HARDIRQ_BITS: u32 = 10;  // Linux uses 10 bits for HARDIRQ
#[allow(dead_code)]
const NMI_BITS: u32 = 1;       // Linux uses 1 bit for NMI

#[allow(dead_code)]
const PREEMPT_SHIFT: u32 = 0;
#[allow(dead_code)]
const SOFTIRQ_SHIFT: u32 = PREEMPT_SHIFT + PREEMPT_BITS;  // 8
#[allow(dead_code)]
const HARDIRQ_SHIFT: u32 = SOFTIRQ_SHIFT + SOFTIRQ_BITS;   // 16
#[allow(dead_code)]
const NMI_SHIFT: u32 = HARDIRQ_SHIFT + HARDIRQ_BITS;       // 26

#[allow(dead_code)]
const PREEMPT_MASK: u32 = ((1 << PREEMPT_BITS) - 1) << PREEMPT_SHIFT;  // 0x000000FF
#[allow(dead_code)]
const SOFTIRQ_MASK: u32 = ((1 << SOFTIRQ_BITS) - 1) << SOFTIRQ_SHIFT;  // 0x0000FF00
#[allow(dead_code)]
const HARDIRQ_MASK: u32 = ((1 << HARDIRQ_BITS) - 1) << HARDIRQ_SHIFT;  // 0x03FF0000
#[allow(dead_code)]
const NMI_MASK: u32 = ((1 << NMI_BITS) - 1) << NMI_SHIFT;              // 0x04000000

#[allow(dead_code)]
const PREEMPT_ACTIVE: u32 = 1 << 28;

// Increment values for each nesting level
#[allow(dead_code)]
const PREEMPT_OFFSET: u32 = 1 << PREEMPT_SHIFT;
#[allow(dead_code)]
const SOFTIRQ_OFFSET: u32 = 1 << SOFTIRQ_SHIFT;
#[allow(dead_code)]
const HARDIRQ_OFFSET: u32 = 1 << HARDIRQ_SHIFT;
#[allow(dead_code)]
const NMI_OFFSET: u32 = 1 << NMI_SHIFT;

// Compile-time assertions to verify HAL constants match struct layout
// These use offset_of! to get actual offsets and compare with HAL constants
// If any assertion fails, the HAL constant is out of sync with the struct

const _: () = assert!(offset_of!(PerCpuData, cpu_id) == PERCPU_CPU_ID_OFFSET,
    "PERCPU_CPU_ID_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, current_thread) == PERCPU_CURRENT_THREAD_OFFSET,
    "PERCPU_CURRENT_THREAD_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, kernel_stack_top) == PERCPU_KERNEL_STACK_TOP_OFFSET,
    "PERCPU_KERNEL_STACK_TOP_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, idle_thread) == PERCPU_IDLE_THREAD_OFFSET,
    "PERCPU_IDLE_THREAD_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, preempt_count) == PERCPU_PREEMPT_COUNT_OFFSET,
    "PERCPU_PREEMPT_COUNT_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, need_resched) == PERCPU_NEED_RESCHED_OFFSET,
    "PERCPU_NEED_RESCHED_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, user_rsp_scratch) == PERCPU_USER_RSP_SCRATCH_OFFSET,
    "PERCPU_USER_RSP_SCRATCH_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, tss) == PERCPU_TSS_OFFSET,
    "PERCPU_TSS_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, softirq_pending) == PERCPU_SOFTIRQ_PENDING_OFFSET,
    "PERCPU_SOFTIRQ_PENDING_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, next_cr3) == PERCPU_NEXT_CR3_OFFSET,
    "PERCPU_NEXT_CR3_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, kernel_cr3) == PERCPU_KERNEL_CR3_OFFSET,
    "PERCPU_KERNEL_CR3_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, saved_process_cr3) == PERCPU_SAVED_PROCESS_CR3_OFFSET,
    "PERCPU_SAVED_PROCESS_CR3_OFFSET mismatch with struct layout");
const _: () = assert!(offset_of!(PerCpuData, exception_cleanup_context) == PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
    "PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET mismatch with struct layout");

// Alignment assertions
const _: () = assert!(PERCPU_PREEMPT_COUNT_OFFSET % 4 == 0, "preempt_count must be 4-byte aligned");
const _: () = assert!(PERCPU_USER_RSP_SCRATCH_OFFSET % 8 == 0, "user_rsp_scratch must be 8-byte aligned");
const _: () = assert!(core::mem::size_of::<usize>() == 8, "This code assumes 64-bit pointers");

// Verify struct size is 192 bytes due to align(64) attribute
// The actual data is 128 bytes (switch_violations ends at offset 128), but align(64) rounds up to 192
const _: () = assert!(core::mem::size_of::<PerCpuData>() == 192, "PerCpuData must be 192 bytes (aligned to 64)");

// Verify bit layout matches Linux kernel
const _: () = assert!(PREEMPT_MASK == 0x000000FF, "PREEMPT_MASK incorrect");
const _: () = assert!(SOFTIRQ_MASK == 0x0000FF00, "SOFTIRQ_MASK incorrect");
const _: () = assert!(HARDIRQ_MASK == 0x03FF0000, "HARDIRQ_MASK incorrect");
const _: () = assert!(NMI_MASK == 0x04000000, "NMI_MASK incorrect");
const _: () = assert!(NMI_SHIFT == 26, "NMI_SHIFT must be 26 to match Linux");

impl PerCpuData {
    /// Create a new per-CPU data structure
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id: cpu_id as u64,
            current_thread: ptr::null_mut(),
            kernel_stack_top: 0,
            idle_thread: ptr::null_mut(),
            preempt_count: 0,
            need_resched: 0,
            _pad: [0; 3],
            user_rsp_scratch: 0,
            tss: ptr::null_mut(),
            softirq_pending: 0,
            _pad2: 0,
            next_cr3: 0,
            kernel_cr3: 0,
            saved_process_cr3: 0,
            exception_cleanup_context: 0,
            _pad3: [0; 7],
            switch_pre_canary: 0,
            switch_post_canary: 0,
            switch_tsc: 0,
            switch_violations: 0,
            _pad_final: [0; 64],
        }
    }
}

/// Static per-CPU data for CPU 0 (BSP)
/// In a real SMP kernel, we'd have an array of these
static mut CPU0_DATA: PerCpuData = PerCpuData::new(0);

/// Flag to indicate whether per-CPU data is initialized and safe to use
/// CRITICAL: Interrupts MUST be disabled until this is true
static PER_CPU_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Check if per-CPU data has been initialized
/// Note: Used in non-interactive builds (logger.rs framebuffer check)
#[allow(dead_code)]
pub fn is_initialized() -> bool {
    PER_CPU_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize per-CPU data for the current CPU
pub fn init() {
    use crate::arch_impl::PageTableOps;
    use crate::arch_impl::current::paging::X86PageTableOps;

    log::info!("Initializing per-CPU data via GS segment");

    // Get pointer to CPU0's per-CPU data
    let cpu_data_ptr = &raw mut CPU0_DATA as *mut PerCpuData;
    let cpu_data_addr = cpu_data_ptr as u64;

    // Set up GS base to point to per-CPU data via HAL
    // This allows us to access per-CPU data via GS segment
    unsafe {
        hal_percpu::msr::write_gs_base_msr(cpu_data_addr);
        hal_percpu::write_kernel_gs_base(cpu_data_addr);
    }

    log::info!("Per-CPU data initialized at {:#x}", cpu_data_addr);
    log::debug!("  GS_BASE = {:#x}", hal_percpu::msr::read_gs_base_msr());
    log::debug!("  KERNEL_GS_BASE = {:#x}", hal_percpu::read_kernel_gs_base());

    // HAL Read-back verification: Verify GS-relative operations actually work
    // This catches misconfigured GS base before any interrupt handlers run

    let read_cpu_id = hal_percpu::X86PerCpu::cpu_id();
    if read_cpu_id != 0 {
        panic!("HAL verification failed: cpu_id read-back mismatch (expected 0, got {})", read_cpu_id);
    }

    // Verify preempt_count read/write cycle
    let initial_preempt = hal_percpu::X86PerCpu::preempt_count();
    hal_percpu::X86PerCpu::preempt_disable();
    let after_disable = hal_percpu::X86PerCpu::preempt_count();
    if after_disable != initial_preempt + 1 {
        panic!("HAL verification failed: preempt_disable did not increment (expected {}, got {})",
               initial_preempt + 1, after_disable);
    }
    hal_percpu::X86PerCpu::preempt_enable();
    let after_enable = hal_percpu::X86PerCpu::preempt_count();
    if after_enable != initial_preempt {
        panic!("HAL verification failed: preempt_enable did not restore (expected {}, got {})",
               initial_preempt, after_enable);
    }
    log::info!("HAL read-back verification passed: GS-relative operations working");

    // Mark per-CPU data as initialized and safe to use
    PER_CPU_INITIALIZED.store(true, Ordering::Release);
    log::info!("Per-CPU data marked as initialized - preempt_count functions now use per-CPU storage");

    // Store the current CR3 as the initial kernel CR3 via HAL
    // NOTE: At this point, we're still using the bootloader's page tables.
    // After memory::init() calls build_master_kernel_pml4(), the kernel switches
    // to the master PML4 and calls set_kernel_cr3() to update this value.
    // This initial value provides a fallback during early boot.
    let kernel_cr3_val = X86PageTableOps::read_root();
    log::info!("Storing initial kernel_cr3 = {:#x} in per-CPU data (bootloader PT)", kernel_cr3_val);

    unsafe {
        hal_percpu::X86PerCpu::set_kernel_cr3(kernel_cr3_val);
    }
    log::info!("kernel_cr3 stored successfully - interrupt handlers can now switch to kernel page tables");

    // HAL boot stage marker - proves HAL per-CPU operations are working
    log::info!("HAL_PERCPU_INITIALIZED: Per-CPU data setup via HAL complete");
}

/// Get the current thread from per-CPU data
pub fn current_thread() -> Option<&'static mut crate::task::thread::Thread> {
    // Use HAL for GS-relative access

    let thread_ptr = hal_percpu::X86PerCpu::current_thread_ptr() as *mut crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some(&mut *thread_ptr) }
    }
}

/// Set the current thread in per-CPU data
pub fn set_current_thread(thread: *mut crate::task::thread::Thread) {
    // Use HAL for GS-relative access

    unsafe {
        hal_percpu::X86PerCpu::set_current_thread_ptr(thread as *mut u8);
    }
}

/// Get the kernel stack top from per-CPU data
pub fn kernel_stack_top() -> u64 {
    // Use HAL for GS-relative access

    hal_percpu::X86PerCpu::kernel_stack_top()
}

/// Set the kernel stack top in per-CPU data
pub fn set_kernel_stack_top(stack_top: u64) {
    // Use HAL for GS-relative access

    unsafe {
        hal_percpu::X86PerCpu::set_kernel_stack_top(stack_top);
    }
}

/// Check if we need to reschedule
pub fn need_resched() -> bool {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        // Use HAL for GS-relative access
        hal_percpu::X86PerCpu::need_resched()
    } else {
        false
    }
}

/// Set the reschedule needed flag
pub fn set_need_resched(need: bool) {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        // Use HAL for GS-relative access
        unsafe {
            hal_percpu::X86PerCpu::set_need_resched(need);
        }
    }
}

/// Check if we're in any interrupt context (hardware IRQ, softirq, or NMI)
/// Returns true if any interrupt nesting level is non-zero
pub fn in_interrupt() -> bool {
    // Use HAL for interrupt context check

    hal_percpu::X86PerCpu::in_interrupt()
}

/// Check if we're in hardware interrupt context
pub fn in_hardirq() -> bool {
    // Use HAL for hardirq context check

    hal_percpu::X86PerCpu::in_hardirq()
}

/// Check if we're in softirq context
pub fn in_softirq() -> bool {
    // Use HAL for softirq context check
    hal_percpu::X86PerCpu::in_softirq()
}

/// Check if we're in NMI context
pub fn in_nmi() -> bool {
    // Use HAL for NMI context check
    hal_percpu::X86PerCpu::in_nmi()
}

/// Enter hardware IRQ context (called by interrupt handlers)
pub fn irq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_enter called before per-CPU initialization");

    // Track irq_enter calls for balance analysis
    IRQ_ENTER_COUNT.fetch_add(1, Ordering::Relaxed);

    // Use HAL for atomic GS-relative increment
    unsafe {
        hal_percpu::X86PerCpu::irq_enter();
    }

    // LOGGING REMOVED: All logging removed to prevent serial lock deadlock
}

/// Exit hardware IRQ context
pub fn irq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_exit called before per-CPU initialization");

    // Debug-only underflow check: verify we're in hardirq context before decrementing
    #[cfg(debug_assertions)]
    {
    
        let count_before = hal_percpu::X86PerCpu::preempt_count();
        debug_assert!(
            (count_before & HARDIRQ_MASK) != 0,
            "irq_exit called but HARDIRQ count is already 0 (preempt_count={:#x})",
            count_before
        );
    }

    // Track irq_exit calls for balance analysis
    IRQ_EXIT_COUNT.fetch_add(1, Ordering::Relaxed);

    // Check for imbalance
    let enters = IRQ_ENTER_COUNT.load(Ordering::Relaxed);
    let exits = IRQ_EXIT_COUNT.load(Ordering::Relaxed);
    if enters > exits {
        let imbalance = enters - exits;
        MAX_PREEMPT_IMBALANCE.fetch_max(imbalance, Ordering::Relaxed);
    }

    // Use HAL for atomic GS-relative decrement
    unsafe {
        hal_percpu::X86PerCpu::irq_exit();
    }

    // LOGGING REMOVED: All logging removed to prevent serial lock deadlock

    // Check if we should process softirqs after exiting hardirq
    // Use HAL to read current preempt_count

    let new_count = hal_percpu::X86PerCpu::preempt_count();

    if new_count == 0 {
        // Check if any softirqs are pending
        let pending = softirq_pending();
        if pending != 0 {
            // Process softirqs (logging removed to prevent deadlock)
            do_softirq();
        }
    }
}

/// Enter NMI context (Non-Maskable Interrupt)
pub fn nmi_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "nmi_enter called before per-CPU initialization");

    // Use HAL for atomic GS-relative increment (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::nmi_enter();
    }
}

/// Exit NMI context
pub fn nmi_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "nmi_exit called before per-CPU initialization");

    // Debug-only underflow check: verify we're in NMI context before decrementing
    #[cfg(debug_assertions)]
    {
    
        let count_before = hal_percpu::X86PerCpu::preempt_count();
        debug_assert!(
            (count_before & NMI_MASK) != 0,
            "nmi_exit called but NMI count is already 0 (preempt_count={:#x})",
            count_before
        );
    }

    // Use HAL for atomic GS-relative decrement (includes compiler fences)
    // NMIs never schedule
    unsafe {
        hal_percpu::X86PerCpu::nmi_exit();
    }
}

/// Enter softirq context (software interrupt / bottom half)
pub fn softirq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "softirq_enter called before per-CPU initialization");

    // Use HAL for atomic GS-relative increment (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::softirq_enter();
    }
}

/// Exit softirq context
pub fn softirq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "softirq_exit called before per-CPU initialization");

    // Debug-only underflow check: verify we're in softirq context before decrementing
    #[cfg(debug_assertions)]
    {
    
        let count_before = hal_percpu::X86PerCpu::preempt_count();
        debug_assert!(
            (count_before & SOFTIRQ_MASK) != 0,
            "softirq_exit called but SOFTIRQ count is already 0 (preempt_count={:#x})",
            count_before
        );
    }

    // Use HAL for atomic GS-relative decrement (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::softirq_exit();
    }

    // Check if we should schedule on softirq exit (similar to IRQ exit)
    // Only if we're returning to preemptible context

    let new_count = hal_percpu::X86PerCpu::preempt_count();
    if new_count == 0 && need_resched() {
        log::info!("softirq_exit: Triggering preempt_schedule_irq");
        crate::task::scheduler::preempt_schedule_irq();
    }
}

/// Get the idle thread from per-CPU data
#[allow(dead_code)]
pub fn idle_thread() -> Option<&'static mut crate::task::thread::Thread> {
    // Use HAL for GS-relative access
    let thread_ptr = hal_percpu::X86PerCpu::idle_thread_ptr() as *mut crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some(&mut *thread_ptr) }
    }
}

/// Set the idle thread in per-CPU data
pub fn set_idle_thread(thread: *mut crate::task::thread::Thread) {
    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_idle_thread_ptr(thread as *mut u8);
    }
}

/// Update TSS RSP0 with the current thread's kernel stack
/// This must be called on every context switch to a thread
pub fn update_tss_rsp0(kernel_stack_top: u64) {
    // Get TSS pointer via HAL
    let tss_ptr = hal_percpu::X86PerCpu::tss_ptr() as *mut x86_64::structures::tss::TaskStateSegment;

    if !tss_ptr.is_null() {
        // Update per-CPU kernel_stack_top via HAL
    
        unsafe {
            hal_percpu::X86PerCpu::set_kernel_stack_top(kernel_stack_top);
        }

        // Update TSS.RSP0
        unsafe {
            (*tss_ptr).privilege_stack_table[0] = VirtAddr::new(kernel_stack_top);
        }
    }
}

/// Set the TSS pointer for this CPU
pub fn set_tss(tss: *mut x86_64::structures::tss::TaskStateSegment) {
    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_tss_ptr(tss as *mut u8);
    }
}

/// Get the user RSP scratch space (used during syscall entry)
#[allow(dead_code)]
pub fn user_rsp_scratch() -> u64 {
    // Use HAL for GS-relative access
    hal_percpu::X86PerCpu::user_rsp_scratch()
}

/// Set the user RSP scratch space (used during syscall entry)
#[allow(dead_code)]
pub fn set_user_rsp_scratch(rsp: u64) {
    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_user_rsp_scratch(rsp);
    }
}

/// Increment preempt count (disable kernel preemption)
/// Only manipulates the PREEMPT bits (0-7), not interrupt counts
/// CRITICAL: Must only be called after per_cpu::init() with interrupts disabled until then
///
/// NOTE on compiler fences: This function adds fences because the HAL's preempt_disable()
/// is a minimal trait implementation without fences. In contrast, irq_enter/exit, nmi_enter/exit,
/// and softirq_enter/exit wrappers don't add fences because their HAL implementations already
/// include them.
pub fn preempt_disable() {
    // Per-CPU data must be initialized before any preemption operations
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_disable called before per-CPU initialization");

    // Compiler barrier before incrementing preempt count
    compiler_fence(Ordering::Acquire);

    // Use HAL for atomic GS-relative increment

    hal_percpu::X86PerCpu::preempt_disable();

    // Compiler barrier after incrementing preempt count
    compiler_fence(Ordering::Release);

    // CRITICAL: Do NOT use log:: macros here as they may recursively call preempt_disable!
}

/// Decrement preempt count (enable kernel preemption)
/// Only manipulates the PREEMPT bits (0-7), not interrupt counts
/// May trigger scheduling if preempt count reaches 0 and not in interrupt context
/// CRITICAL: Must only be called after per_cpu::init() with interrupts disabled until then
pub fn preempt_enable() {
    // Per-CPU data must be initialized before any preemption operations
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_enable called before per-CPU initialization");

    // Compiler barrier before decrementing preempt count
    compiler_fence(Ordering::Acquire);

    // Use HAL for atomic GS-relative decrement

    hal_percpu::X86PerCpu::preempt_enable();

    // Compiler barrier after decrementing preempt count
    compiler_fence(Ordering::Release);

    // CRITICAL: Disable logging to prevent recursion issues

    // Check if we should schedule after preempt_enable
    // Note: We intentionally do NOT call try_schedule() or clear need_resched here.
    // The syscall return path and timer interrupt return path both check
    // need_resched and call check_need_resched_and_switch() which performs
    // the actual context switch with proper register save/restore.
}

/// Get current preempt count
pub fn preempt_count() -> u32 {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_count called before per-CPU initialization");

    // Use HAL for GS-relative access

    hal_percpu::X86PerCpu::preempt_count()
}

/// Clear PREEMPT_ACTIVE bit (bit 28) from preempt_count
///
/// This is called after a context switch completes to clear the flag that was
/// protecting the OLD thread's syscall return path. The NEW thread is not in
/// syscall return, so the flag should not persist.
///
/// Linux clears PREEMPT_ACTIVE in schedule_tail() after a context switch.
/// We follow the same pattern here.
pub fn clear_preempt_active() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for atomic GS-relative bit clear
    unsafe {
        hal_percpu::X86PerCpu::clear_preempt_active();
    }
}

/// Get pending softirq bitmap
pub fn softirq_pending() -> u32 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }

    // Use HAL for GS-relative access
    hal_percpu::X86PerCpu::softirq_pending()
}

/// Set softirq pending bit
#[allow(dead_code)]
pub fn raise_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");

    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for atomic GS-relative bit set
    unsafe {
        hal_percpu::X86PerCpu::raise_softirq(nr);
    }
}

/// Clear softirq pending bit
pub fn clear_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");

    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for atomic GS-relative bit clear
    unsafe {
        hal_percpu::X86PerCpu::clear_softirq(nr);
    }
}

/// Process pending softirqs
/// This is called from irq_exit() when returning to non-interrupt context
pub fn do_softirq() {
    // Don't process softirqs if we're in interrupt context (nested)
    if in_interrupt() {
        return;
    }
    
    // Enter softirq context
    softirq_enter();
    
    // Process pending softirqs
    let pending = softirq_pending();
    if pending != 0 {
        log::debug!("do_softirq: Processing pending softirqs (bitmap={:#x})", pending);
        
        // Process each pending softirq
        // In a real implementation, we'd have an array of softirq handlers
        // For now, we just clear them and log
        for nr in 0..32 {
            if (pending & (1 << nr)) != 0 {
                clear_softirq(nr);
                // log::trace!("  Processing softirq {}", nr);  // Disabled to avoid deadlock
                // softirq_handlers[nr]() would be called here
            }
        }
    }
    
    // Exit softirq context
    softirq_exit();
}

/// Get the target CR3 for next IRETQ
/// Returns 0 if no CR3 switch is needed
#[allow(dead_code)]
pub fn get_next_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }

    // Use HAL for GS-relative access
    hal_percpu::X86PerCpu::next_cr3()
}

/// Set the target CR3 for next IRETQ
/// This communicates to timer_entry.asm and entry.asm (syscall return)
/// which CR3 to switch to before returning to userspace.
/// CR3 switching is deferred to assembly code to avoid double TLB flushes.
pub fn set_next_cr3(cr3: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_next_cr3(cr3);
    }
}

/// Get the kernel CR3 (master kernel page table)
/// Returns 0 if not initialized
#[allow(dead_code)]
pub fn get_kernel_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }

    // Use HAL for GS-relative access
    hal_percpu::X86PerCpu::kernel_cr3()
}

/// Set the kernel CR3 (master kernel page table)
/// This should be called once after build_master_kernel_pml4()
pub fn set_kernel_cr3(cr3: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        log::warn!("set_kernel_cr3 called before per-CPU init, storing for later");
        // We can't store it yet, but we'll set it during init
        return;
    }

    log::info!("Setting kernel_cr3 in per-CPU data to {:#x}", cr3);
    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_kernel_cr3(cr3);
    }
}

/// Set the exception cleanup context flag (per-CPU)
/// Called by exception handlers (GPF, page fault) when they terminate a process
/// and need to allow scheduling from kernel mode
pub fn set_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_exception_cleanup_context(true);
    }
}

/// Clear the exception cleanup context flag (per-CPU)
/// Called after successfully switching to a new thread
pub fn clear_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Use HAL for GS-relative access
    unsafe {
        hal_percpu::X86PerCpu::set_exception_cleanup_context(false);
    }
}

/// Check if we're in exception cleanup context (per-CPU)
pub fn in_exception_cleanup_context() -> bool {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return false;
    }

    // Use HAL for GS-relative access
    hal_percpu::X86PerCpu::exception_cleanup_context()
}

/// Check if we can schedule (preempt_count == 0 and returning to userspace)
pub fn can_schedule(saved_cs: u64) -> bool {
    let current_preempt = preempt_count();
    let returning_to_userspace = (saved_cs & 3) == 3;

    // CRITICAL: Check if current_thread is set before accessing scheduler.
    // During early boot or before first context switch, gs:[8] may be NULL.
    // Timer interrupts can fire before any thread is set, causing a page fault
    // at CR2=0x8 (offset 8 in PerCpuData = current_thread pointer).
    if current_thread().is_none() {
        // No current thread set yet - cannot schedule
        static EARLY_RETURN_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let count = EARLY_RETURN_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if count < 10 {
            log::warn!("can_schedule: returning false - current_thread is None");
        }
        return false;
    }

    let mut returning_to_idle_kernel = false;
    if !returning_to_userspace {
        let current_tid = crate::task::scheduler::current_thread_id();
        let idle_tid = crate::task::scheduler::with_scheduler(|s| s.idle_thread());
        if let (Some(cur), Some(idle)) = (current_tid, idle_tid) {
            returning_to_idle_kernel = cur == idle;
        }
    }

    // Also allow scheduling if we're in exception cleanup context
    let in_exception_cleanup = in_exception_cleanup_context();

    // Check if current thread is blocked or terminated
    // When a thread blocks, it enters an HLT loop waiting for an interrupt.
    // When a thread terminates, it sets need_resched and expects immediate switch.
    // The timer interrupt should be able to switch to another thread.
    let current_thread_blocked_or_terminated = crate::task::scheduler::with_scheduler(|sched| {
        if let Some(current) = sched.current_thread_mut() {
            current.state == crate::task::thread::ThreadState::BlockedOnSignal
                || current.state == crate::task::thread::ThreadState::BlockedOnChildExit
                || current.state == crate::task::thread::ThreadState::Blocked
                || current.state == crate::task::thread::ThreadState::Terminated
        } else {
            false
        }
    }).unwrap_or(false);

    // CRITICAL: When in exception cleanup context, allow scheduling regardless of PREEMPT_ACTIVE.
    // The exception handler has explicitly requested a reschedule after terminating a process.
    // Without this, PREEMPT_ACTIVE (bit 28) blocks scheduling even though we need to recover.
    //
    // Also allow scheduling when the current thread is blocked or terminated - blocking syscalls
    // use HLT to wait for interrupts, and terminated threads need immediate switch.
    let result = in_exception_cleanup
                 || current_thread_blocked_or_terminated
                 || (current_preempt == 0 && (returning_to_userspace || returning_to_idle_kernel));

    // Note: Debug logging removed from hot path - use GDB if debugging is needed

    result
}

/// Get per-CPU base address and size for logging
#[allow(dead_code)]
pub fn get_percpu_info() -> (u64, usize) {
    let cpu_data_ptr = &raw mut CPU0_DATA as *mut PerCpuData;
    let base = cpu_data_ptr as u64;
    let size = core::mem::size_of::<PerCpuData>();
    (base, size)
}

/// Get total number of irq_enter calls (for diagnostics)
#[allow(dead_code)]
pub fn get_irq_enter_count() -> u64 {
    IRQ_ENTER_COUNT.load(Ordering::Relaxed)
}

/// Get total number of irq_exit calls (for diagnostics)
#[allow(dead_code)]
pub fn get_irq_exit_count() -> u64 {
    IRQ_EXIT_COUNT.load(Ordering::Relaxed)
}

/// Get maximum observed preempt imbalance (enters - exits)
/// A persistently high value may indicate missing irq_exit calls
#[allow(dead_code)]
pub fn get_max_preempt_imbalance() -> u64 {
    MAX_PREEMPT_IMBALANCE.load(Ordering::Relaxed)
}

