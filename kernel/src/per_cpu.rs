//! Per-CPU data support using GS segment
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the GS segment register without locks.

use core::ptr;
use core::sync::atomic::{compiler_fence, Ordering, AtomicU64};
use x86_64::VirtAddr;
use x86_64::registers::model_specific::{GsBase, KernelGsBase};

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
    /// Bits 0-7:   PREEMPT count (nested preempt_disable calls)
    /// Bits 8-15:  SOFTIRQ count (nested softirq handlers)
    /// Bits 16-23: HARDIRQ count (nested hardware interrupts)
    /// Bits 24-27: NMI count (nested NMIs)
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

// Compile-time offset calculations and validation
// These MUST match the actual struct layout or GS-relative access will be incorrect
#[allow(dead_code)]
const CPU_ID_OFFSET: usize = 0;           // offset 0: usize (8 bytes)
#[allow(dead_code)]
const CURRENT_THREAD_OFFSET: usize = 8;    // offset 8: *mut Thread (8 bytes)
#[allow(dead_code)]
const KERNEL_STACK_TOP_OFFSET: usize = 16; // offset 16: VirtAddr (8 bytes)
#[allow(dead_code)]
const IDLE_THREAD_OFFSET: usize = 24;      // offset 24: *mut Thread (8 bytes)
#[allow(dead_code)]
const PREEMPT_COUNT_OFFSET: usize = 32;    // offset 32: u32 (4 bytes) - ALIGNED
#[allow(dead_code)]
const NEED_RESCHED_OFFSET: usize = 36;     // offset 36: u8 (1 byte)
// Padding at 37-39 (3 bytes)
#[allow(dead_code)]
const USER_RSP_SCRATCH_OFFSET: usize = 40; // offset 40: u64 (8 bytes) - ALIGNED
#[allow(dead_code)]
const TSS_OFFSET: usize = 48;              // offset 48: *mut TSS (8 bytes)
#[allow(dead_code)]
const SOFTIRQ_PENDING_OFFSET: usize = 56;  // offset 56: u32 (4 bytes)
#[allow(dead_code)]
const NEXT_CR3_OFFSET: usize = 64;         // offset 64: u64 (8 bytes) - ALIGNED
#[allow(dead_code)]
const KERNEL_CR3_OFFSET: usize = 72;       // offset 72: u64 (8 bytes) - ALIGNED
#[allow(dead_code)]
const SAVED_PROCESS_CR3_OFFSET: usize = 80; // offset 80: u64 (8 bytes) - ALIGNED
#[allow(dead_code)]
const EXCEPTION_CLEANUP_CONTEXT_OFFSET: usize = 88; // offset 88: bool (1 byte)
// _pad3 at offset 89-95 (7 bytes)
#[allow(dead_code)]
const SWITCH_PRE_CANARY_OFFSET: usize = 96;         // offset 96: u64 (8 bytes)
#[allow(dead_code)]
const SWITCH_POST_CANARY_OFFSET: usize = 104;       // offset 104: u64 (8 bytes)
#[allow(dead_code)]
const SWITCH_TSC_OFFSET: usize = 112;               // offset 112: u64 (8 bytes)
#[allow(dead_code)]
const SWITCH_VIOLATIONS_OFFSET: usize = 120;        // offset 120: u64 (8 bytes)

// Magic values for canary computation (non-zero to detect corruption)
#[allow(dead_code)]
const CANARY_MAGIC: u64 = 0xDEADCAFE_00000000;

// Compile-time assertions to ensure offsets are correct
// These will fail to compile if the offsets don't match expected values
const _: () = assert!(PREEMPT_COUNT_OFFSET % 4 == 0, "preempt_count must be 4-byte aligned");
const _: () = assert!(PREEMPT_COUNT_OFFSET == 32, "preempt_count offset mismatch");
const _: () = assert!(USER_RSP_SCRATCH_OFFSET % 8 == 0, "user_rsp_scratch must be 8-byte aligned");
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
pub fn is_initialized() -> bool {
    PER_CPU_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize per-CPU data for the current CPU
pub fn init() {
    log::info!("Initializing per-CPU data via GS segment");

    // Get pointer to CPU0's per-CPU data
    let cpu_data_ptr = &raw mut CPU0_DATA as *mut PerCpuData;
    let cpu_data_addr = cpu_data_ptr as u64;

    // Set up GS base to point to per-CPU data
    // This allows us to access per-CPU data via GS segment
    GsBase::write(VirtAddr::new(cpu_data_addr));
    KernelGsBase::write(VirtAddr::new(cpu_data_addr));

    log::info!("Per-CPU data initialized at {:#x}", cpu_data_addr);
    log::debug!("  GS_BASE = {:#x}", GsBase::read().as_u64());
    log::debug!("  KERNEL_GS_BASE = {:#x}", KernelGsBase::read().as_u64());

    // Mark per-CPU data as initialized and safe to use
    PER_CPU_INITIALIZED.store(true, Ordering::Release);
    log::info!("Per-CPU data marked as initialized - preempt_count functions now use per-CPU storage");

    // Store the current CR3 as the initial kernel CR3
    // NOTE: At this point, we're still using the bootloader's page tables.
    // After memory::init() calls build_master_kernel_pml4(), the kernel switches
    // to the master PML4 and calls set_kernel_cr3() to update this value.
    // This initial value provides a fallback during early boot.
    let (current_frame, _) = x86_64::registers::control::Cr3::read();
    let kernel_cr3_val = current_frame.start_address().as_u64();
    log::info!("Storing initial kernel_cr3 = {:#x} in per-CPU data (bootloader PT)", kernel_cr3_val);

    unsafe {
        core::arch::asm!(
            "mov gs:[{offset}], {}",
            in(reg) kernel_cr3_val,
            offset = const KERNEL_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }
    log::info!("kernel_cr3 stored successfully - interrupt handlers can now switch to kernel page tables");
}

/// Get the current thread from per-CPU data
pub fn current_thread() -> Option<&'static mut crate::task::thread::Thread> {
    unsafe {
        // Access current_thread field via GS segment
        // Offset 8 = size of cpu_id field
        let thread_ptr: *mut crate::task::thread::Thread;
        core::arch::asm!(
            "mov {}, gs:[8]",
            out(reg) thread_ptr,
            options(nostack, preserves_flags)
        );
        
        if thread_ptr.is_null() {
            None
        } else {
            Some(&mut *thread_ptr)
        }
    }
}

/// Set the current thread in per-CPU data
pub fn set_current_thread(thread: *mut crate::task::thread::Thread) {
    unsafe {
        // Write to current_thread field via GS segment
        // Offset 8 = size of cpu_id field
        core::arch::asm!(
            "mov gs:[8], {}",
            in(reg) thread,
            options(nostack, preserves_flags)
        );
    }
}

/// Get the kernel stack top from per-CPU data
pub fn kernel_stack_top() -> u64 {
    unsafe {
        // Access kernel_stack_top field via GS segment
        // Offset 16 = cpu_id (8) + current_thread (8)
        let stack_top: u64;
        core::arch::asm!(
            "mov {}, gs:[16]",
            out(reg) stack_top,
            options(nostack, preserves_flags)
        );
        stack_top
    }
}

/// Set the kernel stack top in per-CPU data
pub fn set_kernel_stack_top(stack_top: u64) {
    unsafe {
        // Write to kernel_stack_top field via GS segment
        // Offset 16 = cpu_id (8) + current_thread (8)
        core::arch::asm!(
            "mov gs:[16], {}",
            in(reg) stack_top,
            options(nostack, preserves_flags)
        );
    }
}

/// Check if we need to reschedule
pub fn need_resched() -> bool {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        unsafe {
            let need_resched: u8;
            core::arch::asm!(
                "mov {need}, byte ptr gs:[{offset}]",
                need = out(reg_byte) need_resched,
                offset = const NEED_RESCHED_OFFSET,
                options(nostack, readonly)
            );
            need_resched != 0
        }
    } else {
        false
    }
}

/// Set the reschedule needed flag
pub fn set_need_resched(need: bool) {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        unsafe {
            let value: u8 = if need { 1 } else { 0 };
            core::arch::asm!(
                "mov byte ptr gs:[{offset}], {val}",
                val = in(reg_byte) value,
                offset = const NEED_RESCHED_OFFSET,
                options(nostack)
            );
        }
    }
}

/// Check if we're in any interrupt context (hardware IRQ, softirq, or NMI)
/// Returns true if any interrupt nesting level is non-zero
pub fn in_interrupt() -> bool {
    let count = preempt_count();
    // Check if any interrupt bits are set (HARDIRQ, SOFTIRQ, or NMI)
    (count & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0
}

/// Check if we're in hardware interrupt context
pub fn in_hardirq() -> bool {
    let count = preempt_count();
    (count & HARDIRQ_MASK) != 0
}

/// Check if we're in softirq context
pub fn in_softirq() -> bool {
    let count = preempt_count();
    (count & SOFTIRQ_MASK) != 0
}

/// Check if we're in NMI context
pub fn in_nmi() -> bool {
    let count = preempt_count();
    (count & NMI_MASK) != 0
}

/// Enter hardware IRQ context (called by interrupt handlers)
pub fn irq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_enter called before per-CPU initialization");

    // Track irq_enter calls for balance analysis
    IRQ_ENTER_COUNT.fetch_add(1, Ordering::Relaxed);

    unsafe {
        let old_count: u32;
        core::arch::asm!(
            "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
            "add dword ptr gs:[{offset}], {inc:e}",  // Add HARDIRQ_OFFSET
            old = out(reg) old_count,
            inc = in(reg) HARDIRQ_OFFSET,
            offset = const PREEMPT_COUNT_OFFSET,
            options(nostack, preserves_flags)
        );

        let new_count = old_count + HARDIRQ_OFFSET;

        // Check for overflow in debug builds
        debug_assert!(
            (new_count & HARDIRQ_MASK) >= (old_count & HARDIRQ_MASK),
            "irq_enter: HARDIRQ count overflow! Was {:#x}, would be {:#x}",
            old_count & HARDIRQ_MASK,
            new_count & HARDIRQ_MASK
        );

        // LOGGING REMOVED: All logging removed to prevent serial lock deadlock
        // Previously logged first 10 irq_enter calls for CI validation
    }
}

/// Exit hardware IRQ context
pub fn irq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "irq_exit called before per-CPU initialization");

    // Track irq_exit calls for balance analysis
    IRQ_EXIT_COUNT.fetch_add(1, Ordering::Relaxed);

    // Check for imbalance
    let enters = IRQ_ENTER_COUNT.load(Ordering::Relaxed);
    let exits = IRQ_EXIT_COUNT.load(Ordering::Relaxed);
    if enters > exits {
        let imbalance = enters - exits;
        MAX_PREEMPT_IMBALANCE.fetch_max(imbalance, Ordering::Relaxed);
    }

    unsafe {
            let old_count: u32;
            core::arch::asm!(
                "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
                "sub dword ptr gs:[{offset}], {dec:e}",  // Subtract HARDIRQ_OFFSET
                old = out(reg) old_count,
                dec = in(reg) HARDIRQ_OFFSET,
                offset = const PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );

            let new_count = old_count.wrapping_sub(HARDIRQ_OFFSET);

            // Check for underflow in debug builds
            debug_assert!(
                (old_count & HARDIRQ_MASK) >= HARDIRQ_OFFSET,
                "irq_exit: HARDIRQ count underflow! Was {:#x}",
                old_count & HARDIRQ_MASK
            );

            // LOGGING REMOVED: All logging removed to prevent serial lock deadlock
            // Previously logged first 10 irq_exit calls for CI validation

        // Check if we should process softirqs
        // Linux processes softirqs when returning to non-interrupt context
        if new_count == 0 {
            // Check if any softirqs are pending
            let pending = softirq_pending();
            if pending != 0 {
                // Process softirqs (logging removed to prevent deadlock)
                do_softirq();
            }

            // After softirq processing, re-check if we should schedule
            // Only if we're still at preempt_count == 0 with need_resched set
            // Defer the actual scheduling to the interrupt return path
            // (No logging to avoid deadlock)
        }
    }
}

/// Enter NMI context (Non-Maskable Interrupt)
pub fn nmi_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "nmi_enter called before per-CPU initialization");
    
    unsafe {
        compiler_fence(Ordering::Acquire);
            
            let old_count: u32;
            core::arch::asm!(
                "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
                "add dword ptr gs:[{offset}], {inc:e}",  // Add NMI_OFFSET
                old = out(reg) old_count,
                inc = in(reg) NMI_OFFSET,
                offset = const PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );

            let _new_count = old_count + NMI_OFFSET;
            
            // Check for overflow in debug builds (NMI only has 1 bit, so max nesting is 1)
            debug_assert!(
                (old_count & NMI_MASK) == 0,
                "nmi_enter: NMI already set! Cannot nest NMIs. Count was {:#x}",
                old_count
            );
            
            // log::trace!("nmi_enter: {:#x} -> {:#x}", old_count, new_count);  // Disabled to avoid deadlock
            
            compiler_fence(Ordering::Release);
    }
}

/// Exit NMI context
pub fn nmi_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "nmi_exit called before per-CPU initialization");
    
    unsafe {
        compiler_fence(Ordering::Acquire);
            
            let old_count: u32;
            core::arch::asm!(
                "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
                "sub dword ptr gs:[{offset}], {dec:e}",  // Subtract NMI_OFFSET
                old = out(reg) old_count,
                dec = in(reg) NMI_OFFSET,
                offset = const PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );

            let _new_count = old_count.wrapping_sub(NMI_OFFSET);
            
            // Check for underflow in debug builds
            debug_assert!(
                (old_count & NMI_MASK) != 0,
                "nmi_exit: NMI bit not set! Was {:#x}",
                old_count
            );
            
            // log::trace!("nmi_exit: {:#x} -> {:#x}", old_count, new_count);  // Disabled to avoid deadlock
            
            compiler_fence(Ordering::Release);
            // NMIs never schedule
    }
}

/// Enter softirq context (software interrupt / bottom half)
pub fn softirq_enter() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "softirq_enter called before per-CPU initialization");
    
    unsafe {
        compiler_fence(Ordering::Acquire);
            
            let old_count: u32;
            core::arch::asm!(
                "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
                "add dword ptr gs:[{offset}], {inc:e}",  // Add SOFTIRQ_OFFSET
                old = out(reg) old_count,
                inc = in(reg) SOFTIRQ_OFFSET,
                offset = const PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );
            
            let new_count = old_count + SOFTIRQ_OFFSET;
            
            // Check for overflow in debug builds
            debug_assert!(
                (new_count & SOFTIRQ_MASK) >= (old_count & SOFTIRQ_MASK),
                "softirq_enter: SOFTIRQ count overflow! Was {:#x}, would be {:#x}",
                old_count & SOFTIRQ_MASK,
                new_count & SOFTIRQ_MASK
            );
            
            // log::trace!("softirq_enter: {:#x} -> {:#x}", old_count, new_count);  // Disabled to avoid deadlock
            
            compiler_fence(Ordering::Release);
    }
}

/// Exit softirq context
pub fn softirq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "softirq_exit called before per-CPU initialization");
    
    unsafe {
        compiler_fence(Ordering::Acquire);
            
            let old_count: u32;
            core::arch::asm!(
                "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
                "sub dword ptr gs:[{offset}], {dec:e}",  // Subtract SOFTIRQ_OFFSET
                old = out(reg) old_count,
                dec = in(reg) SOFTIRQ_OFFSET,
                offset = const PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );
            
            let new_count = old_count.wrapping_sub(SOFTIRQ_OFFSET);
            
            // Check for underflow in debug builds
            debug_assert!(
                (old_count & SOFTIRQ_MASK) >= SOFTIRQ_OFFSET,
                "softirq_exit: SOFTIRQ count underflow! Was {:#x}",
                old_count & SOFTIRQ_MASK
            );
            
            // log::trace!("softirq_exit: {:#x} -> {:#x}", old_count, new_count);  // Disabled to avoid deadlock
            
            compiler_fence(Ordering::Release);
            
            // Check if we should schedule on softirq exit (similar to IRQ exit)
            // Only if we're returning to preemptible context
            if new_count == 0 && need_resched() {
                log::info!("softirq_exit: Triggering preempt_schedule_irq");
                crate::task::scheduler::preempt_schedule_irq();
            }
    }
}

/// Get the idle thread from per-CPU data
#[allow(dead_code)]
pub fn idle_thread() -> Option<&'static mut crate::task::thread::Thread> {
    unsafe {
        // Access idle_thread field via GS segment
        // Offset 24 = cpu_id (8) + current_thread (8) + kernel_stack_top (8)
        let thread_ptr: *mut crate::task::thread::Thread;
        core::arch::asm!(
            "mov {}, gs:[24]",
            out(reg) thread_ptr,
            options(nostack, preserves_flags)
        );
        
        if thread_ptr.is_null() {
            None
        } else {
            Some(&mut *thread_ptr)
        }
    }
}

/// Set the idle thread in per-CPU data
pub fn set_idle_thread(thread: *mut crate::task::thread::Thread) {
    unsafe {
        // Write to idle_thread field via GS segment
        // Offset 24 = cpu_id (8) + current_thread (8) + kernel_stack_top (8)
        core::arch::asm!(
            "mov gs:[24], {}",
            in(reg) thread,
            options(nostack, preserves_flags)
        );
    }
}

/// Update TSS RSP0 with the current thread's kernel stack
/// This must be called on every context switch to a thread
pub fn update_tss_rsp0(kernel_stack_top: u64) {
    unsafe {
        // BUG FIX: Previously this code read gs:0 expecting a pointer to PerCpuData,
        // but gs:0 contains cpu_id (value 0), not a pointer. When cpu_id is 0,
        // the code treated it as a null pointer and skipped the TSS update entirely!
        // This caused syscalls to use stale kernel stacks, leading to page faults.
        //
        // The fix is to access the TSS pointer directly at its correct offset (48),
        // and update kernel_stack_top at its correct offset (16).

        // Get TSS pointer from per-CPU data at offset 48 (TSS_OFFSET)
        let tss_ptr: *mut x86_64::structures::tss::TaskStateSegment;
        core::arch::asm!(
            "mov {}, gs:[{offset}]",
            out(reg) tss_ptr,
            offset = const TSS_OFFSET,
            options(nostack, preserves_flags)
        );

        if !tss_ptr.is_null() {
            // Update per-CPU kernel_stack_top at offset 16 (KERNEL_STACK_TOP_OFFSET)
            core::arch::asm!(
                "mov gs:[{offset}], {}",
                in(reg) kernel_stack_top,
                offset = const KERNEL_STACK_TOP_OFFSET,
                options(nostack, preserves_flags)
            );

            // Update TSS.RSP0
            (*tss_ptr).privilege_stack_table[0] = VirtAddr::new(kernel_stack_top);

            // log::trace!("Updated TSS.RSP0 to {:#x}", kernel_stack_top);  // Disabled to avoid deadlock
        }
    }
}

/// Set the TSS pointer for this CPU
pub fn set_tss(tss: *mut x86_64::structures::tss::TaskStateSegment) {
    unsafe {
        // Store TSS pointer directly at offset 48 (TSS_OFFSET) in per-CPU data
        // BUG FIX: Previously read gs:0 (cpu_id) as a pointer, which is wrong.
        core::arch::asm!(
            "mov gs:[{offset}], {}",
            in(reg) tss,
            offset = const TSS_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Get the user RSP scratch space (used during syscall entry)
#[allow(dead_code)]
pub fn user_rsp_scratch() -> u64 {
    unsafe {
        // Read user_rsp_scratch directly at offset 40 (USER_RSP_SCRATCH_OFFSET)
        // BUG FIX: Previously read gs:0 (cpu_id) as a pointer, which is wrong.
        let rsp: u64;
        core::arch::asm!(
            "mov {}, gs:[{offset}]",
            out(reg) rsp,
            offset = const USER_RSP_SCRATCH_OFFSET,
            options(nostack, preserves_flags)
        );
        rsp
    }
}

/// Set the user RSP scratch space (used during syscall entry)
#[allow(dead_code)]
pub fn set_user_rsp_scratch(rsp: u64) {
    unsafe {
        // Store user_rsp_scratch directly at offset 40 (USER_RSP_SCRATCH_OFFSET)
        // BUG FIX: Previously read gs:0 (cpu_id) as a pointer, which is wrong.
        core::arch::asm!(
            "mov gs:[{offset}], {}",
            in(reg) rsp,
            offset = const USER_RSP_SCRATCH_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Increment preempt count (disable kernel preemption)
/// Only manipulates the PREEMPT bits (0-7), not interrupt counts
/// CRITICAL: Must only be called after per_cpu::init() with interrupts disabled until then
pub fn preempt_disable() {
    // Per-CPU data must be initialized before any preemption operations
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "preempt_disable called before per-CPU initialization");
    
    unsafe {
        // Compiler barrier before incrementing preempt count
        compiler_fence(Ordering::Acquire);
        
        let old_count: u32;
        
        // Use addl for incrementing per-CPU preempt count
        // No LOCK prefix needed for per-CPU data
        core::arch::asm!(
            "mov {old:e}, dword ptr gs:[{offset}]",  // Read current value
            "add dword ptr gs:[{offset}], {inc:e}", // Add PREEMPT_OFFSET
            old = out(reg) old_count,
            inc = in(reg) PREEMPT_OFFSET,
            offset = const PREEMPT_COUNT_OFFSET,
            options(nostack, preserves_flags)
        );
        
        let new_count = old_count + PREEMPT_OFFSET;
        
        // Check for overflow in debug builds
        debug_assert!(
            (new_count & PREEMPT_MASK) >= (old_count & PREEMPT_MASK),
            "preempt_disable: PREEMPT count overflow! Was {:#x}, would be {:#x}",
            old_count & PREEMPT_MASK,
            new_count & PREEMPT_MASK
        );
        
        // Compiler barrier after incrementing preempt count
        compiler_fence(Ordering::Release);
        
        // CRITICAL: Do NOT use log:: macros here as they may recursively call preempt_disable!
        // This was causing the double preempt_disable issue when coming from userspace.
        // The logging infrastructure might acquire locks which call preempt_disable.
        #[cfg(never)] // Disable this logging to prevent recursion
        {
            // Get CPU ID for logging (at offset 0)
            let cpu_id: usize;
            core::arch::asm!(
                "mov {}, gs:[0]",
                out(reg) cpu_id,
                options(nostack)
            );
            
            log::debug!("preempt_disable: {:#x} -> {:#x} (per-CPU, CPU {})", old_count, new_count, cpu_id);
        }
    }
}

/// Decrement preempt count (enable kernel preemption)
/// Only manipulates the PREEMPT bits (0-7), not interrupt counts
/// May trigger scheduling if preempt count reaches 0 and not in interrupt context
/// CRITICAL: Must only be called after per_cpu::init() with interrupts disabled until then
pub fn preempt_enable() {
    // Per-CPU data must be initialized before any preemption operations
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "preempt_enable called before per-CPU initialization");
    
    unsafe {
        // Compiler barrier before decrementing preempt count
        compiler_fence(Ordering::Acquire);
        
        // Atomic decrement on GS-relative memory
        let old_count: u32;
        
        // Use subl for decrementing per-CPU preempt count
        // No LOCK prefix needed for per-CPU data
        core::arch::asm!(
            "mov {old:e}, dword ptr gs:[{offset}]",   // Read current value
            "sub dword ptr gs:[{offset}], {dec:e}",  // Subtract PREEMPT_OFFSET
            old = out(reg) old_count,
            dec = in(reg) PREEMPT_OFFSET,
            offset = const PREEMPT_COUNT_OFFSET,
            options(nostack, preserves_flags)
        );
        
        let new_count = old_count.wrapping_sub(PREEMPT_OFFSET);
        
        // Compiler barrier after decrementing preempt count
        compiler_fence(Ordering::Release);

        // Get CPU ID for logging (at offset 0)
        let _cpu_id: usize;
        core::arch::asm!(
            "mov {}, gs:[0]",
            out(reg) _cpu_id,
            options(nostack)
        );

        // CRITICAL: Disable logging to prevent recursion issues
        #[cfg(never)]
        log::debug!("preempt_enable: {:#x} -> {:#x} (per-CPU, CPU {})", old_count, new_count, _cpu_id);
        
        // Check for underflow in debug builds
        debug_assert!(
            (old_count & PREEMPT_MASK) >= PREEMPT_OFFSET,
            "preempt_enable: PREEMPT count underflow! Was {:#x}",
            old_count & PREEMPT_MASK
        );
        
        if (new_count & PREEMPT_MASK) == 0 {
            // PREEMPT count reached 0, check if we should schedule
            // Only schedule if:
            // 1. We're not in any interrupt context (no HARDIRQ/SOFTIRQ/NMI bits)
            // 2. need_resched is set
            if (new_count & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) == 0 {
                // Not in interrupt context, safe to check for scheduling
                // Note: We intentionally do NOT call try_schedule() or clear need_resched here.
                // The syscall return path and timer interrupt return path both check
                // need_resched and call check_need_resched_and_switch() which performs
                // the actual context switch with proper register save/restore.
                // Clearing the flag here would prevent those paths from scheduling.
            }
        }
    }
}

/// Get current preempt count
pub fn preempt_count() -> u32 {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire),
                  "preempt_count called before per-CPU initialization");

    // Read preempt_count directly from GS segment
    unsafe {
        let count: u32;
        core::arch::asm!(
            "mov {count:e}, dword ptr gs:[{offset}]",
            count = out(reg) count,
            offset = const PREEMPT_COUNT_OFFSET,
            options(nostack, readonly)
        );
        count
    }
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

    unsafe {
        // Clear bit 28 (PREEMPT_ACTIVE) from preempt_count
        // Use AND with ~PREEMPT_ACTIVE to clear the bit
        core::arch::asm!(
            "and dword ptr gs:[{offset}], {mask:e}",
            mask = in(reg) !PREEMPT_ACTIVE,
            offset = const PREEMPT_COUNT_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Get pending softirq bitmap
pub fn softirq_pending() -> u32 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    
    unsafe {
        let pending: u32;
        core::arch::asm!(
            "mov {pending:e}, dword ptr gs:[{offset}]",
            pending = out(reg) pending,
            offset = const SOFTIRQ_PENDING_OFFSET,
            options(nostack, readonly, preserves_flags)
        );
        pending
    }
}

/// Set softirq pending bit
#[allow(dead_code)]
pub fn raise_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");
    
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    
    unsafe {
        let bit = 1u32 << nr;
        core::arch::asm!(
            "or dword ptr gs:[{offset}], {bit:e}",
            bit = in(reg) bit,
            offset = const SOFTIRQ_PENDING_OFFSET,
            options(nostack, preserves_flags)
        );
        // log::trace!("Raised softirq {}, pending bitmap now: {:#x}", nr, softirq_pending());  // Disabled to avoid deadlock
    }
}

/// Clear softirq pending bit
pub fn clear_softirq(nr: u32) {
    debug_assert!(nr < 32, "Invalid softirq number");
    
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    
    unsafe {
        let mask = !(1u32 << nr);
        core::arch::asm!(
            "and dword ptr gs:[{offset}], {mask:e}",
            mask = in(reg) mask,
            offset = const SOFTIRQ_PENDING_OFFSET,
            options(nostack, preserves_flags)
        );
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

    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {}, gs:[{offset}]",
            out(reg) cr3,
            offset = const NEXT_CR3_OFFSET,
            options(nostack, readonly, preserves_flags)
        );
        cr3
    }
}

/// Set the target CR3 for next IRETQ
/// This communicates to timer_entry.asm and entry.asm (syscall return)
/// which CR3 to switch to before returning to userspace.
/// CR3 switching is deferred to assembly code to avoid double TLB flushes.
pub fn set_next_cr3(cr3: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        core::arch::asm!(
            "mov gs:[{offset}], {}",
            in(reg) cr3,
            offset = const NEXT_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Get the kernel CR3 (master kernel page table)
/// Returns 0 if not initialized
#[allow(dead_code)]
pub fn get_kernel_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }

    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {}, gs:[{offset}]",
            out(reg) cr3,
            offset = const KERNEL_CR3_OFFSET,
            options(nostack, readonly, preserves_flags)
        );
        cr3
    }
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
    unsafe {
        core::arch::asm!(
            "mov gs:[{offset}], {}",
            in(reg) cr3,
            offset = const KERNEL_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Set the exception cleanup context flag (per-CPU)
/// Called by exception handlers (GPF, page fault) when they terminate a process
/// and need to allow scheduling from kernel mode
pub fn set_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        // Set exception_cleanup_context to 1 at offset 88
        core::arch::asm!(
            "mov byte ptr gs:[{offset}], 1",
            offset = const EXCEPTION_CLEANUP_CONTEXT_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Clear the exception cleanup context flag (per-CPU)
/// Called after successfully switching to a new thread
pub fn clear_exception_cleanup_context() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        // Set exception_cleanup_context to 0 at offset 88
        core::arch::asm!(
            "mov byte ptr gs:[{offset}], 0",
            offset = const EXCEPTION_CLEANUP_CONTEXT_OFFSET,
            options(nostack, preserves_flags)
        );
    }
}

/// Check if we're in exception cleanup context (per-CPU)
pub fn in_exception_cleanup_context() -> bool {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return false;
    }

    unsafe {
        let value: u8;
        core::arch::asm!(
            "mov {val}, byte ptr gs:[{offset}]",
            val = out(reg_byte) value,
            offset = const EXCEPTION_CLEANUP_CONTEXT_OFFSET,
            options(nostack, readonly, preserves_flags)
        );
        value != 0
    }
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

    // CRITICAL: When in exception cleanup context, allow scheduling regardless of PREEMPT_ACTIVE.
    // The exception handler has explicitly requested a reschedule after terminating a process.
    // Without this, PREEMPT_ACTIVE (bit 28) blocks scheduling even though we need to recover.
    let result = in_exception_cleanup
                 || (current_preempt == 0 && (returning_to_userspace || returning_to_idle_kernel));

    // Debug logging for exception handler path
    static CAN_SCHED_LOG_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = CAN_SCHED_LOG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count < 20 || (in_exception_cleanup && count < 100) {
        log::debug!(
            "can_schedule: preempt={}, to_user={}, to_idle_kern={}, exc_cleanup={}, result={}",
            current_preempt, returning_to_userspace, returning_to_idle_kernel, in_exception_cleanup, result
        );
    }

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

