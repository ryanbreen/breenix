//! Per-CPU data support using GS segment
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the GS segment register without locks.
//!
//! Architecture-specific operations (GS-relative memory access, MSR operations)
//! are delegated to the HAL's per-CPU module.

use core::mem::offset_of;
use core::ptr;
use core::sync::atomic::{compiler_fence, AtomicU64, Ordering};
use x86_64::VirtAddr;

// Import HAL per-CPU operations and traits
use crate::arch_impl::current::percpu as hal_percpu;
use crate::arch_impl::PerCpuOps;

// Import HAL constants - single source of truth for per-CPU offsets
use crate::arch_impl::current::constants::{
    DISPATCH_MARK_INVALID, DISPATCH_MARK_VALID, PERCPU_CPU_ID_OFFSET,
    PERCPU_CURRENT_THREAD_OFFSET, PERCPU_DISPATCH_MARK_RIP_OFFSET, PERCPU_DISPATCH_MARK_RSP_OFFSET,
    PERCPU_DISPATCH_MARK_STATE_OFFSET, PERCPU_DISPATCH_MARK_TID_OFFSET,
    PERCPU_DISPATCH_MARK_WAIT_ITERS_OFFSET,
    PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET, PERCPU_IDLE_THREAD_OFFSET, PERCPU_KERNEL_CR3_OFFSET,
    PERCPU_KERNEL_STACK_TOP_OFFSET, PERCPU_NEED_RESCHED_OFFSET, PERCPU_NEXT_CR3_OFFSET,
    PERCPU_PREEMPT_COUNT_OFFSET, PERCPU_SAVED_PROCESS_CR3_OFFSET, PERCPU_SOFTIRQ_PENDING_OFFSET,
    PERCPU_TSS_OFFSET, PERCPU_USER_RSP_SCRATCH_OFFSET,
};

// Global tracking counters for irq_enter/irq_exit balance analysis
static IRQ_ENTER_COUNT: AtomicU64 = AtomicU64::new(0);
static IRQ_EXIT_COUNT: AtomicU64 = AtomicU64::new(0);
static MAX_PREEMPT_IMBALANCE: AtomicU64 = AtomicU64::new(0);
// Fail-closed guard for the preempt bracket (#672). preempt_disable() and
// preempt_enable() are one protocol: an enable with no matching disable is a
// protocol violation, not a number to wrap. The x86 HAL decrement is a bare
// `sub dword ptr gs:[..], 1` on a u32, so before this counter existed an
// unpaired enable wrapped preempt_count to 0xFFFFFFFF and can_schedule()'s
// `preempt_count == 0` arms - ordinary timer preemption and yield-driven
// scheduling - were dead for the rest of that CPU's uptime. The release-build
// behaviour is now saturate-at-zero plus this counter; debug builds panic on
// the spot. No logging here: preempt_enable() runs inside spinlock release and
// on interrupt-return paths.
static PREEMPT_UNDERFLOW_COUNT: AtomicU64 = AtomicU64::new(0);

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

    // === Dispatch mark (#772) ===
    // The resume frame the most recent completed dispatch installed on this
    // CPU. `check_need_resched_and_switch` and the three save sites compare
    // the frame they see against this pair to count an identical-frame
    // observation -- a wait-loop revisit census since R113, not a defect
    // census. These four words come out of the padding that already followed
    // `switch_violations`, so the struct keeps its 192-byte size and no offset
    // above moves.
    /// Resume RIP recorded by the last completed dispatch (offset 128)
    pub dispatch_mark_rip: u64,

    /// Resume RSP recorded by the last completed dispatch (offset 136)
    pub dispatch_mark_rsp: u64,

    /// Thread the last completed dispatch installed that frame for (offset 144)
    pub dispatch_mark_tid: u64,

    /// Dispatch-mark state (offset 152): `DISPATCH_MARK_INVALID` or
    /// `DISPATCH_MARK_VALID`
    pub dispatch_mark_state: u64,

    /// Park count the dispatched thread carried at that dispatch (offset 160).
    ///
    /// `Thread::wait_loop_iters` read at the moment the mark was written. A
    /// save whose frame is byte-identical to the mark compares its own read of
    /// that counter against this word: advanced means the thread went round its
    /// wait loop and re-parked on the same halt, unchanged means it retired
    /// no instructions.
    pub dispatch_mark_wait_iters: u64,

    /// Padding to reach 192 bytes (align(64) boundary)
    /// (offset 168-191): 24 bytes of padding
    _pad_final: [u8; 24],
}

// Linux-style preempt_count bit layout constants
// Matches Linux kernel's exact bit partitioning
#[allow(dead_code)]
const PREEMPT_BITS: u32 = 8;
#[allow(dead_code)]
const SOFTIRQ_BITS: u32 = 8;
#[allow(dead_code)]
const HARDIRQ_BITS: u32 = 10; // Linux uses 10 bits for HARDIRQ
#[allow(dead_code)]
const NMI_BITS: u32 = 1; // Linux uses 1 bit for NMI

#[allow(dead_code)]
const PREEMPT_SHIFT: u32 = 0;
#[allow(dead_code)]
const SOFTIRQ_SHIFT: u32 = PREEMPT_SHIFT + PREEMPT_BITS; // 8
#[allow(dead_code)]
const HARDIRQ_SHIFT: u32 = SOFTIRQ_SHIFT + SOFTIRQ_BITS; // 16
#[allow(dead_code)]
const NMI_SHIFT: u32 = HARDIRQ_SHIFT + HARDIRQ_BITS; // 26

/// Mask of the PREEMPT nesting bits - the only field preempt_disable()/
/// preempt_enable() move, and the field the underflow guard consults.
const PREEMPT_MASK: u32 = ((1 << PREEMPT_BITS) - 1) << PREEMPT_SHIFT; // 0x000000FF
#[allow(dead_code)]
const SOFTIRQ_MASK: u32 = ((1 << SOFTIRQ_BITS) - 1) << SOFTIRQ_SHIFT; // 0x0000FF00
#[allow(dead_code)]
const HARDIRQ_MASK: u32 = ((1 << HARDIRQ_BITS) - 1) << HARDIRQ_SHIFT; // 0x03FF0000
#[allow(dead_code)]
const NMI_MASK: u32 = ((1 << NMI_BITS) - 1) << NMI_SHIFT; // 0x04000000

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

const _: () = assert!(
    offset_of!(PerCpuData, cpu_id) == PERCPU_CPU_ID_OFFSET,
    "PERCPU_CPU_ID_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, current_thread) == PERCPU_CURRENT_THREAD_OFFSET,
    "PERCPU_CURRENT_THREAD_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, kernel_stack_top) == PERCPU_KERNEL_STACK_TOP_OFFSET,
    "PERCPU_KERNEL_STACK_TOP_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, idle_thread) == PERCPU_IDLE_THREAD_OFFSET,
    "PERCPU_IDLE_THREAD_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, preempt_count) == PERCPU_PREEMPT_COUNT_OFFSET,
    "PERCPU_PREEMPT_COUNT_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, need_resched) == PERCPU_NEED_RESCHED_OFFSET,
    "PERCPU_NEED_RESCHED_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, user_rsp_scratch) == PERCPU_USER_RSP_SCRATCH_OFFSET,
    "PERCPU_USER_RSP_SCRATCH_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, tss) == PERCPU_TSS_OFFSET,
    "PERCPU_TSS_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, softirq_pending) == PERCPU_SOFTIRQ_PENDING_OFFSET,
    "PERCPU_SOFTIRQ_PENDING_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, next_cr3) == PERCPU_NEXT_CR3_OFFSET,
    "PERCPU_NEXT_CR3_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, kernel_cr3) == PERCPU_KERNEL_CR3_OFFSET,
    "PERCPU_KERNEL_CR3_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, saved_process_cr3) == PERCPU_SAVED_PROCESS_CR3_OFFSET,
    "PERCPU_SAVED_PROCESS_CR3_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, exception_cleanup_context) == PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
    "PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, dispatch_mark_rip) == PERCPU_DISPATCH_MARK_RIP_OFFSET,
    "PERCPU_DISPATCH_MARK_RIP_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, dispatch_mark_rsp) == PERCPU_DISPATCH_MARK_RSP_OFFSET,
    "PERCPU_DISPATCH_MARK_RSP_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, dispatch_mark_tid) == PERCPU_DISPATCH_MARK_TID_OFFSET,
    "PERCPU_DISPATCH_MARK_TID_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, dispatch_mark_state) == PERCPU_DISPATCH_MARK_STATE_OFFSET,
    "PERCPU_DISPATCH_MARK_STATE_OFFSET mismatch with struct layout"
);
const _: () = assert!(
    offset_of!(PerCpuData, dispatch_mark_wait_iters) == PERCPU_DISPATCH_MARK_WAIT_ITERS_OFFSET,
    "PERCPU_DISPATCH_MARK_WAIT_ITERS_OFFSET mismatch with struct layout"
);

// Alignment assertions
const _: () = assert!(
    PERCPU_PREEMPT_COUNT_OFFSET % 4 == 0,
    "preempt_count must be 4-byte aligned"
);
const _: () = assert!(
    PERCPU_USER_RSP_SCRATCH_OFFSET % 8 == 0,
    "user_rsp_scratch must be 8-byte aligned"
);
const _: () = assert!(
    core::mem::size_of::<usize>() == 8,
    "This code assumes 64-bit pointers"
);

// Verify struct size is 192 bytes due to align(64) attribute
// The actual data is 128 bytes (switch_violations ends at offset 128), but align(64) rounds up to 192
const _: () = assert!(
    core::mem::size_of::<PerCpuData>() == 192,
    "PerCpuData must be 192 bytes (aligned to 64)"
);

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
            dispatch_mark_rip: 0,
            dispatch_mark_rsp: 0,
            dispatch_mark_tid: 0,
            dispatch_mark_state: DISPATCH_MARK_INVALID,
            dispatch_mark_wait_iters: 0,
            _pad_final: [0; 24],
        }
    }
}

/// Static per-CPU data for CPU 0 (BSP)
/// In a real SMP kernel, we'd have an array of these
static mut CPU0_DATA: PerCpuData = PerCpuData::new(0);

/// Flag to indicate whether per-CPU data is initialized and safe to use
/// CRITICAL: Interrupts MUST be disabled until this is true
static PER_CPU_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Check if per-CPU data has been initialized
/// Note: Used in non-interactive builds (logger.rs framebuffer check)
#[allow(dead_code)]
pub fn is_initialized() -> bool {
    PER_CPU_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize per-CPU data for the current CPU
pub fn init() {
    use crate::arch_impl::current::paging::X86PageTableOps;
    use crate::arch_impl::PageTableOps;

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
    log::debug!(
        "  KERNEL_GS_BASE = {:#x}",
        hal_percpu::read_kernel_gs_base()
    );

    // HAL Read-back verification: Verify GS-relative operations actually work
    // This catches misconfigured GS base before any interrupt handlers run

    let read_cpu_id = hal_percpu::X86PerCpu::cpu_id();
    if read_cpu_id != 0 {
        panic!(
            "HAL verification failed: cpu_id read-back mismatch (expected 0, got {})",
            read_cpu_id
        );
    }

    // Verify preempt_count read/write cycle
    let initial_preempt = hal_percpu::X86PerCpu::preempt_count();
    hal_percpu::X86PerCpu::preempt_disable();
    let after_disable = hal_percpu::X86PerCpu::preempt_count();
    if after_disable != initial_preempt + 1 {
        panic!(
            "HAL verification failed: preempt_disable did not increment (expected {}, got {})",
            initial_preempt + 1,
            after_disable
        );
    }
    hal_percpu::X86PerCpu::preempt_enable();
    let after_enable = hal_percpu::X86PerCpu::preempt_count();
    if after_enable != initial_preempt {
        panic!(
            "HAL verification failed: preempt_enable did not restore (expected {}, got {})",
            initial_preempt, after_enable
        );
    }
    log::info!("HAL read-back verification passed: GS-relative operations working");

    // Mark per-CPU data as initialized and safe to use
    PER_CPU_INITIALIZED.store(true, Ordering::Release);
    log::info!(
        "Per-CPU data marked as initialized - preempt_count functions now use per-CPU storage"
    );

    // Store the current CR3 as the initial kernel CR3 via HAL
    // NOTE: At this point, we're still using the bootloader's page tables.
    // After memory::init() calls build_master_kernel_pml4(), the kernel switches
    // to the master PML4 and calls set_kernel_cr3() to update this value.
    // This initial value provides a fallback during early boot.
    let kernel_cr3_val = X86PageTableOps::read_root();
    log::info!(
        "Storing initial kernel_cr3 = {:#x} in per-CPU data (bootloader PT)",
        kernel_cr3_val
    );

    unsafe {
        hal_percpu::X86PerCpu::set_kernel_cr3(kernel_cr3_val);
    }
    log::info!(
        "kernel_cr3 stored successfully - interrupt handlers can now switch to kernel page tables"
    );

    // HAL boot stage marker - proves HAL per-CPU operations are working
    log::info!("HAL_PERCPU_INITIALIZED: Per-CPU data setup via HAL complete");
}

/// Get the current thread from per-CPU data
pub fn current_thread() -> Option<&'static mut crate::task::thread::Thread> {
    // Use HAL for GS-relative access

    let thread_ptr =
        hal_percpu::X86PerCpu::current_thread_ptr() as *mut crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some(&mut *thread_ptr) }
    }
}

/// Read the current thread ID without taking the scheduler lock or creating a
/// mutable reference to the GS-relative thread object.
#[inline(always)]
pub fn current_thread_id_lock_free() -> Option<u64> {
    let thread_ptr =
        hal_percpu::X86PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some((*thread_ptr).id) }
    }
}

/// Record that the running thread just parked on the shared halt primitive
/// (#772, R113).
///
/// Called from the kernel's park primitives -- `crate::arch_halt_with_interrupts`,
/// `crate::arch_halt` and the private one in `graphics/render_task.rs` --
/// immediately before the halt, and by hand from the two loops that park on a
/// raw `enable_and_hlt`/`wfi` of their own (`task/executor.rs`'s
/// `sleep_if_idle` and `task/spawn.rs`'s `idle_thread_fn`). Every blocking
/// wait loop in the kernel therefore reaches a bump at its own park point with
/// no per-site call to keep in sync. The
/// five idle and terminal halt loops that park on a raw `enable_and_hlt` --
/// four in `main.rs`, and `idle_loop` in `interrupts/context_switch.rs` -- are
/// NOT counted; `crate::arch_halt_with_interrupts` carries the full census and
/// what the omission costs.
// claim-lint:ok: 25 of 25 arch_halt_with_interrupts call sites and 24 of 24
// arch_halt call sites under kernel/src reach this function, counted by grep in
// this slot.
///
/// Two relaxed atomic adds in the counted case: one whole-machine park total,
/// and one through the per-CPU current-thread pointer -- the same lock-free
/// deref `current_thread_id_lock_free` above already performs on the
/// interrupt-return path. A park this function refuses (per-CPU data not yet
/// initialised, or no thread installed) bumps `WAIT_LOOP_PARK_SKIPPED` instead
/// of a thread, so the park side is auditable rather than assumed: what
/// reached a thread is `WAIT_LOOP_PARK_TOTAL - WAIT_LOOP_PARK_SKIPPED`.
/// No lock, no allocation, no formatting, and no control flow depends on any
/// of the values.
#[inline(always)]
pub fn note_wait_loop_park() {
    // The same guard the 4 dispatch-mark accessors below carry, and for a
    // sharper reason on this arch: `X86PerCpu::current_thread_ptr` is a bare
    // `mov reg, gs:[8]`, without the aarch64 twin's null-base fallback.
    // A park reached before `init` installs the GS base would read linear
    // address 8 and, if that read produced a non-null value, would do a
    // `lock xadd` at an unvalidated address. This function is now reached from
    // all 3 park primitives, including ones that run long before the dispatch
    // path does, so the guard is not hypothetical hygiene.
    // claim-lint:ok: 4 of 4 dispatch-mark accessors below take this guard and
    // 3 of 3 park primitives reach this function, counted by grep in this slot.
    crate::tracing::providers::counters::note_park_total();
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        crate::tracing::providers::counters::note_park_skipped();
        return;
    }
    let thread_ptr =
        hal_percpu::X86PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;

    if thread_ptr.is_null() {
        crate::tracing::providers::counters::note_park_skipped();
        return;
    }
    unsafe {
        (*thread_ptr).wait_loop_iters.fetch_add(1, Ordering::Relaxed);
    }
}

/// Read the running thread of this CPU as an (id, park-count) pair (#772, R113).
///
/// Both words come from one deref, so the caller can check that the count it
/// reads belongs to the thread it means to attribute it to. The pair is absent
/// when this CPU has no thread installed.
#[inline(always)]
pub fn current_wait_loop_iters() -> Option<(u64, u64)> {
    let thread_ptr =
        hal_percpu::X86PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe {
            Some((
                (*thread_ptr).id,
                (*thread_ptr).wait_loop_iters.load(Ordering::Relaxed),
            ))
        }
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

/// Check if we're executing a hardware IRQ, NMI, or softirq.
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

/// Check if we're executing a softirq rather than merely disabling bottom halves.
pub fn in_serving_softirq() -> bool {
    hal_percpu::X86PerCpu::in_serving_softirq()
}

/// Return the complete softirq field, including bottom-half disable nesting.
pub fn softirq_count() -> u32 {
    hal_percpu::X86PerCpu::softirq_count()
}

/// Check if we're in NMI context
pub fn in_nmi() -> bool {
    // Use HAL for NMI context check
    hal_percpu::X86PerCpu::in_nmi()
}

/// Enter hardware IRQ context (called by interrupt handlers)
pub fn irq_enter() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "irq_enter called before per-CPU initialization"
    );

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "irq_exit called before per-CPU initialization"
    );

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "nmi_enter called before per-CPU initialization"
    );

    // Use HAL for atomic GS-relative increment (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::nmi_enter();
    }
}

/// Exit NMI context
pub fn nmi_exit() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "nmi_exit called before per-CPU initialization"
    );

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "softirq_enter called before per-CPU initialization"
    );

    // Use HAL for atomic GS-relative increment (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::softirq_enter();
    }
}

/// Disable bottom-half execution without entering softirq execution context.
pub fn bh_disable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "bh_disable called before per-CPU initialization"
    );
    hal_percpu::X86PerCpu::bh_disable();
}

/// Re-enable bottom-half execution.
pub fn bh_enable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "bh_enable called before per-CPU initialization"
    );

    #[cfg(debug_assertions)]
    {
        let count_before = hal_percpu::X86PerCpu::preempt_count();
        debug_assert!(
            (count_before & SOFTIRQ_MASK) >= 2 * SOFTIRQ_OFFSET,
            "bh_enable called but bottom halves are not disabled (preempt_count={:#x})",
            count_before
        );
    }

    hal_percpu::X86PerCpu::bh_enable();
}

/// Exit softirq context
pub fn softirq_exit() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "softirq_exit called before per-CPU initialization"
    );

    // Debug-only underflow check: verify we're in softirq context before decrementing
    #[cfg(debug_assertions)]
    {
        let count_before = hal_percpu::X86PerCpu::preempt_count();
        debug_assert!(
            (count_before & SOFTIRQ_OFFSET) != 0,
            "softirq_exit called but SOFTIRQ count is already 0 (preempt_count={:#x})",
            count_before
        );
    }

    // Use HAL for atomic GS-relative decrement (includes compiler fences)
    unsafe {
        hal_percpu::X86PerCpu::softirq_exit();
    }

    // Note: We intentionally do NOT call preempt_schedule_irq() or try_schedule() here.
    // The need_resched flag is checked by the assembly interrupt return path,
    // which handles both the schedule() call and the actual context switch atomically.
    // Calling schedule() from Rust code without an immediate context switch
    // would desync scheduler state from reality (see scheduler.rs ARCHITECTURAL CONSTRAINT).
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
    let tss_ptr =
        hal_percpu::X86PerCpu::tss_ptr() as *mut x86_64::structures::tss::TaskStateSegment;

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_disable called before per-CPU initialization"
    );

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_enable called before per-CPU initialization"
    );

    // Compiler barrier before decrementing preempt count
    compiler_fence(Ordering::Acquire);

    // Fail-closed underflow guard (#672): decrementing PREEMPT bits that are
    // already zero means some path released a brake it never took. Saturate at
    // zero and count it rather than wrapping to 0xFFFFFFFF, which would leave
    // this CPU with clean preemptive scheduling structurally disabled. Debug
    // builds make it fatal; release builds leave the count truthful and the
    // violation readable at census time via preempt_underflow_count(). Only the
    // PREEMPT bits are consulted, matching what this function decrements - the
    // softirq/hardirq/NMI fields have their own enter/exit wrappers.
    //
    // The check and the decrement are not one atomic read-modify-write, and they
    // do not need to be: preempt_count is per-CPU, the PREEMPT field is moved
    // only by paired disable/enable calls on this same CPU, and an interrupt
    // taken in the window moves the HARDIRQ/SOFTIRQ fields and restores whatever
    // it did to the PREEMPT field before returning. The decrement it guards was
    // already a non-atomic `sub` on the same per-CPU word.
    if hal_percpu::X86PerCpu::preempt_count() & PREEMPT_MASK == 0 {
        PREEMPT_UNDERFLOW_COUNT.fetch_add(1, Ordering::Relaxed);
        debug_assert!(
            false,
            "preempt_enable() underflow: PREEMPT bits already zero (unpaired release)"
        );
        compiler_fence(Ordering::Release);
        return;
    }

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
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_count called before per-CPU initialization"
    );

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
///
/// Delegates to the softirqd subsystem if initialized, otherwise uses a
/// basic fallback that just clears pending bits.
pub fn do_softirq() {
    // Don't process softirqs from nested execution or while bottom halves are disabled.
    if in_interrupt() || in_softirq() {
        return;
    }

    // Delegate to softirqd if initialized (has proper handler dispatch)
    if crate::task::softirqd::is_initialized() {
        crate::task::softirqd::do_softirq();
        return;
    }

    // Fallback: basic implementation before softirqd is ready
    // Just clear the pending bits without calling handlers
    softirq_enter();

    let pending = softirq_pending();
    if pending != 0 {
        // NOTE: No logging here - this runs in interrupt exit path
        for nr in 0..32 {
            if (pending & (1 << nr)) != 0 {
                clear_softirq(nr);
            }
        }
    }

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

/// Get the process CR3 saved for a future return to userspace.
/// Returns 0 if per-CPU storage is not initialized.
#[inline]
pub fn get_saved_process_cr3() -> u64 {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }

    hal_percpu::X86PerCpu::saved_process_cr3()
}

/// Set the process CR3 saved for a future return to userspace.
///
/// Writing 0 makes the return path skip its CR3 restore, which is what the
/// retirement pipeline wants once a root has been deferred.
#[inline]
pub fn set_saved_process_cr3(cr3: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        hal_percpu::X86PerCpu::set_saved_process_cr3(cr3);
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

/// How the frame observed at an interrupt return or a context save relates to
/// the resume frame the last completed dispatch installed on this CPU (#772).
///
/// Both callers are census-only. R113 retired the identical-frame predicate as
/// #772's oracle -- it recognizes a thread that went around its wait loop and
/// re-parked on the same instruction just as readily as one that retired
/// nothing -- and R115 removed the refusal that used to act on it, so no
/// control flow depends on this value.
// claim-lint:ok: docs/planning/green-program/sockets/772-DIAG-2026-09-03.md
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DispatchProgress {
    /// The mark does not describe this frame: either no dispatch is recorded,
    /// the recorded dispatch was of a different thread, or the thread has
    /// moved off the RIP/RSP it was dispatched to.
    Advanced,
    /// The frame is byte-identical -- RIP AND RSP -- to the one the last
    /// dispatch installed for this same thread.
    NoProgress,
}

/// Record the resume frame a completed dispatch installed.
///
/// Called from the interrupt-return path once the frame is final, so this is
/// five GS-relative stores: no lock, no allocation, no formatting.
#[inline(always)]
pub fn set_dispatch_mark(tid: u64, rip: u64, rsp: u64, wait_iters: u64) {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::X86PerCpu::set_dispatch_mark(tid, rip, rsp, wait_iters);
    }
}

/// Invalidate the dispatch mark.
///
/// A path that leaves the CPU running something other than the thread the
/// mark names must reach this or `set_dispatch_mark`, or a stale mark would
/// mis-attribute an identical-frame observation to a different thread. Today
/// there is 1 such path -- the dispatch site in
/// `check_need_resched_and_switch` -- and it reaches one of the two on a
/// completed switch.
#[inline(always)]
pub fn clear_dispatch_mark() {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        hal_percpu::X86PerCpu::set_dispatch_mark_state(DISPATCH_MARK_INVALID);
    }
}

/// Classify the frame an interrupt was entered with against the dispatch mark.
///
/// `tid` must be the thread the CPU is actually running -- pass the same
/// source the mark was written from (`current_thread_id_lock_free()`), so the
/// two ends of the comparison agree on identity.
#[inline(always)]
pub fn classify_dispatch_progress(tid: u64, rip: u64, rsp: u64) -> DispatchProgress {
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return DispatchProgress::Advanced;
    }
    let state = hal_percpu::X86PerCpu::dispatch_mark_state();
    if state == DISPATCH_MARK_INVALID {
        return DispatchProgress::Advanced;
    }
    if hal_percpu::X86PerCpu::dispatch_mark_tid() != tid
        || hal_percpu::X86PerCpu::dispatch_mark_rip() != rip
        || hal_percpu::X86PerCpu::dispatch_mark_rsp() != rsp
    {
        return DispatchProgress::Advanced;
    }
    DispatchProgress::NoProgress
}

/// Split an identical-frame observation into a wait-loop revisit and a
/// dispatch that retired no instructions (#772, R113).
///
/// Only meaningful once `classify_dispatch_progress` has already returned
/// `NoProgress` for the same thread: this reads the park count the thread
/// carries now against the one the dispatch mark stamped, so
/// `Revisit` means the thread reached a park point again, `ZeroIter` means it
/// is still sitting on the park it was dispatched to.
///
/// `tid` must be the thread being saved. The identity is re-checked against
/// the per-CPU current-thread pointer the count is read through, and a
/// mismatch -- or a count below the stamp, which a re-published thread row
/// could produce -- is reported as `Unknown` rather than guessed at.
///
/// One lock-free deref plus two loads. No lock, no allocation, no formatting.
#[inline(always)]
pub fn classify_no_progress_kind(
    tid: u64,
) -> crate::tracing::providers::sched::DispatchNoProgressKind {
    use crate::tracing::providers::sched::DispatchNoProgressKind;
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        return DispatchNoProgressKind::Unknown;
    }
    match current_wait_loop_iters() {
        Some((current_tid, iters)) if current_tid == tid => {
            let stamped = hal_percpu::X86PerCpu::dispatch_mark_wait_iters();
            if iters > stamped {
                DispatchNoProgressKind::Revisit
            } else if iters == stamped {
                DispatchNoProgressKind::ZeroIter
            } else {
                DispatchNoProgressKind::Unknown
            }
        }
        _ => DispatchNoProgressKind::Unknown,
    }
}

const _: () = assert!(
    DISPATCH_MARK_INVALID != DISPATCH_MARK_VALID,
    "dispatch mark states must be distinct"
);

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
        static EARLY_RETURN_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
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

    // Check if current thread is blocked or terminated. This must recognize
    // every ThreadState the scheduler treats as "not really running" --
    // schedule() and unblock() (task/scheduler.rs) both switch on the same
    // five variants (Blocked, BlockedOnSignal, BlockedOnChildExit,
    // BlockedOnTimer, BlockedOnIO); this check used to be missing
    // BlockedOnTimer and BlockedOnIO, which meant a thread parked in
    // sys_nanosleep's or a device wait's HLT loop was never recognized as
    // blocked here. On a single CPU, with nothing else forcing a reschedule
    // (not returning to userspace, not idle, need_resched already consumed),
    // that starved every OTHER ready thread indefinitely -- reproduced by
    // #673's init, whose infinite reap loop nanosleep()s between waitpid
    // attempts and is the first x86 production thread to ever call nanosleep
    // in a retry loop while a sibling thread needs the CPU.
    //
    // This check deliberately recognizes BlockedOnTimer (the variant #673's
    // starvation actually needs -- nanosleep()) but NOT BlockedOnIO (#673
    // review, M5, narrow fix chosen over widening both). The property that
    // makes the other four variants (Blocked, BlockedOnSignal,
    // BlockedOnChildExit, Terminated) unconditionally safe to recognize here
    // is that they all feed the SAME `current_thread_blocked_or_terminated`
    // term below, which is OR'd ahead of every `current_preempt == 0` guard
    // in `result` -- so all five variants, were BlockedOnIO added, would
    // bypass that guard identically; none is special-cased relative to the
    // others by this expression's structure (#673 review, MA1: an earlier
    // version of this comment claimed BlockedOnIO alone bypassed the guard,
    // which the expression itself disproves).
    //
    // The real reason BlockedOnIO stays out is narrower: it is the ONLY one
    // of the five states the boot thread can hold while #673's own
    // production-init brake is up (`launch_x86_production_init()`'s caller
    // in main.rs holds `preempt_disable()` across the disk-backed ext2 read
    // that loads `/sbin/init`) -- the boot thread does not nanosleep, wait
    // on a child, or receive a signal during that read. Today the one path
    // that sets BlockedOnIO for that read
    // (`Completion::wait_timeout_uninterruptible()`, drivers/virtio/block.rs
    // -> task/completion.rs) already calls `preempt_enable()` of its own
    // accord before setting the state and restores the brake afterward, so
    // recognizing it here would not currently defeat that specific brake --
    // but other BlockedOnIO producers on x86 (the WaitQueueHead-based
    // `prepare_to_wait(BlockedOnIO)` call sites in
    // drivers/virtio/{block,block_mmio,sound,sound_mmio,gpu_pci}.rs and
    // syscall/graphics.rs) make no such promise, and nothing here proves
    // every future BlockedOnIO producer will. Recognizing it unconditionally
    // would trade a proof for an observation. Leaving it out does not reopen
    // anything: it was never recognized here before #673 either. This is the
    // documented, still-open gap between preempt_count and scheduling
    // admission during a boot-thread disk-completion wait (#666, #508); if a
    // future device-wait starvation needs BlockedOnIO recognized here too,
    // that is a #666/#508 fix in its own right (establishing the invariant
    // across every producer first), not a side effect of this one.
    // When a thread blocks, it enters an HLT loop waiting for an interrupt.
    // When a thread terminates, it sets need_resched and expects immediate switch.
    // The timer interrupt should be able to switch to another thread.
    let current_thread_blocked_or_terminated =
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(current) = sched.current_thread_mut() {
                current.state == crate::task::thread::ThreadState::BlockedOnSignal
                    || current.state == crate::task::thread::ThreadState::BlockedOnChildExit
                    || current.state == crate::task::thread::ThreadState::BlockedOnTimer
                    || current.state == crate::task::thread::ThreadState::Blocked
                    || current.state == crate::task::thread::ThreadState::Terminated
            } else {
                false
            }
        })
        .unwrap_or(false);

    // Check if need_resched is set - kernel threads use yield_current() which sets this flag
    let need_resched_set = crate::task::scheduler::is_need_resched();

    // CRITICAL: When in exception cleanup context, allow scheduling regardless of PREEMPT_ACTIVE.
    // The exception handler has explicitly requested a reschedule after terminating a process.
    // Without this, PREEMPT_ACTIVE (bit 28) blocks scheduling even though we need to recover.
    //
    // Also allow scheduling when the current thread is blocked or terminated - blocking syscalls
    // use HLT to wait for interrupts, and terminated threads need immediate switch.
    //
    // NEW: Allow scheduling for kernel threads (including kthreads) when they call yield_current().
    // yield_current() sets need_resched, and we need to honor that even for kernel threads.
    let result = in_exception_cleanup
        || current_thread_blocked_or_terminated
        || (current_preempt == 0 && (returning_to_userspace || returning_to_idle_kernel))
        || (current_preempt == 0 && need_resched_set);

    // DEBUG: Print why can_schedule returns false (every 1000 calls)
    static CAN_SCHED_DEBUG_COUNT: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let dbg_count = CAN_SCHED_DEBUG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if !result && dbg_count % 1000 == 0 {
        // Raw serial output to debug
        use x86_64::instructions::port::Port;
        unsafe {
            let mut port: Port<u8> = Port::new(0x3F8);
            // Print: 'p' + preempt_count_hex + 'r' + (need_resched ? '1' : '0')
            port.write(b'p');
            let p = current_preempt as u8;
            port.write(if (p >> 4) < 10 {
                b'0' + (p >> 4)
            } else {
                b'A' + (p >> 4) - 10
            });
            port.write(if (p & 0xF) < 10 {
                b'0' + (p & 0xF)
            } else {
                b'A' + (p & 0xF) - 10
            });
            port.write(b'r');
            port.write(if need_resched_set { b'1' } else { b'0' });
            port.write(b' ');
        }
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

/// Number of preempt_enable() calls refused because the PREEMPT bits were
/// already zero (#672). Any nonzero value is a bracket protocol violation:
/// some path released a scheduling brake it never took. Emitted once per boot
/// as `[PREEMPT_BRACKET_CENSUS:underflow=N]` from kernel_main_continue() and
/// pinned at zero by docker/qemu/run-x86-prod-profile-boot-test.sh.
pub fn preempt_underflow_count() -> u64 {
    PREEMPT_UNDERFLOW_COUNT.load(Ordering::Relaxed)
}
