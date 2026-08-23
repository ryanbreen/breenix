//! Feature-gated AArch64 self-checks for per-CPU exception-stack ownership.
//!
//! Each CPU owns one `PERCPU_STACK_STRIDE` slot in the per-CPU stack region;
//! the upper half of a slot is that CPU's idle/exception stack. Nothing in the
//! kernel checks, when a stack top is written into a CPU's per-CPU data, that
//! the address belongs to *that* CPU's slot rather than another CPU's. Two CPUs
//! can therefore end up building exception frames on one stack, and the
//! resulting overwrite of live kernel data surfaces much later as an
//! unexplained fault.
//!
//! This module only MEASURES that invariant. It changes no kernel behaviour:
//! the two in-path recorders are pure atomics, the stimulus goes through the
//! ordinary public setters, and with `percpu_stack_custody_oracle` off every
//! entry point here is an empty inline no-op.
//!
//! Four probes, all reported once per boot:
//!
//! - **A** — plant a recognisable image in an offline CPU's exception stack,
//!   install that stack top on a different CPU through the ordinary setters,
//!   and see whether the write is accepted and the image is then overwritten by
//!   a register-save frame.
//! - **B** — the control arm: a CPU's own slot top and an ordinary heap-backed
//!   thread kernel stack must both still be accepted. Shipped in the same build
//!   so probe A passing later cannot be the result of a blanket refusal.
//! - **C** — passive census: does any slot get named by a CPU that does not own
//!   it, and how many CPUs ever name one slot at once.
//! - **D** — is thread id 0 a live thread id (the `swapper/0` overwrite), and
//!   does resolving tid 0 through the scheduler's idle lookup succeed.

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
use crate::arch_impl::aarch64::constants::{
    percpu_kernel_stack_top, percpu_stack_region_base, MAX_CPUS, PERCPU_STACK_REGION_SIZE,
    PERCPU_STACK_STRIDE,
};
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
use crate::arch_impl::aarch64::percpu::Aarch64PerCpu;
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
use crate::arch_impl::traits::PerCpuOps;

// ============================================================================
// Shared geometry
// ============================================================================

/// Size in bytes of the register-save frame the exception vectors carve off the
/// current SP (`sub sp, sp, #272`). It is also the size of probe A's image.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const SAVE_FRAME_BYTES: u64 = 272;

/// Number of u64 words in one save frame.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const IMAGE_WORDS: usize = (SAVE_FRAME_BYTES / 8) as usize;

/// High half of probe A's fill word. The low bits carry the word index, so a
/// partial overwrite is still readable.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const IMAGE_PATTERN: u64 = 0xA11E_0000_0000_0000;

/// Byte offset of `Aarch64ExceptionFrame::elr` within a save frame.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const FRAME_ELR_OFFSET: u64 = 248;

/// Byte offset of `Aarch64ExceptionFrame::spsr` within a save frame.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const FRAME_SPSR_OFFSET: u64 = 256;

// A save frame based at `top - 272` keeps its saved return address 24 bytes
// below `top` and its saved processor state 16 bytes below `top`.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const _: () = assert!(SAVE_FRAME_BYTES - FRAME_ELR_OFFSET == 24);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const _: () = assert!(SAVE_FRAME_BYTES - FRAME_SPSR_OFFSET == 16);

// ============================================================================
// Probe A — is a cross-CPU stack top accepted, and what happens next
// ============================================================================

/// The offline CPU whose slot probe A borrows; `NO_CPU` when none exists.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_TARGET_CPU: AtomicU64 = AtomicU64::new(NO_CPU);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_STIMULUS_CPU: AtomicU64 = AtomicU64::new(NO_CPU);
/// 0 = not armed, 1 = armed, 2 = fired and disarmed. Claimed with a CAS so the
/// stimulus happens exactly once for the whole boot.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_ARM_VERIFIED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_STIMULI: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_ACCEPTED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_OVERWRITTEN: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_ELR_SLOT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_SPSR_SLOT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static A_OVERLAY: AtomicU64 = AtomicU64::new(0);

/// Sentinel for "no CPU": `MAX_CPUS` is small, so any out-of-range value works
/// and every consumer range-checks before indexing.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
const NO_CPU: u64 = u64::MAX;

// ============================================================================
// Probe B — the control arm
// ============================================================================

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static B_CPU: AtomicU64 = AtomicU64::new(NO_CPU);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static B_OWN_TOP_ACCEPTED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static B_HEAP_STACK_ACCEPTED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static B_TARGET_IMAGE_DISTURBED: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Probe C — passive occupancy census
// ============================================================================

/// One bitmap per slot: bit `c` is set while CPU `c` names that slot as its
/// kernel stack top or user-SP scratch.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static OCCUPANCY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static C_OBSERVATIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static C_FOREIGN_OCCUPANCY: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static C_MAX_CONCURRENT: AtomicU64 = AtomicU64::new(0);
/// Only meaningful when `C_FOREIGN_OCCUPANCY` is nonzero.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static C_WORST_SLOT: AtomicU64 = AtomicU64::new(0);
/// Only meaningful when `C_FOREIGN_OCCUPANCY` is nonzero.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static C_WORST_CPU: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Probe D — is thread id 0 a live thread id
// ============================================================================

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static D_SWAPPER_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
static D_ZERO_RESOLVES: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// In-path recorders — atomics only, no locks, no formatting
// ============================================================================

/// Record one per-CPU stack-top install for probe C's occupancy census.
///
/// Appended to `Aarch64PerCpu::set_kernel_stack_top` and
/// `Aarch64PerCpu::set_user_rsp_scratch` after the value has been written.
/// Runs on the dispatch path, so it is atomics only.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
#[inline]
pub fn note_stack_top_install(value: u64) {
    let cpu = Aarch64PerCpu::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    let bit = 1u64 << cpu;

    let Some(slot) = slot_of_stack_top(value) else {
        // An ordinary heap-backed thread kernel stack: this CPU occupies no
        // slot in the per-CPU stack region at all.
        for other in OCCUPANCY.iter() {
            other.fetch_and(!bit, Ordering::AcqRel);
        }
        return;
    };

    let occupancy = OCCUPANCY[slot].fetch_or(bit, Ordering::AcqRel) | bit;
    for (index, other) in OCCUPANCY.iter().enumerate() {
        if index != slot {
            other.fetch_and(!bit, Ordering::AcqRel);
        }
    }
    C_OBSERVATIONS.fetch_add(1, Ordering::AcqRel);
    C_MAX_CONCURRENT.fetch_max(u64::from(occupancy.count_ones()), Ordering::AcqRel);
    if slot != cpu {
        C_WORST_SLOT.store(slot as u64, Ordering::Release);
        C_WORST_CPU.store(cpu as u64, Ordering::Release);
        C_FOREIGN_OCCUPANCY.fetch_add(1, Ordering::Release);
    }
}

/// Probe A's stimulus, appended to the userspace-dispatch stack-top install.
///
/// When armed, exactly once for the whole boot and never on the target CPU
/// itself, install the *target* CPU's exception-stack top on THIS CPU through
/// the ordinary public setters, and record whether it was accepted. The next
/// context switch installs this CPU's proper stack top by itself, so the
/// stimulus is self-limiting.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
#[inline]
pub fn note_user_dispatch_stack_install(cpu_id: usize) {
    if A_ARMED.load(Ordering::Acquire) != 1 {
        return;
    }
    let target = A_TARGET_CPU.load(Ordering::Acquire);
    if target >= MAX_CPUS as u64 || target == cpu_id as u64 {
        return;
    }
    if A_ARMED
        .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let f = percpu_kernel_stack_top(target as usize);
    unsafe {
        Aarch64PerCpu::set_kernel_stack_top(f);
        Aarch64PerCpu::set_user_rsp_scratch(f);
    }
    A_STIMULUS_CPU.store(cpu_id as u64, Ordering::Release);
    if Aarch64PerCpu::kernel_stack_top() == f {
        A_ACCEPTED.fetch_add(1, Ordering::AcqRel);
    }
    // Published last: the reporting thread waits on this counter, so seeing it
    // nonzero means the fields above are already visible.
    A_STIMULI.fetch_add(1, Ordering::Release);
}

// ============================================================================
// Geometry helpers
// ============================================================================

/// The slot a *stack top* names, or `None` when the value is outside the
/// per-CPU stack region.
///
/// Stack tops are exclusive upper bounds: `percpu_kernel_stack_top(cpu)` is
/// `base + (cpu + 1) * PERCPU_STACK_STRIDE`, one past the last byte of slot
/// `cpu`. Attribution therefore uses the last addressable byte below the top.
/// A half-open `[base, base + size)` test with a plain `(value - base) / stride`
/// would put every legitimate own-slot top in the NEXT slot and would push the
/// last slot's top out of the region entirely.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
#[inline]
fn slot_of_stack_top(value: u64) -> Option<usize> {
    let base = percpu_stack_region_base();
    let end = base + PERCPU_STACK_REGION_SIZE as u64;
    if value <= base || value > end {
        return None;
    }
    Some(((value - 1 - base) / PERCPU_STACK_STRIDE) as usize)
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
#[inline]
fn image_word(index: usize) -> u64 {
    IMAGE_PATTERN | index as u64
}

/// Fill `[top - 272, top)` with the recognisable image.
///
/// # Safety
/// `top` must be the top of a mapped, idle per-CPU exception stack.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
unsafe fn write_target_image(top: u64) {
    let bottom = top - SAVE_FRAME_BYTES;
    for index in 0..IMAGE_WORDS {
        core::ptr::write_volatile((bottom + (index as u64) * 8) as *mut u64, image_word(index));
    }
}

/// Count how many of the image's words no longer match.
///
/// # Safety
/// `top` must be the top of a mapped per-CPU exception stack.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
unsafe fn count_image_mismatches(top: u64) -> u64 {
    let bottom = top - SAVE_FRAME_BYTES;
    let mut mismatches = 0;
    for index in 0..IMAGE_WORDS {
        if core::ptr::read_volatile((bottom + (index as u64) * 8) as *const u64)
            != image_word(index)
        {
            mismatches += 1;
        }
    }
    mismatches
}

/// The highest CPU index that is not online. Its slot is reserved, mapped and
/// completely unused, which is why probe A borrows it.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn select_target_cpu() -> Option<usize> {
    (0..MAX_CPUS)
        .rev()
        .find(|&cpu| !crate::arch_impl::aarch64::smp::is_cpu_online(cpu))
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn monotonic_now_ms() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds
        .saturating_mul(1_000)
        .saturating_add(nanos / 1_000_000)
}

// ============================================================================
// Probe bodies driven from the oracle kernel thread
// ============================================================================

/// Probe B: save / set / read back / restore this CPU's kernel stack top for
/// its own slot top and for the heap-backed stack it arrived with.
///
/// The whole sequence runs with interrupts masked so no exception can be taken
/// while a temporary value is installed.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn run_control_arm(target_image_top: Option<u64>) {
    crate::arch_without_interrupts(|| {
        let cpu = Aarch64PerCpu::cpu_id() as usize;
        B_CPU.store(cpu as u64, Ordering::Release);
        if cpu >= MAX_CPUS {
            return;
        }

        let saved = Aarch64PerCpu::kernel_stack_top();
        let own_top = percpu_kernel_stack_top(cpu);

        unsafe { Aarch64PerCpu::set_kernel_stack_top(own_top) };
        let own_top_accepted = u64::from(Aarch64PerCpu::kernel_stack_top() == own_top);
        unsafe { Aarch64PerCpu::set_kernel_stack_top(saved) };

        unsafe { Aarch64PerCpu::set_kernel_stack_top(saved) };
        let heap_stack_accepted = u64::from(Aarch64PerCpu::kernel_stack_top() == saved);

        B_OWN_TOP_ACCEPTED.store(own_top_accepted, Ordering::Release);
        B_HEAP_STACK_ACCEPTED.store(heap_stack_accepted, Ordering::Release);
    });

    if let Some(top) = target_image_top {
        B_TARGET_IMAGE_DISTURBED.store(
            unsafe { count_image_mismatches(top) },
            Ordering::Release,
        );
    }
}

/// Probe D: read CPU 0's registered idle tid and whether tid 0 resolves through
/// the scheduler's own idle lookup.
#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn run_zero_tid_probe() {
    if let Some((swapper_tid, zero_resolves)) = super::scheduler::zero_tid_idle_probe() {
        D_SWAPPER_TID.store(swapper_tid, Ordering::Release);
        D_ZERO_RESOLVES.store(u64::from(zero_resolves), Ordering::Release);
    }
}

// ============================================================================
// Reports
// ============================================================================

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn cpu_field(value: u64) -> Option<u64> {
    (value < MAX_CPUS as u64).then_some(value)
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn report_cross_cpu_install() {
    let target_cpu = A_TARGET_CPU.load(Ordering::Acquire);
    let stimulus_cpu = A_STIMULUS_CPU.load(Ordering::Acquire);
    let arm_verified = A_ARM_VERIFIED.load(Ordering::Acquire);
    let stimuli = A_STIMULI.load(Ordering::Acquire);
    let accepted = A_ACCEPTED.load(Ordering::Acquire);
    let overwritten = A_OVERWRITTEN.load(Ordering::Acquire);
    let elr_slot = A_ELR_SLOT.load(Ordering::Acquire);
    let spsr_slot = A_SPSR_SLOT.load(Ordering::Acquire);
    let overlay = A_OVERLAY.load(Ordering::Acquire);

    // No offline slot to borrow means the probe never ran; it must not pass
    // silently. `stimuli == 0` is likewise a failure: a probe that never fired
    // proves nothing about the invariant.
    let passed = cpu_field(target_cpu).is_some()
        && arm_verified == 1
        && stimuli > 0
        && accepted == 0
        && overwritten == 0
        && overlay == 0;

    match (cpu_field(target_cpu), cpu_field(stimulus_cpu)) {
        (Some(target), Some(stimulus)) => crate::serial_println!(
            "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu={}:stimulus_cpu={}:arm_verified={}:stimuli={}:accepted={}:overwritten={}:elr_slot=0x{:x}:spsr_slot=0x{:x}:overlay={}:{}]",
            target,
            stimulus,
            arm_verified,
            stimuli,
            accepted,
            overwritten,
            elr_slot,
            spsr_slot,
            overlay,
            if passed { "PASS" } else { "FAIL" },
        ),
        (Some(target), None) => crate::serial_println!(
            "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu={}:stimulus_cpu=none:arm_verified={}:stimuli={}:accepted={}:overwritten={}:elr_slot=0x{:x}:spsr_slot=0x{:x}:overlay={}:{}]",
            target,
            arm_verified,
            stimuli,
            accepted,
            overwritten,
            elr_slot,
            spsr_slot,
            overlay,
            if passed { "PASS" } else { "FAIL" },
        ),
        _ => crate::serial_println!(
            "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=none:stimulus_cpu=none:arm_verified={}:stimuli={}:accepted={}:overwritten={}:elr_slot=0x{:x}:spsr_slot=0x{:x}:overlay={}:FAIL]",
            arm_verified,
            stimuli,
            accepted,
            overwritten,
            elr_slot,
            spsr_slot,
            overlay,
        ),
    }
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn report_control_arm() {
    let cpu = B_CPU.load(Ordering::Acquire);
    let own_top_accepted = B_OWN_TOP_ACCEPTED.load(Ordering::Acquire);
    let heap_stack_accepted = B_HEAP_STACK_ACCEPTED.load(Ordering::Acquire);
    let target_image_disturbed = B_TARGET_IMAGE_DISTURBED.load(Ordering::Acquire);
    let passed = own_top_accepted == 1 && heap_stack_accepted == 1;

    match cpu_field(cpu) {
        Some(cpu) => crate::serial_println!(
            "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu={}:own_top_accepted={}:heap_stack_accepted={}:target_image_disturbed={}:{}]",
            cpu,
            own_top_accepted,
            heap_stack_accepted,
            target_image_disturbed,
            if passed { "PASS" } else { "FAIL" },
        ),
        None => crate::serial_println!(
            "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=none:own_top_accepted={}:heap_stack_accepted={}:target_image_disturbed={}:FAIL]",
            own_top_accepted,
            heap_stack_accepted,
            target_image_disturbed,
        ),
    }
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn report_occupancy_census() {
    let observations = C_OBSERVATIONS.load(Ordering::Acquire);
    let foreign_occupancy = C_FOREIGN_OCCUPANCY.load(Ordering::Acquire);
    let max_concurrent = C_MAX_CONCURRENT.load(Ordering::Acquire);
    let worst_slot = C_WORST_SLOT.load(Ordering::Acquire);
    let worst_cpu = C_WORST_CPU.load(Ordering::Acquire);
    let passed = observations > 0 && foreign_occupancy == 0;

    crate::serial_println!(
        "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots={}:observations={}:foreign_occupancy={}:max_concurrent={}:worst_slot={}:worst_cpu={}:{}]",
        MAX_CPUS,
        observations,
        foreign_occupancy,
        max_concurrent,
        worst_slot,
        worst_cpu,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn report_zero_tid() {
    let swapper_tid = D_SWAPPER_TID.load(Ordering::Acquire);
    let zero_resolves = D_ZERO_RESOLVES.load(Ordering::Acquire);
    let passed = swapper_tid != 0 && zero_resolves == 0;

    crate::serial_println!(
        "[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid={}:zero_resolves={}:{}]",
        swapper_tid,
        zero_resolves,
        if passed { "PASS" } else { "FAIL" },
    );
}

// ============================================================================
// Driver
// ============================================================================

#[cfg(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
))]
fn percpu_stack_oracle_thread() {
    /// How long to wait for a userspace dispatch to carry the stimulus.
    const STIMULUS_WAIT_CAP_MS: u64 = 14_000;
    /// How long to let the stimulated CPU take exceptions on the borrowed stack
    /// before reading the image back.
    const SETTLE_AFTER_STIMULUS_MS: u64 = 2_000;

    // 1. Pick the offline slot and lay down the recognisable image. Reading all
    //    of it back proves the region is really mapped and really idle.
    let target = select_target_cpu();
    let target_image_top = target.map(percpu_kernel_stack_top);
    if let (Some(cpu), Some(top)) = (target, target_image_top) {
        A_TARGET_CPU.store(cpu as u64, Ordering::Release);
        unsafe { write_target_image(top) };
        let verified = u64::from(unsafe { count_image_mismatches(top) } == 0);
        A_ARM_VERIFIED.store(verified, Ordering::Release);
    }

    // 2. The control arm runs BEFORE the stimulus is armed, so its
    //    target_image_disturbed reading is about probe B alone.
    run_control_arm(target_image_top);

    // 3. Arm the stimulus. An unverified image is not armed: planting a stack
    //    pointer in a slot we could not read back is not a measurement.
    if A_ARM_VERIFIED.load(Ordering::Acquire) == 1 {
        A_ARMED.store(1, Ordering::Release);
    }

    // 4. Wait for the stimulus to fire, then let the CPU run on the borrowed
    //    stack long enough to take exceptions on it.
    let start_ms = monotonic_now_ms();
    let mut stimulus_seen_ms = 0;
    loop {
        let now_ms = monotonic_now_ms();
        if A_STIMULI.load(Ordering::Acquire) != 0 {
            stimulus_seen_ms = now_ms.max(1);
            break;
        }
        if now_ms.saturating_sub(start_ms) >= STIMULUS_WAIT_CAP_MS {
            break;
        }
        super::strand_oracle::sleep_sample_period();
    }
    while stimulus_seen_ms != 0
        && monotonic_now_ms().saturating_sub(stimulus_seen_ms) < SETTLE_AFTER_STIMULUS_MS
    {
        super::strand_oracle::sleep_sample_period();
    }

    // 5. Read the image back and classify what replaced it.
    if let Some(top) = target_image_top {
        let overwritten = unsafe { count_image_mismatches(top) };
        let frame_base = top - SAVE_FRAME_BYTES;
        let elr_slot =
            unsafe { core::ptr::read_volatile((frame_base + FRAME_ELR_OFFSET) as *const u64) };
        let spsr_slot =
            unsafe { core::ptr::read_volatile((frame_base + FRAME_SPSR_OFFSET) as *const u64) };
        A_OVERWRITTEN.store(overwritten, Ordering::Release);
        A_ELR_SLOT.store(elr_slot, Ordering::Release);
        A_SPSR_SLOT.store(spsr_slot, Ordering::Release);
        A_OVERLAY.store(
            u64::from(overwritten > 0 && (elr_slot >> 48) == 0xffff && (spsr_slot & 0xF) == 0),
            Ordering::Release,
        );
    }

    run_zero_tid_probe();

    report_cross_cpu_install();
    report_control_arm();
    report_occupancy_census();
    report_zero_tid();
}

/// Start the per-CPU stack custody probes. With
/// `percpu_stack_custody_oracle` off this emits no thread and installs no hook.
pub fn start() {
    #[cfg(all(
        target_arch = "aarch64",
        feature = "percpu_stack_custody_oracle",
        feature = "boot_tests"
    ))]
    {
        let _ = super::kthread::kthread_run(percpu_stack_oracle_thread, "percpu-stack-oracle");
    }
}

// ============================================================================
// Disabled build: every entry point is an empty inline no-op
// ============================================================================

#[cfg(not(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
)))]
#[inline(always)]
pub fn note_stack_top_install(_value: u64) {}

#[cfg(not(all(
    target_arch = "aarch64",
    feature = "percpu_stack_custody_oracle",
    feature = "boot_tests"
)))]
#[inline(always)]
pub fn note_user_dispatch_stack_install(_cpu_id: usize) {}
