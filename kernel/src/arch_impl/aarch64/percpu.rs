//! ARM64 per-CPU data access using TPIDR_EL1.
//!
//! On ARM64, TPIDR_EL1 holds the base pointer to the per-CPU data structure.
//! This is similar to x86's GS segment base. Each CPU core sets TPIDR_EL1 to
//! point to its own PerCpuData structure during initialization.
//!
//! Unlike x86 where we can access fields directly via GS:offset, on ARM64 we
//! read TPIDR_EL1 to get the base address and then add offsets manually.

#![allow(dead_code)]

use crate::arch_impl::aarch64::constants::{
    HARDIRQ_MASK, HARDIRQ_SHIFT, NMI_MASK, NMI_SHIFT, PERCPU_CPU_ID_OFFSET,
    PERCPU_CURRENT_THREAD_OFFSET, PERCPU_DISPATCH_ELR_OFFSET, PERCPU_DISPATCH_SPSR_OFFSET,
    PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET, PERCPU_IDLE_THREAD_OFFSET, PERCPU_KERNEL_CR3_OFFSET,
    PERCPU_KERNEL_STACK_TOP_OFFSET, PERCPU_NEED_RESCHED_OFFSET, PERCPU_NEXT_CR3_OFFSET,
    PERCPU_PREEMPT_COUNT_OFFSET, PERCPU_SAVED_PROCESS_CR3_OFFSET, PERCPU_SOFTIRQ_PENDING_OFFSET,
    PERCPU_TSS_OFFSET, PERCPU_USER_RSP_SCRATCH_OFFSET, PREEMPT_ACTIVE, SOFTIRQ_DISABLE_OFFSET,
    SOFTIRQ_MASK, SOFTIRQ_OFFSET,
};
use crate::arch_impl::aarch64::constants::{
    percpu_kernel_stack_top, percpu_sched_stack_top, percpu_stack_published_owner,
    percpu_stack_slot_of, percpu_stack_top_owned_by,
};
use crate::arch_impl::traits::PerCpuOps;
use core::panic::Location;
use core::sync::atomic::{compiler_fence, AtomicU32, AtomicU64, Ordering};

pub struct Aarch64PerCpu;

// =============================================================================
// Per-CPU stack-top ownership check
// =============================================================================

/// Monotonic count of stack-top installs refused because the address named a
/// slot this CPU does not own. Never reset.
pub static PERCPU_STACK_ALIEN_REFUSALS: AtomicU64 = AtomicU64::new(0);

/// Emission budget for the whole boot. The refusal record is written with
/// `raw_uart_*` from the dispatch path, so it is bounded exactly like the
/// resume-PC refusal record.
static PERCPU_STACK_ALIEN_EMISSIONS: AtomicU64 = AtomicU64::new(0);

/// Maximum alien-install records emitted per boot.
const PERCPU_STACK_ALIEN_EMISSION_CAP: u64 = 16;

/// Total stack-top installs refused for naming another CPU's slot.
pub fn percpu_stack_alien_refusals() -> u64 {
    PERCPU_STACK_ALIEN_REFUSALS.load(Ordering::Acquire)
}

/// Decide whether a per-CPU stack-top install may proceed, and record it when
/// it may not.
///
/// The decision itself is `percpu_stack_top_owned_by`, the one custody
/// predicate this arch shares with the producer side (`idle_dispatch_stack`),
/// so a value the producer normalised can never be refused here and a value
/// this refuses can never have been produced by the normaliser.
///
/// A refusal writes nothing: the previous value stays in place. A leaked stack
/// is always better than a shared one, and substituting inside the setter would
/// hide the defect that produced the address. Callers that are about to RUN on
/// the address must not simply ignore the refusal — `install_idle_return_sp` is
/// the fail-closed install for exactly that case.
///
/// `site` is threaded in from the setter's own `#[track_caller]` location so
/// the record names the code that asked for the install, not the setter.
fn percpu_stack_install_permitted(addr: u64, site: &'static Location<'static>) -> bool {
    let cpu = <Aarch64PerCpu as PerCpuOps>::cpu_id() as usize;
    if percpu_stack_top_owned_by(cpu, addr) {
        return true;
    }
    record_percpu_stack_alien(cpu, addr, site);
    false
}

/// Producer-side custody: the stack top CPU `cpu` may actually dispatch onto,
/// given the address some upstream structure preferred.
///
/// The idle pivot used to take `preferred` verbatim from
/// `cpu_state[cpu].idle_thread`'s `kernel_stack_top` and only substitute when it
/// was zero, so a thread record naming another CPU's slot went straight into
/// the per-CPU words, into the idle thread's saved `context.sp`, and into SP.
/// Refusing that downstream in the setter came too late — the setter writes
/// nothing, and the caller pivoted onto the refused address anyway.
///
/// So the substitution happens HERE, at the choice, using the same predicate
/// the setter guard applies. `percpu_kernel_stack_top(cpu)` is own-slot by
/// construction, so the returned value is always installable and the setter can
/// never refuse it. The refusal is recorded with the caller's own site, which is
/// why this takes `site` explicitly rather than reading `Location::caller()` —
/// the useful location is the dispatch site, not this helper's line.
///
/// It takes a `CpuId`, not a `usize`: the round-4 RCA
/// (`docs/planning/t3g-prb/PRB-R4-RCA-IDENTITY.md`) showed the predicate was
/// already shared with the setter and the two sides still disagreed, because
/// the producer spent an index captured before interrupts were masked while the
/// setter re-read the hardware. A value of this type can only come from a
/// hardware read, so a carried index cannot reach the decision at all.
#[inline]
pub fn percpu_stack_top_for(cpu: CpuId, preferred: u64, site: &'static Location<'static>) -> u64 {
    if percpu_stack_top_owned_by(cpu.index(), preferred) {
        return preferred;
    }
    record_percpu_stack_alien(cpu.index(), preferred, site);
    percpu_kernel_stack_top(cpu.index())
}

/// Which half of a per-CPU stack slot a pivot destination names.
///
/// A refused pivot has to be replaced by an address in the SAME half: the
/// scheduler half and the idle/exception half are two disjoint stacks, and
/// substituting across the boundary would put a scheduler pivot and an idle
/// pivot on one stack — the very sharing this custody exists to prevent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PivotHalf {
    /// The lower, scheduler-owned half (`percpu_sched_stack_top`).
    Scheduler,
    /// The upper, idle/exception-owned half (`percpu_kernel_stack_top`).
    Exception,
}

/// Custody for a stack a CPU is about to PIVOT ONTO and run on.
///
/// The per-CPU words are guarded by `percpu_stack_install_permitted`, but a
/// pivot is not an install: `mov sp, x` runs on the address whether or not any
/// per-CPU word ever named it. Three of the four aarch64 pivots take their
/// destination from a value that has already been adjudicated; the fourth
/// (`scheduler_stack_top`) was adjudicated nowhere at all, which is how CPU 3
/// came to stand on CPU 0's scheduler half in the round-3 specimens.
///
/// Same predicate as every other custody site, evaluated against the same
/// hardware identity, recorded through the same single emitter — this is the
/// existing check applied to the SP a CPU actually runs on, not a new one.
#[inline]
pub fn percpu_pivot_top_for(
    cpu: CpuId,
    preferred: u64,
    half: PivotHalf,
    site: &'static Location<'static>,
) -> u64 {
    if percpu_stack_top_owned_by(cpu.index(), preferred) {
        return preferred;
    }
    record_percpu_stack_alien(cpu.index(), preferred, site);
    match half {
        PivotHalf::Scheduler => percpu_sched_stack_top(cpu.index()),
        PivotHalf::Exception => percpu_kernel_stack_top(cpu.index()),
    }
}

/// Whether CPU `cpu` may RESUME a thread whose saved kernel SP is `sp`.
///
/// A saved SP standing in a per-CPU stack slot names a stack that belongs to
/// exactly one CPU. Resuming such a thread anywhere else puts two CPUs on one
/// stack, which is this campaign's producer-corruption family in one step. The
/// dispatch path refuses it and routes the thread to the CPU that owns the
/// slot; see `dispatch_thread_locked`.
///
/// Heap-backed kernel stacks — every ordinary thread — name no slot and are
/// admitted by the same predicate without a second rule.
#[inline]
pub fn percpu_stack_resume_permitted(
    cpu: CpuId,
    sp: u64,
    site: &'static Location<'static>,
) -> bool {
    if percpu_stack_top_owned_by(cpu.index(), sp) {
        return true;
    }
    record_percpu_stack_alien(cpu.index(), sp, site);
    false
}

// =============================================================================
// The CPU identity a per-CPU decision may be made with
// =============================================================================

/// Count of decisions whose carried CPU index disagreed with the hardware
/// identity read where the decision was actually made. Never reset.
///
/// This is its own census, not part of `PERCPU_STACK_ALIEN_REFUSALS`: two
/// rounds of RCA chased the wrong producer precisely because the disagreement
/// arrived disguised as an ordinary alien address.
pub static CPU_IDENTITY_SPLIT_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Emission budget for the whole boot, exactly like the alien record's.
static CPU_IDENTITY_SPLIT_EMISSIONS: AtomicU64 = AtomicU64::new(0);

/// Maximum identity-split records emitted per boot.
const CPU_IDENTITY_SPLIT_EMISSION_CAP: u64 = 16;

/// Total decisions whose carried index disagreed with the hardware identity.
pub fn cpu_identity_split_events() -> u64 {
    CPU_IDENTITY_SPLIT_EVENTS.load(Ordering::Acquire)
}

/// The one emitter of the `[CPU_IDENTITY_SPLIT:` record.
#[cold]
#[inline(never)]
fn record_cpu_identity_split(carried: usize, fresh: usize, site: &'static Location<'static>) {
    CPU_IDENTITY_SPLIT_EVENTS.fetch_add(1, Ordering::Release);
    if CPU_IDENTITY_SPLIT_EMISSIONS.fetch_add(1, Ordering::Relaxed)
        < CPU_IDENTITY_SPLIT_EMISSION_CAP
    {
        use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};
        raw_uart_str("[CPU_IDENTITY_SPLIT:carried=");
        raw_uart_dec(carried as u64);
        raw_uart_str(":fresh=");
        raw_uart_dec(fresh as u64);
        raw_uart_str(":site=");
        raw_uart_str(site.file());
        raw_uart_str(":");
        raw_uart_dec(u64::from(site.line()));
        raw_uart_str("]\n");
    }
}

/// A CPU index read from this CPU's own hardware per-CPU block at the point it
/// is spent.
///
/// A plain `usize` cannot say WHEN it was read. `schedule_from_kernel` captured
/// one with interrupts still enabled and then spent it, after a preemption that
/// could resume the thread anywhere, on `scheduler_stack_top`,
/// `INLINE_SCHEDULE_STATE[]` and `cpu_state[]` — so CPU 3 pivoted onto CPU 0's
/// scheduler stack and read CPU 0's spilled locals back as its own. This type
/// exists so that class cannot be written: there is no constructor from an
/// index, only from a hardware read, and every per-CPU stack decision on the
/// dispatch path takes this type.
///
/// It is `Copy` and one word wide, so passing it costs exactly what passing the
/// index cost.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CpuId(usize);

impl CpuId {
    /// Read this CPU's identity now.
    ///
    /// Callers use this where the identity is being established for the first
    /// time in a masked region; where a caller already carries an index, it must
    /// use `current_checked` so the disagreement is recorded rather than lost.
    #[inline(always)]
    pub fn current() -> CpuId {
        CpuId(<Aarch64PerCpu as PerCpuOps>::cpu_id() as usize)
    }

    /// Read this CPU's identity now, and record it when a carried index
    /// disagrees.
    ///
    /// The hardware read wins — it is the identity the per-CPU block, `boot.S`
    /// and the setter guard all use — so the decision is made for the CPU that
    /// will actually run on the result. The carried value is not silently
    /// discarded: a disagreement means the invocation belongs to another CPU
    /// and it earns its own gate-failing record.
    #[inline(always)]
    #[track_caller]
    pub fn current_checked(carried: usize) -> CpuId {
        let fresh = <Aarch64PerCpu as PerCpuOps>::cpu_id() as usize;
        if fresh != carried {
            record_cpu_identity_split(carried, fresh, Location::caller());
        }
        CpuId(fresh)
    }

    /// The index, for the array and arithmetic uses that need one.
    #[inline(always)]
    pub fn index(self) -> usize {
        self.0
    }
}

/// The one emitter of the `[PERCPU_STACK_ALIEN:` refusal record.
///
/// Both custody sides funnel through it — the producer that declines to choose
/// a foreign address and the setter that declines to install one — so the
/// evidence channel is a single literal with a single census, and moving the
/// repair upstream cannot quietly move the evidence out of the gate.
///
/// Lock-free `raw_uart_*` with a whole-boot emission budget: this runs from the
/// dispatch path, where the resume-PC refusal record set the precedent.
fn record_percpu_stack_alien(cpu: usize, addr: u64, site: &'static Location<'static>) {
    let slot = percpu_stack_slot_of(addr).unwrap_or(usize::MAX);
    let published = percpu_stack_published_owner(slot);

    PERCPU_STACK_ALIEN_REFUSALS.fetch_add(1, Ordering::Release);
    if PERCPU_STACK_ALIEN_EMISSIONS.fetch_add(1, Ordering::Relaxed)
        < PERCPU_STACK_ALIEN_EMISSION_CAP
    {
        use crate::arch_impl::aarch64::context_switch::{
            last_dispatched_tid, raw_uart_dec, raw_uart_hex, raw_uart_str,
        };
        raw_uart_str("[PERCPU_STACK_ALIEN:cpu=");
        raw_uart_dec(cpu as u64);
        raw_uart_str(":owner=");
        match published {
            Some(owner) => raw_uart_dec(owner as u64),
            None => raw_uart_str("unpublished"),
        }
        raw_uart_str(":sp=");
        raw_uart_hex(addr);
        raw_uart_str(":tid=");
        raw_uart_dec(last_dispatched_tid(cpu).unwrap_or(0));
        raw_uart_str(":site=");
        raw_uart_str(site.file());
        raw_uart_str(":");
        raw_uart_dec(u64::from(site.line()));
        raw_uart_str("]\n");
    }
}

/// Read TPIDR_EL1 (per-CPU data base pointer)
#[inline(always)]
fn read_tpidr_el1() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) val, options(nomem, nostack, preserves_flags));
    }
    val
}

/// Write TPIDR_EL1 (per-CPU data base pointer)
#[inline(always)]
unsafe fn write_tpidr_el1(val: u64) {
    core::arch::asm!("msr tpidr_el1, {}", in(reg) val, options(nomem, nostack, preserves_flags));
}

/// Read a u64 from per-CPU data at the given offset
#[inline(always)]
fn percpu_read_u64(offset: usize) -> u64 {
    let base = read_tpidr_el1();
    if base == 0 {
        // Per-CPU not yet initialized
        return 0;
    }
    unsafe { core::ptr::read_volatile((base as *const u8).add(offset) as *const u64) }
}

/// Write a u64 to per-CPU data at the given offset
#[inline(always)]
unsafe fn percpu_write_u64(offset: usize, val: u64) {
    let base = read_tpidr_el1();
    if base == 0 {
        return; // Per-CPU not yet initialized
    }
    core::ptr::write_volatile((base as *mut u8).add(offset) as *mut u64, val);
}

/// Read a u32 from per-CPU data at the given offset
#[inline(always)]
fn percpu_read_u32(offset: usize) -> u32 {
    let base = read_tpidr_el1();
    if base == 0 {
        return 0;
    }
    unsafe { core::ptr::read_volatile((base as *const u8).add(offset) as *const u32) }
}

/// Get atomic reference to a u32 field in per-CPU data
#[inline(always)]
fn percpu_atomic_u32(offset: usize) -> Option<&'static AtomicU32> {
    let base = read_tpidr_el1();
    if base == 0 {
        return None;
    }
    unsafe { Some(&*((base as *const u8).add(offset) as *const AtomicU32)) }
}

impl PerCpuOps for Aarch64PerCpu {
    /// Get the current CPU ID
    ///
    /// Reads from the per-CPU data structure. If not initialized,
    /// falls back to reading MPIDR_EL1 Aff0 field.
    #[inline]
    fn cpu_id() -> u64 {
        let base = read_tpidr_el1();
        if base == 0 {
            // Per-CPU not yet initialized, read from MPIDR_EL1
            let mpidr: u64;
            unsafe {
                core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
            }
            return mpidr & 0xFF;
        }
        percpu_read_u64(PERCPU_CPU_ID_OFFSET)
    }

    /// Get the current thread pointer
    #[inline]
    fn current_thread_ptr() -> *mut u8 {
        percpu_read_u64(PERCPU_CURRENT_THREAD_OFFSET) as *mut u8
    }

    /// Set the current thread pointer
    #[inline]
    unsafe fn set_current_thread_ptr(ptr: *mut u8) {
        percpu_write_u64(PERCPU_CURRENT_THREAD_OFFSET, ptr as u64);
    }

    /// Get the kernel stack top for this CPU
    #[inline]
    fn kernel_stack_top() -> u64 {
        percpu_read_u64(PERCPU_KERNEL_STACK_TOP_OFFSET)
    }

    /// Set the kernel stack top for this CPU
    ///
    /// Refuses an address belonging to another CPU's per-CPU stack slot; see
    /// `percpu_stack_install_permitted`.
    #[inline]
    #[track_caller]
    unsafe fn set_kernel_stack_top(addr: u64) {
        if !percpu_stack_install_permitted(addr, Location::caller()) {
            return;
        }
        percpu_write_u64(PERCPU_KERNEL_STACK_TOP_OFFSET, addr);
        crate::task::percpu_stack_oracle::note_stack_top_install(addr);
    }

    /// Get the preempt count (atomically)
    #[inline]
    fn preempt_count() -> u32 {
        match percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            Some(atomic) => atomic.load(Ordering::Relaxed),
            None => 0,
        }
    }

    /// Disable preemption by incrementing preempt count
    #[inline]
    fn preempt_disable() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Enable preemption by decrementing preempt count
    #[inline]
    fn preempt_enable() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(1, Ordering::Release);
        }
    }

    #[inline(always)]
    fn bh_disable() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(SOFTIRQ_DISABLE_OFFSET, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    #[inline(always)]
    fn bh_enable() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(SOFTIRQ_DISABLE_OFFSET, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Check if we're executing a hardirq, NMI, or softirq.
    #[inline]
    fn in_interrupt() -> bool {
        let count = Self::preempt_count();
        (count & (HARDIRQ_MASK | NMI_MASK)) != 0 || (count & SOFTIRQ_OFFSET) != 0
    }

    #[inline(always)]
    fn in_serving_softirq() -> bool {
        (Self::preempt_count() & SOFTIRQ_OFFSET) != 0
    }

    #[inline(always)]
    fn softirq_count() -> u32 {
        Self::preempt_count() & SOFTIRQ_MASK
    }

    #[inline(always)]
    fn in_softirq() -> bool {
        Self::softirq_count() != 0
    }

    /// Check if we're in hardirq context
    #[inline]
    fn in_hardirq() -> bool {
        let count = Self::preempt_count();
        (count & HARDIRQ_MASK) != 0
    }

    /// Check if scheduling is allowed
    ///
    /// Returns true if preempt_count is 0 (no preemption disabled,
    /// not in interrupt context).
    #[inline]
    fn can_schedule() -> bool {
        Self::preempt_count() == 0
    }
}

// =============================================================================
// Additional ARM64-specific per-CPU helpers (matching x86_64 API)
// =============================================================================

/// Read a u8 from per-CPU data at the given offset
#[inline(always)]
fn percpu_read_u8(offset: usize) -> u8 {
    let base = read_tpidr_el1();
    if base == 0 {
        return 0;
    }
    unsafe { core::ptr::read_volatile((base as *const u8).add(offset)) }
}

/// Write a u8 to per-CPU data at the given offset
#[inline(always)]
unsafe fn percpu_write_u8(offset: usize, val: u8) {
    let base = read_tpidr_el1();
    if base == 0 {
        return;
    }
    core::ptr::write_volatile((base as *mut u8).add(offset), val);
}

/// Write a u32 to per-CPU data at the given offset
#[inline(always)]
unsafe fn percpu_write_u32(offset: usize, val: u32) {
    let base = read_tpidr_el1();
    if base == 0 {
        return;
    }
    core::ptr::write_volatile((base as *mut u8).add(offset) as *mut u32, val);
}

impl Aarch64PerCpu {
    /// Get preempt count (forwarding to trait impl)
    #[inline(always)]
    pub fn preempt_count() -> u32 {
        <Self as PerCpuOps>::preempt_count()
    }

    /// Get CPU ID (forwarding to trait impl)
    #[inline(always)]
    pub fn cpu_id() -> u64 {
        <Self as PerCpuOps>::cpu_id()
    }

    /// Get the need_resched flag.
    #[inline(always)]
    pub fn need_resched() -> bool {
        percpu_read_u8(PERCPU_NEED_RESCHED_OFFSET) != 0
    }

    /// Set the need_resched flag.
    #[inline(always)]
    pub unsafe fn set_need_resched(need: bool) {
        percpu_write_u8(PERCPU_NEED_RESCHED_OFFSET, if need { 1 } else { 0 });
    }

    /// Get the next TTBR0 value (for context switching).
    /// On ARM64 this is the equivalent of x86's next_cr3.
    #[inline(always)]
    pub fn next_cr3() -> u64 {
        percpu_read_u64(PERCPU_NEXT_CR3_OFFSET)
    }

    /// Set the next TTBR0 value.
    ///
    /// Every Rust-side write of this word goes through here, so this is where
    /// the #786 follow-on ASID census counts what the corridor is handed. The
    /// count is lock-free and allocation-free -- see `note_shadow_publish`.
    /// claim-lint:ok: 17 of 17 Rust-side publishes of either shadow word are
    /// printed by `every_shadow_publish_has_an_accounted_asid -- --nocapture`,
    /// recorded in
    /// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/05-suite-green-with-census.txt;
    /// the 2 stores this cannot see are the assembly ones in `syscall_entry.S`.
    #[inline(always)]
    pub unsafe fn set_next_cr3(val: u64) {
        super::ttbr0::note_shadow_publish(val);
        percpu_write_u64(PERCPU_NEXT_CR3_OFFSET, val);
    }

    /// Get the saved process TTBR0.
    #[inline(always)]
    pub fn saved_process_cr3() -> u64 {
        percpu_read_u64(PERCPU_SAVED_PROCESS_CR3_OFFSET)
    }

    /// Set the saved process TTBR0.
    ///
    /// The other half of the census hook on `set_next_cr3`: this is the word
    /// the `.Lrestore_saved_ttbr` arm of `syscall_entry.S` installs verbatim,
    /// ASID field included.
    #[inline(always)]
    pub unsafe fn set_saved_process_cr3(val: u64) {
        super::ttbr0::note_shadow_publish(val);
        percpu_write_u64(PERCPU_SAVED_PROCESS_CR3_OFFSET, val);
    }

    /// Get the kernel TTBR0 (used by interrupt/syscall entry).
    #[inline(always)]
    pub fn kernel_cr3() -> u64 {
        percpu_read_u64(PERCPU_KERNEL_CR3_OFFSET)
    }

    /// Set the kernel TTBR0.
    #[inline(always)]
    pub unsafe fn set_kernel_cr3(val: u64) {
        percpu_write_u64(PERCPU_KERNEL_CR3_OFFSET, val);
    }

    /// Enter hard IRQ context (increment HARDIRQ count).
    #[inline(always)]
    pub unsafe fn irq_enter() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(1 << HARDIRQ_SHIFT, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Exit hard IRQ context (decrement HARDIRQ count).
    #[inline(always)]
    pub unsafe fn irq_exit() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(1 << HARDIRQ_SHIFT, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Set the PREEMPT_ACTIVE flag.
    #[inline(always)]
    pub unsafe fn set_preempt_active() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_or(PREEMPT_ACTIVE, Ordering::Relaxed);
        }
    }

    /// Clear the PREEMPT_ACTIVE flag.
    #[inline(always)]
    pub unsafe fn clear_preempt_active() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_and(!PREEMPT_ACTIVE, Ordering::Relaxed);
        }
    }

    /// Get the idle thread pointer.
    #[inline(always)]
    pub fn idle_thread_ptr() -> *mut u8 {
        percpu_read_u64(PERCPU_IDLE_THREAD_OFFSET) as *mut u8
    }

    /// Set the idle thread pointer.
    #[inline(always)]
    pub unsafe fn set_idle_thread_ptr(ptr: *mut u8) {
        percpu_write_u64(PERCPU_IDLE_THREAD_OFFSET, ptr as u64);
    }

    /// Get the TSS/equivalent pointer.
    /// On ARM64 this might point to an exception-handling context structure.
    #[inline(always)]
    pub fn tss_ptr() -> *mut u8 {
        percpu_read_u64(PERCPU_TSS_OFFSET) as *mut u8
    }

    /// Set the TSS/equivalent pointer.
    #[inline(always)]
    pub unsafe fn set_tss_ptr(ptr: *mut u8) {
        percpu_write_u64(PERCPU_TSS_OFFSET, ptr as u64);
    }

    /// Get the user SP scratch value.
    #[inline(always)]
    pub fn user_rsp_scratch() -> u64 {
        percpu_read_u64(PERCPU_USER_RSP_SCRATCH_OFFSET)
    }

    /// Set the user SP scratch value.
    ///
    /// Refuses an address belonging to another CPU's per-CPU stack slot; see
    /// `percpu_stack_install_permitted`.
    #[inline(always)]
    #[track_caller]
    pub unsafe fn set_user_rsp_scratch(sp: u64) {
        if !percpu_stack_install_permitted(sp, Location::caller()) {
            return;
        }
        percpu_write_u64(PERCPU_USER_RSP_SCRATCH_OFFSET, sp);
        crate::task::percpu_stack_oracle::note_stack_top_install(sp);
    }

    /// Install `sp` into BOTH per-CPU return-SP words and report the address
    /// that is now actually installed.
    ///
    /// This is the fail-closed install for the idle pivot. Its callers do not
    /// merely publish the address, they then `mov sp, <address>` and run on it,
    /// so ignoring a refusal is not an option: the refused value would become
    /// the live stack pointer while the two per-CPU words still named something
    /// else. The #635 acceptance battery observed exactly that — a refused
    /// `percpu_kernel_stack_top(0)` reaching SP on CPU 3.
    ///
    /// On refusal the fallback is this CPU's own exception-stack top, derived
    /// from the same `cpu_id()` the refusal used. That makes the fallback
    /// own-slot by construction, so it cannot itself be foreign, and it needs
    /// no second adjudication.
    ///
    /// The refusal record is still emitted by `percpu_stack_install_permitted`.
    /// Nothing here suppresses it, and nothing here redirects execution — the
    /// caller is told what it may run on and decides for itself.
    #[inline]
    #[track_caller]
    pub unsafe fn install_idle_return_sp(sp: u64) -> u64 {
        let granted = if percpu_stack_install_permitted(sp, Location::caller()) {
            sp
        } else {
            percpu_kernel_stack_top(<Self as PerCpuOps>::cpu_id() as usize)
        };
        percpu_write_u64(PERCPU_KERNEL_STACK_TOP_OFFSET, granted);
        percpu_write_u64(PERCPU_USER_RSP_SCRATCH_OFFSET, granted);
        crate::task::percpu_stack_oracle::note_stack_top_install(granted);
        granted
    }

    /// Get the softirq pending bitmap.
    #[inline(always)]
    pub fn softirq_pending() -> u32 {
        percpu_read_u32(PERCPU_SOFTIRQ_PENDING_OFFSET)
    }

    /// Set a softirq pending bit.
    #[inline(always)]
    pub unsafe fn raise_softirq(nr: u32) {
        debug_assert!(nr < 32, "Invalid softirq number");
        if let Some(atomic) = percpu_atomic_u32(PERCPU_SOFTIRQ_PENDING_OFFSET) {
            atomic.fetch_or(1 << nr, Ordering::Relaxed);
        }
    }

    /// Clear a softirq pending bit.
    #[inline(always)]
    pub unsafe fn clear_softirq(nr: u32) {
        debug_assert!(nr < 32, "Invalid softirq number");
        if let Some(atomic) = percpu_atomic_u32(PERCPU_SOFTIRQ_PENDING_OFFSET) {
            atomic.fetch_and(!(1 << nr), Ordering::Relaxed);
        }
    }

    /// Get dispatch ELR (per-CPU copy for race-free ERET).
    #[inline(always)]
    pub fn dispatch_elr() -> u64 {
        percpu_read_u64(PERCPU_DISPATCH_ELR_OFFSET)
    }

    /// Set dispatch ELR (per-CPU copy for race-free ERET).
    #[inline(always)]
    pub unsafe fn set_dispatch_elr(val: u64) {
        percpu_write_u64(PERCPU_DISPATCH_ELR_OFFSET, val);
    }

    /// Get dispatch SPSR (per-CPU copy for race-free ERET).
    #[inline(always)]
    pub fn dispatch_spsr() -> u64 {
        percpu_read_u64(PERCPU_DISPATCH_SPSR_OFFSET)
    }

    /// Set dispatch SPSR (per-CPU copy for race-free ERET).
    #[inline(always)]
    pub unsafe fn set_dispatch_spsr(val: u64) {
        percpu_write_u64(PERCPU_DISPATCH_SPSR_OFFSET, val);
    }

    /// Get exception cleanup context flag.
    #[inline(always)]
    pub fn exception_cleanup_context() -> bool {
        percpu_read_u8(PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET) != 0
    }

    /// Set exception cleanup context flag.
    #[inline(always)]
    pub unsafe fn set_exception_cleanup_context(value: bool) {
        percpu_write_u8(
            PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
            if value { 1 } else { 0 },
        );
    }

    /// Enter softirq context.
    #[inline(always)]
    pub unsafe fn softirq_enter() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(SOFTIRQ_OFFSET, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Exit softirq context.
    #[inline(always)]
    pub unsafe fn softirq_exit() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(SOFTIRQ_OFFSET, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Enter NMI context (on ARM64, this is FIQ or equivalent).
    #[inline(always)]
    pub unsafe fn nmi_enter() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(1 << NMI_SHIFT, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Exit NMI context.
    #[inline(always)]
    pub unsafe fn nmi_exit() {
        compiler_fence(Ordering::Acquire);
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(1 << NMI_SHIFT, Ordering::Relaxed);
        }
        compiler_fence(Ordering::Release);
    }

    /// Check if in NMI context.
    #[inline(always)]
    pub fn in_nmi() -> bool {
        let count = Self::preempt_count();
        (count & NMI_MASK) != 0
    }
}

// =============================================================================
// Per-CPU initialization and setup
// =============================================================================

/// Initialize per-CPU data for the current CPU
///
/// This should be called early in boot for each CPU core.
/// The base pointer should point to a PerCpuData structure.
#[inline]
pub unsafe fn init_percpu(base: u64, cpu_id: u64) {
    // Set TPIDR_EL1 to point to our per-CPU data
    write_tpidr_el1(base);

    // Initialize the CPU ID field
    core::ptr::write_volatile(
        (base as *mut u8).add(PERCPU_CPU_ID_OFFSET) as *mut u64,
        cpu_id,
    );

    // Initialize preempt_count to 0
    core::ptr::write_volatile(
        (base as *mut u8).add(PERCPU_PREEMPT_COUNT_OFFSET) as *mut u32,
        0,
    );
}

/// Get the raw per-CPU base pointer
#[inline]
pub fn percpu_base() -> u64 {
    read_tpidr_el1()
}

/// Check if per-CPU is initialized for this CPU
#[inline]
pub fn percpu_initialized() -> bool {
    read_tpidr_el1() != 0
}
