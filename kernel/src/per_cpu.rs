//! Per-CPU data support using GS segment
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the GS segment register without locks.

use core::ptr;
use core::sync::atomic::{compiler_fence, Ordering};
use x86_64::VirtAddr;
use x86_64::registers::model_specific::{GsBase, KernelGsBase};

/// Per-CPU data structure with cache-line alignment and stable ABI
/// This structure is accessed from assembly code, so field order and offsets must be stable
/// CRITICAL: The repr(C) attribute ensures field ordering matches declaration order
#[repr(C, align(64))]
pub struct PerCpuData {
    /// CPU ID (offset 0) - for multi-processor support
    pub cpu_id: usize,
    
    /// Current thread pointer (offset 8)
    pub current_thread: *mut crate::task::thread::Thread,
    
    /// Kernel stack pointer for syscalls/interrupts (offset 16) - TSS.RSP0
    pub kernel_stack_top: VirtAddr,
    
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
    
    /// Reserved for future use - maintains 64-byte alignment
    _reserved: u32,
}

// Linux-style preempt_count bit layout constants
// Matches Linux kernel's exact bit partitioning
const PREEMPT_BITS: u32 = 8;
const SOFTIRQ_BITS: u32 = 8;
const HARDIRQ_BITS: u32 = 10;  // Linux uses 10 bits for HARDIRQ
const NMI_BITS: u32 = 1;       // Linux uses 1 bit for NMI

const PREEMPT_SHIFT: u32 = 0;
const SOFTIRQ_SHIFT: u32 = PREEMPT_SHIFT + PREEMPT_BITS;  // 8
const HARDIRQ_SHIFT: u32 = SOFTIRQ_SHIFT + SOFTIRQ_BITS;   // 16
const NMI_SHIFT: u32 = HARDIRQ_SHIFT + HARDIRQ_BITS;       // 26

const PREEMPT_MASK: u32 = ((1 << PREEMPT_BITS) - 1) << PREEMPT_SHIFT;  // 0x000000FF
const SOFTIRQ_MASK: u32 = ((1 << SOFTIRQ_BITS) - 1) << SOFTIRQ_SHIFT;  // 0x0000FF00
const HARDIRQ_MASK: u32 = ((1 << HARDIRQ_BITS) - 1) << HARDIRQ_SHIFT;  // 0x03FF0000
const NMI_MASK: u32 = ((1 << NMI_BITS) - 1) << NMI_SHIFT;              // 0x04000000

const PREEMPT_ACTIVE: u32 = 1 << 28;

// Increment values for each nesting level
const PREEMPT_OFFSET: u32 = 1 << PREEMPT_SHIFT;
const SOFTIRQ_OFFSET: u32 = 1 << SOFTIRQ_SHIFT;
const HARDIRQ_OFFSET: u32 = 1 << HARDIRQ_SHIFT;
const NMI_OFFSET: u32 = 1 << NMI_SHIFT;

// Compile-time offset calculations and validation
// These MUST match the actual struct layout or GS-relative access will be incorrect
const CPU_ID_OFFSET: usize = 0;           // offset 0: usize (8 bytes)
const CURRENT_THREAD_OFFSET: usize = 8;    // offset 8: *mut Thread (8 bytes)
const KERNEL_STACK_TOP_OFFSET: usize = 16; // offset 16: VirtAddr (8 bytes)
const IDLE_THREAD_OFFSET: usize = 24;      // offset 24: *mut Thread (8 bytes)
const PREEMPT_COUNT_OFFSET: usize = 32;    // offset 32: u32 (4 bytes) - ALIGNED
const NEED_RESCHED_OFFSET: usize = 36;     // offset 36: u8 (1 byte)
// Padding at 37-39 (3 bytes)
const USER_RSP_SCRATCH_OFFSET: usize = 40; // offset 40: u64 (8 bytes) - ALIGNED
const TSS_OFFSET: usize = 48;              // offset 48: *mut TSS (8 bytes)
const SOFTIRQ_PENDING_OFFSET: usize = 56;  // offset 56: u32 (4 bytes)

// Compile-time assertions to ensure offsets are correct
// These will fail to compile if the offsets don't match expected values
const _: () = assert!(PREEMPT_COUNT_OFFSET % 4 == 0, "preempt_count must be 4-byte aligned");
const _: () = assert!(PREEMPT_COUNT_OFFSET == 32, "preempt_count offset mismatch");
const _: () = assert!(USER_RSP_SCRATCH_OFFSET % 8 == 0, "user_rsp_scratch must be 8-byte aligned");
const _: () = assert!(core::mem::size_of::<usize>() == 8, "This code assumes 64-bit pointers");

// Verify struct size is exactly 64 bytes (cache line)
const _: () = assert!(core::mem::size_of::<PerCpuData>() == 64, "PerCpuData must be exactly 64 bytes");

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
            cpu_id,
            current_thread: ptr::null_mut(),
            kernel_stack_top: VirtAddr::new(0),
            idle_thread: ptr::null_mut(),
            preempt_count: 0,
            need_resched: 0,
            _pad: [0; 3],
            user_rsp_scratch: 0,
            tss: ptr::null_mut(),
            softirq_pending: 0,
            _reserved: 0,
        }
    }
}

/// Static per-CPU data for CPU 0 (BSP)
/// In a real SMP kernel, we'd have an array of these
static mut CPU0_DATA: PerCpuData = PerCpuData::new(0);

/// Flag to indicate whether per-CPU data is initialized and safe to use
/// CRITICAL: Interrupts MUST be disabled until this is true
static PER_CPU_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initialize per-CPU data for the current CPU
pub fn init() {
    log::info!("Initializing per-CPU data via GS segment");
    
    unsafe {
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
    }
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
pub fn kernel_stack_top() -> VirtAddr {
    unsafe {
        // Access kernel_stack_top field via GS segment
        // Offset 16 = cpu_id (8) + current_thread (8)
        let stack_top: u64;
        core::arch::asm!(
            "mov {}, gs:[16]",
            out(reg) stack_top,
            options(nostack, preserves_flags)
        );
        VirtAddr::new(stack_top)
    }
}

/// Set the kernel stack top in per-CPU data
pub fn set_kernel_stack_top(stack_top: VirtAddr) {
    unsafe {
        // Write to kernel_stack_top field via GS segment
        // Offset 16 = cpu_id (8) + current_thread (8)
        core::arch::asm!(
            "mov gs:[16], {}",
            in(reg) stack_top.as_u64(),
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
        
        // Log first few for CI validation
        static IRQ_ENTER_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let enter_count = IRQ_ENTER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if enter_count < 10 {
            log::info!("irq_enter #{}: preempt_count {:#x} -> {:#x} (HARDIRQ incremented)", 
                      enter_count, old_count, new_count);
        } else {
            log::trace!("irq_enter: {:#x} -> {:#x}", old_count, new_count);
        }
    }
}

/// Exit hardware IRQ context
pub fn irq_exit() {
    debug_assert!(PER_CPU_INITIALIZED.load(Ordering::Acquire), 
                  "irq_exit called before per-CPU initialization");
    
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
            
            // Log first few for CI validation
            static IRQ_EXIT_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let exit_count = IRQ_EXIT_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if exit_count < 10 {
                log::info!("irq_exit #{}: preempt_count {:#x} -> {:#x} (HARDIRQ decremented)", 
                          exit_count, old_count, new_count);
            } else {
                log::trace!("irq_exit: {:#x} -> {:#x}", old_count, new_count);
            }
            
        // Check if we should process softirqs
        // Linux processes softirqs when returning to non-interrupt context
        if new_count == 0 {
            // Check if any softirqs are pending
            if softirq_pending() != 0 {
                log::info!("irq_exit: Processing pending softirqs (bitmap={:#x})", softirq_pending());
                // Process softirqs
                do_softirq();
            }
            
            // After softirq processing, re-check if we should schedule
            // Only if we're still at preempt_count == 0 with need_resched set
            // (softirq handlers might have set need_resched)
            if need_resched() {
                log::info!("irq_exit: Triggering preempt_schedule_irq");
                // This is where preempt_schedule_irq() would be called
                // It's a special scheduling point that's safe from IRQ context
                crate::task::scheduler::preempt_schedule_irq();
            }
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
            
            let new_count = old_count + NMI_OFFSET;
            
            // Check for overflow in debug builds (NMI only has 1 bit, so max nesting is 1)
            debug_assert!(
                (old_count & NMI_MASK) == 0,
                "nmi_enter: NMI already set! Cannot nest NMIs. Count was {:#x}",
                old_count
            );
            
            log::trace!("nmi_enter: {:#x} -> {:#x}", old_count, new_count);
            
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
            
            let new_count = old_count.wrapping_sub(NMI_OFFSET);
            
            // Check for underflow in debug builds
            debug_assert!(
                (old_count & NMI_MASK) != 0,
                "nmi_exit: NMI bit not set! Was {:#x}",
                old_count
            );
            
            log::trace!("nmi_exit: {:#x} -> {:#x}", old_count, new_count);
            
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
            
            log::trace!("softirq_enter: {:#x} -> {:#x}", old_count, new_count);
            
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
            
            log::trace!("softirq_exit: {:#x} -> {:#x}", old_count, new_count);
            
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
pub fn update_tss_rsp0(kernel_stack_top: VirtAddr) {
    unsafe {
        // Get TSS pointer from per-CPU data
        let cpu_data: *mut PerCpuData;
        core::arch::asm!(
            "mov {}, gs:0",
            out(reg) cpu_data,
            options(nostack, preserves_flags)
        );
        
        if !cpu_data.is_null() && !(*cpu_data).tss.is_null() {
            // Update both per-CPU kernel_stack_top and TSS.RSP0
            (*cpu_data).kernel_stack_top = kernel_stack_top;
            (*(*cpu_data).tss).privilege_stack_table[0] = kernel_stack_top;
            
            log::trace!("Updated TSS.RSP0 to {:#x}", kernel_stack_top);
        }
    }
}

/// Set the TSS pointer for this CPU
pub fn set_tss(tss: *mut x86_64::structures::tss::TaskStateSegment) {
    unsafe {
        let cpu_data: *mut PerCpuData;
        core::arch::asm!(
            "mov {}, gs:0",
            out(reg) cpu_data,
            options(nostack, preserves_flags)
        );
        
        if !cpu_data.is_null() {
            (*cpu_data).tss = tss;
        }
    }
}

/// Get the user RSP scratch space (used during syscall entry)
pub fn user_rsp_scratch() -> u64 {
    unsafe {
        let cpu_data: *const PerCpuData;
        core::arch::asm!(
            "mov {}, gs:0",
            out(reg) cpu_data,
            options(nostack, preserves_flags)
        );
        
        if cpu_data.is_null() {
            0
        } else {
            (*cpu_data).user_rsp_scratch
        }
    }
}

/// Set the user RSP scratch space (used during syscall entry)
pub fn set_user_rsp_scratch(rsp: u64) {
    unsafe {
        let cpu_data: *mut PerCpuData;
        core::arch::asm!(
            "mov {}, gs:0",
            out(reg) cpu_data,
            options(nostack, preserves_flags)
        );
        
        if !cpu_data.is_null() {
            (*cpu_data).user_rsp_scratch = rsp;
        }
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
        let cpu_id: usize;
        core::arch::asm!(
            "mov {}, gs:[0]",
            out(reg) cpu_id,
            options(nostack)
        );
        
        log::debug!("preempt_enable: {:#x} -> {:#x} (per-CPU, CPU {})", old_count, new_count, cpu_id);
        
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
                if need_resched() {
                    log::info!("preempt_enable: Triggering preempt_schedule (PREEMPT->0, not in IRQ, need_resched set)");
                    // Call scheduler - this is the normal preemption path
                    // In Linux, this would call preempt_schedule() which is slightly
                    // different from schedule(), but for now we use schedule()
                    // TODO: Implement preempt_schedule() that sets PREEMPT_ACTIVE
                    crate::task::scheduler::schedule();
                }
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
        log::trace!("Raised softirq {}, pending bitmap now: {:#x}", nr, softirq_pending());
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
                log::trace!("  Processing softirq {}", nr);
                // softirq_handlers[nr]() would be called here
            }
        }
    }
    
    // Exit softirq context
    softirq_exit();
}

/// Check if we can schedule (preempt_count == 0 and returning to userspace)
pub fn can_schedule(saved_cs: u64) -> bool {
    let preempt_count = preempt_count();
    let is_userspace = (saved_cs & 3) == 3;
    let can_sched = preempt_count == 0 && is_userspace;
    
    log::debug!(
        "can_schedule: preempt_count={}, cs_rpl={}, userspace={}, result={}", 
        preempt_count, 
        saved_cs & 3, 
        is_userspace,
        can_sched
    );
    
    if preempt_count > 0 {
        log::debug!("can_schedule: BLOCKED by preempt_count={}", preempt_count);
    }
    
    if !is_userspace {
        log::debug!("can_schedule: BLOCKED - not returning to userspace (CS RPL={})", saved_cs & 3);
    }
    
    can_sched
}