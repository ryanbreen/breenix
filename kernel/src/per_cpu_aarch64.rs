//! Per-CPU data support for ARM64 using TPIDR_EL1.
//!
//! This module provides per-CPU data structures that can be accessed
//! efficiently via the TPIDR_EL1 register without locks.
//!
//! Unlike the x86_64 version, this is a simpler implementation that
//! delegates most operations directly to the HAL layer since the task
//! and scheduling subsystems are not yet ported to ARM64.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    /// Scratch slot used by assembly nested-IRQ EL1 return paths (offset 48).
    /// The field name remains `tss` for layout compatibility with shared
    /// per-CPU constants, but ARM64 repurposes it as a nested resume-SP slot.
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
    /// Padding to align eret_scratch to 8 bytes
    _pad3a: [u8; 7],
    /// Scratch register save area for ERET paths (offset 96)
    /// Used by assembly to save one register across SP switches during ERET.
    pub eret_scratch: u64,
    /// Last frame ELR selected by the Rust dispatcher (offset 104).
    pub dispatch_elr: u64,
    /// Last frame SPSR selected by the Rust dispatcher (offset 112).
    pub dispatch_spsr: u64,
    /// ELR captured by an assembly ERET invariant redirect (offset 120).
    pub eret_guard_elr: u64,
    /// SPSR captured by an assembly ERET invariant redirect (offset 128).
    pub eret_guard_spsr: u64,
    /// Guard source tag, written last to publish the record (offset 136).
    pub eret_guard_source: u64,
    /// Second scratch slot used by the assembly dispatch ERET path to carry
    /// `frame.x17` across the SP switch (offset 144).
    pub eret_scratch2: u64,
    /// X29 captured by an assembly ERET invariant redirect (offset 152).
    pub eret_guard_x29: u64,
    /// X30 captured by an assembly ERET invariant redirect (offset 160).
    pub eret_guard_x30: u64,
    /// SP captured by an assembly ERET invariant redirect (offset 168).
    pub eret_guard_sp: u64,
    /// Number of assembly ERET invariant redirects (offset 176).
    pub eret_guard_count: u64,
    /// This CPU's fixed idle/exception stack top (offset 184).
    pub idle_stack_top: u64,
}

const _: () = assert!(
    core::mem::size_of::<PerCpuData>() == 192,
    "PerCpuData must be 192 bytes"
);

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
            _pad3a: [0; 7],
            eret_scratch: 0,
            dispatch_elr: 0,
            dispatch_spsr: 0,
            eret_guard_elr: 0,
            eret_guard_spsr: 0,
            eret_guard_source: 0,
            eret_scratch2: 0,
            eret_guard_x29: 0,
            eret_guard_x30: 0,
            eret_guard_sp: 0,
            eret_guard_count: 0,
            idle_stack_top: 0,
        }
    }
}

const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_elr)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_ELR_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_spsr)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_SPSR_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_source)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_SOURCE_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_scratch2)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_SCRATCH2_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_x29)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_X29_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_x30)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_X30_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_sp)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_SP_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, eret_guard_count)
        == crate::arch_impl::aarch64::constants::PERCPU_ERET_GUARD_COUNT_OFFSET
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, idle_stack_top)
        == crate::arch_impl::aarch64::constants::PERCPU_IDLE_STACK_TOP_OFFSET
);

/// Per-CPU data for all CPUs (up to MAX_CPUS).
/// Each CPU's TPIDR_EL1 points to its own entry in this array.
static mut ALL_CPU_DATA: [PerCpuData; crate::arch_impl::aarch64::constants::MAX_CPUS] = [
    PerCpuData::new(0),
    PerCpuData::new(1),
    PerCpuData::new(2),
    PerCpuData::new(3),
    PerCpuData::new(4),
    PerCpuData::new(5),
    PerCpuData::new(6),
    PerCpuData::new(7),
];

/// Flag to indicate whether per-CPU data is initialized
static PER_CPU_INITIALIZED: AtomicBool = AtomicBool::new(false);
static EARLY_SOFTIRQ_PENDING: AtomicU32 = AtomicU32::new(0);

/// Read the last assembly ERET-guard redirect record for `cpu_id`.
/// The source tag is written last by assembly and acts as the validity word.
pub fn eret_guard_record(cpu_id: usize) -> Option<(u64, u64, u64)> {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return None;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    let source =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_source)) };
    if source == 0 {
        return None;
    }
    core::sync::atomic::fence(Ordering::Acquire);
    let elr = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_elr)) };
    let spsr =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_spsr)) };
    Some((source, elr, spsr))
}

/// Read the complete assembly ERET-guard redirect record for `cpu_id`.
/// The source tag is written last by assembly and acts as the validity word.
pub fn eret_guard_record_full(cpu_id: usize) -> Option<(u64, u64, u64, u64, u64, u64, u64)> {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return None;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    let source =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_source)) };
    if source == 0 {
        return None;
    }
    core::sync::atomic::fence(Ordering::Acquire);
    let elr = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_elr)) };
    let spsr =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_spsr)) };
    let x29 = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_x29)) };
    let x30 = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_x30)) };
    let sp = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_sp)) };
    let count =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_count)) };
    Some((source, elr, spsr, x29, x30, sp, count))
}

/// Total refusal-arm executions recorded across all CPUs.
///
/// Each arm increments its CPU's counter before publishing, so this does not
/// coalesce the way the single-entry record slot does. It is the denominator
/// that explains a drained-refusal count lower than an injection count.
pub fn eret_guard_events_total() -> u64 {
    let mut total = 0u64;
    for cpu_id in 0..crate::arch_impl::aarch64::constants::MAX_CPUS {
        let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
        total = total.saturating_add(unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_count))
        });
    }
    total
}

/// Exclusively claim the ERET-guard validity word after reading its record.
pub fn eret_guard_claim_source(cpu_id: usize) -> u64 {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return 0;
    }

    let cpu_data = unsafe { &raw mut ALL_CPU_DATA[cpu_id] };
    let source = unsafe {
        core::sync::atomic::AtomicU64::from_ptr(core::ptr::addr_of_mut!(
            (*cpu_data).eret_guard_source
        ))
    };
    source.swap(0, Ordering::AcqRel)
}

/// Publish a synthetic ERET-guard refusal record into `cpu_id`'s per-CPU slot.
///
/// Test-only. The foreign-record oracle plants a record in an offline CPU slot
/// so the drain's foreign path can be exercised without disturbing any running
/// CPU's state, and without waiting for a real cross-CPU refusal to race.
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
pub fn plant_synthetic_eret_guard_record(cpu_id: usize, elr: u64, sp: u64, source: u64) -> bool {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS || source == 0 {
        return false;
    }
    let cpu_data = unsafe { &raw mut ALL_CPU_DATA[cpu_id] };
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_elr), elr);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_spsr), 0);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_x29), 0);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_x30), 0);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_sp), sp);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).eret_guard_count), 1);
    }
    core::sync::atomic::fence(Ordering::Release);
    let source_word = unsafe {
        core::sync::atomic::AtomicU64::from_ptr(core::ptr::addr_of_mut!(
            (*cpu_data).eret_guard_source
        ))
    };
    source_word.store(source, Ordering::Release);
    true
}

/// True when `cpu_id` still has an unclaimed ERET-guard refusal record.
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
pub fn eret_guard_record_is_published(cpu_id: usize) -> bool {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return false;
    }
    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).eret_guard_source)) != 0 }
}

/// Read the fixed idle/exception stack top recorded for `cpu_id`.
pub fn idle_stack_top(cpu_id: usize) -> u64 {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return 0;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).idle_stack_top)) }
}

/// Record the fixed idle/exception stack top for `cpu_id`.
pub fn set_idle_stack_top(cpu_id: usize, top: u64) {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return;
    }

    let cpu_data = unsafe { &raw mut ALL_CPU_DATA[cpu_id] };
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*cpu_data).idle_stack_top), top);
    }
}

/// Snapshot the two per-CPU pointers that can keep a kernel-stack slot live.
///
/// These fields are also read and written by exception-return assembly, so use
/// volatile loads rather than borrowing the shared per-CPU object. Callers use
/// this only as a conservative reclamation/allocator exclusion check.
pub fn live_stack_snapshot(cpu_id: usize) -> Option<(u64, u64)> {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return None;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    let kernel_stack_top =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).kernel_stack_top)) };
    let user_rsp_scratch =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).user_sp_scratch)) };
    Some((kernel_stack_top, user_rsp_scratch))
}

/// Read another CPU's preempt count out of its published per-CPU block.
///
/// `preempt_count()` below reads this CPU's block through `TPIDR_EL1` and can
/// therefore only answer for the CPU asking. A preemption bracket is held by
/// whichever CPU took it, so an observer scoring someone else's bracket has to
/// read the owner's word — the same volatile read of `ALL_CPU_DATA[cpu]` that
/// `live_stack_snapshot` above already makes for the stack-custody words, and
/// for the same reason: the field is written by another CPU while this one
/// looks at it.
///
/// `None` for an index outside the array; an offline CPU reads its initialised
/// zero, which is the conservative answer for every caller here (it can only
/// make a bracket look absent, never present).
#[cfg(feature = "coreproof")]
pub fn preempt_count_snapshot(cpu_id: usize) -> Option<u32> {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return None;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    Some(unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).preempt_count)) })
}

/// Snapshot the per-CPU TTBR0 shadows that can retain a userspace root.
///
/// Exception-return assembly also reads and writes these fields, so use
/// volatile loads rather than borrowing the shared per-CPU object. Callers
/// combine this conservative snapshot with a scheduling-epoch grace period
/// before returning frames reachable from a retired root to the allocator.
pub fn ttbr0_shadow_snapshot(cpu_id: usize) -> Option<(u64, u64)> {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS {
        return None;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    let saved_process_ttbr0 =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).saved_process_ttbr0)) };
    let next_ttbr0 =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).next_ttbr0)) };
    Some((saved_process_ttbr0, next_ttbr0))
}

/// Check if per-CPU data has been initialized
pub fn is_initialized() -> bool {
    PER_CPU_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize per-CPU data for CPU 0 (boot CPU).
pub fn init() {
    log::info!("Initializing per-CPU data via TPIDR_EL1");

    init_cpu(0);

    log::info!(
        "Per-CPU data initialized at {:#x}",
        hal_percpu::percpu_base()
    );
    log::debug!("  TPIDR_EL1 = {:#x}", hal_percpu::percpu_base());

    // Verification
    let read_cpu_id = hal_percpu::Aarch64PerCpu::cpu_id();
    if read_cpu_id != 0 {
        panic!(
            "HAL verification failed: cpu_id read-back mismatch (expected 0, got {})",
            read_cpu_id
        );
    }
    log::info!("HAL read-back verification passed: TPIDR_EL1-relative operations working");

    // Mark per-CPU data as initialized
    PER_CPU_INITIALIZED.store(true, Ordering::Release);
    let early_pending = EARLY_SOFTIRQ_PENDING.swap(0, Ordering::AcqRel);
    if early_pending != 0 {
        for nr in 0..32 {
            if (early_pending & (1 << nr)) != 0 {
                unsafe {
                    hal_percpu::Aarch64PerCpu::raise_softirq(nr);
                }
            }
        }
    }
    log::info!("Per-CPU data marked as initialized");
}

/// Initialize per-CPU data for a specific CPU.
///
/// Sets TPIDR_EL1 to point to the CPU's entry in ALL_CPU_DATA.
/// Must be called on the target CPU itself (each CPU sets its own TPIDR_EL1).
///
/// For CPU 0 this is called from `init()`. For secondary CPUs it is called
/// from `secondary_cpu_entry_rust()`.
pub fn init_cpu(cpu_id: usize) {
    let max_cpus = crate::arch_impl::aarch64::constants::MAX_CPUS;
    if cpu_id >= max_cpus {
        return;
    }

    let cpu_data_ptr = unsafe { &raw mut ALL_CPU_DATA[cpu_id] as *mut PerCpuData };
    let cpu_data_addr = cpu_data_ptr as u64;

    unsafe {
        hal_percpu::init_percpu(cpu_data_addr, cpu_id as u64);
    }
    set_idle_stack_top(
        cpu_id,
        crate::arch_impl::aarch64::constants::percpu_kernel_stack_top(cpu_id),
    );
    // Stamp this CPU's name into its own idle/exception stack half. Runs on the
    // CPU itself and before its first guarded stack-top install: secondaries
    // reach here from `secondary_cpu_entry_rust` ahead of their
    // `set_kernel_stack_top`, and CPU 0 reaches it from `init()` ahead of
    // `init_scheduler()`.
    crate::arch_impl::aarch64::constants::publish_percpu_stack_owner(cpu_id);
}

/// Get the current thread pointer (raw)
pub fn current_thread_ptr() -> *mut u8 {
    hal_percpu::Aarch64PerCpu::current_thread_ptr()
}

/// Get the current thread from per-CPU data
pub fn current_thread() -> Option<&'static mut crate::task::thread::Thread> {
    let thread_ptr =
        hal_percpu::Aarch64PerCpu::current_thread_ptr() as *mut crate::task::thread::Thread;

    if thread_ptr.is_null() {
        None
    } else {
        unsafe { Some(&mut *thread_ptr) }
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
/// and one through the per-CPU current-thread pointer. A park this function
/// refuses (per-CPU data not yet initialised, or no thread installed) bumps
/// `WAIT_LOOP_PARK_SKIPPED` instead of a thread, so the park side is auditable
/// rather than assumed: what reached a thread is
/// `WAIT_LOOP_PARK_TOTAL - WAIT_LOOP_PARK_SKIPPED`. No lock, no allocation, no
/// formatting, and no control flow depends on any of the values.
///
/// The dispatch mark that consumes this count is x86-only today, so on aarch64
/// this keeps the count and no reader consumes it yet. There is deliberately
/// no aarch64 `current_wait_loop_iters` accessor: the x86 one exists because
/// the dispatch mark stamps the count, and an uncalled twin here would be dead
/// code that the `pub`-in-`pub mod` shape hides from the lint rather than
/// justifies. It comes back when aarch64 grows a dispatch mark.
#[inline(always)]
pub fn note_wait_loop_park() {
    // The x86 twin needs this guard because its current-thread read is a bare
    // `mov reg, gs:[8]`; here `percpu_read_u64` already returns 0 when
    // TPIDR_EL1 reads 0, so the null check below would catch a pre-init park
    // on its own. The guard is carried anyway so the two arches have one shape, and
    // so a pre-init park is refused for the same stated reason on both.
    crate::tracing::providers::counters::note_park_total();
    if !PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        crate::tracing::providers::counters::note_park_skipped();
        return;
    }
    let thread_ptr =
        hal_percpu::Aarch64PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;

    if thread_ptr.is_null() {
        crate::tracing::providers::counters::note_park_skipped();
        return;
    }
    unsafe {
        (*thread_ptr).wait_loop_iters.fetch_add(1, Ordering::Relaxed);
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
///
/// `#[track_caller]` so a refused install names the caller of this wrapper
/// rather than the wrapper itself.
#[track_caller]
pub fn set_kernel_stack_top(stack_top: u64) {
    unsafe {
        hal_percpu::Aarch64PerCpu::set_kernel_stack_top(stack_top);
    }
}

/// Set user_rsp_scratch (SP restored by boot.S ERET path)
///
/// `#[track_caller]` so a refused install names the caller of this wrapper
/// rather than the wrapper itself.
#[track_caller]
pub fn set_user_rsp_scratch(sp: u64) {
    unsafe {
        hal_percpu::Aarch64PerCpu::set_user_rsp_scratch(sp);
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

/// Read another CPU's reschedule flag for scheduler diagnostics.
pub(crate) fn need_resched_for_cpu(cpu_id: usize) -> bool {
    if cpu_id >= crate::arch_impl::aarch64::constants::MAX_CPUS
        || !PER_CPU_INITIALIZED.load(Ordering::Acquire)
    {
        return false;
    }

    let cpu_data = unsafe { &raw const ALL_CPU_DATA[cpu_id] };
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*cpu_data).need_resched)) != 0 }
}

/// Set the reschedule needed flag
pub fn set_need_resched(need: bool) {
    if PER_CPU_INITIALIZED.load(Ordering::Acquire) {
        unsafe {
            hal_percpu::Aarch64PerCpu::set_need_resched(need);
        }
    }
}

/// Check if we're executing a hardware IRQ, NMI/FIQ, or softirq.
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

/// Check if we're executing a softirq rather than merely disabling bottom halves.
pub fn in_serving_softirq() -> bool {
    hal_percpu::Aarch64PerCpu::in_serving_softirq()
}

/// Return the complete softirq field, including bottom-half disable nesting.
pub fn softirq_count() -> u32 {
    hal_percpu::Aarch64PerCpu::softirq_count()
}

/// Check if we're in NMI/FIQ context
pub fn in_nmi() -> bool {
    hal_percpu::Aarch64PerCpu::in_nmi()
}

/// Enter hardware IRQ context
pub fn irq_enter() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "irq_enter called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::irq_enter();
    }
}

/// Exit hardware IRQ context
pub fn irq_exit() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "irq_exit called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::irq_exit();
    }
}

/// Enter NMI context
pub fn nmi_enter() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "nmi_enter called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::nmi_enter();
    }
}

/// Exit NMI context
pub fn nmi_exit() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "nmi_exit called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::nmi_exit();
    }
}

/// Enter softirq context
pub fn softirq_enter() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "softirq_enter called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::softirq_enter();
    }
}

/// Disable bottom-half execution without entering softirq execution context.
pub fn bh_disable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "bh_disable called before per-CPU initialization"
    );
    hal_percpu::Aarch64PerCpu::bh_disable();
}

/// Re-enable bottom-half execution.
pub fn bh_enable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "bh_enable called before per-CPU initialization"
    );
    hal_percpu::Aarch64PerCpu::bh_enable();
}

/// Exit softirq context
pub fn softirq_exit() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "softirq_exit called before per-CPU initialization"
    );
    unsafe {
        hal_percpu::Aarch64PerCpu::softirq_exit();
    }
}

/// Increment preempt count (disable kernel preemption)
pub fn preempt_disable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_disable called before per-CPU initialization"
    );
    hal_percpu::Aarch64PerCpu::preempt_disable();
}

/// Decrement preempt count (enable kernel preemption)
pub fn preempt_enable() {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_enable called before per-CPU initialization"
    );
    hal_percpu::Aarch64PerCpu::preempt_enable();
}

/// Get current preempt count
pub fn preempt_count() -> u32 {
    debug_assert!(
        PER_CPU_INITIALIZED.load(Ordering::Acquire),
        "preempt_count called before per-CPU initialization"
    );
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
        EARLY_SOFTIRQ_PENDING.fetch_or(1 << nr, Ordering::Release);
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
    if in_interrupt() || in_softirq() {
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
    let cpu_data_ptr = unsafe { &raw mut ALL_CPU_DATA[0] as *mut PerCpuData };
    let base = cpu_data_ptr as u64;
    let size = core::mem::size_of::<PerCpuData>();
    (base, size)
}
