//! Per-thread x87/SSE register state.
//!
//! The kernel is built soft-float and executes no x87 or SSE instruction, so
//! whatever these registers hold belongs to the last thread a dispatch handed
//! them to: the CPU's owner. Every place that changes a CPU's current thread
//! on the way back out of an interrupt (`switch_to_thread`, and the rollback
//! in `abort_dispatch_and_resume`) hands the registers over with `hand_over`:
//! the owner saves them, the incoming thread loads what it saved and becomes
//! the owner. Kernel threads and idle take part like any other thread; the
//! state they carry is never used.
//!
//! Owners are thread ids, and threads are found through the scheduler, so a
//! thread reaped since it last ran is skipped, never written.
//!
//! `cpu_init::init_fpu` enables x87 and SSE in XCR0 and nothing wider, which
//! is exactly the state FXSAVE covers.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::task::scheduler::{Scheduler, MAX_CPUS};

/// One FXSAVE image (Intel SDM Vol. 1, 10.5.1): 512 bytes, 16-byte aligned.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub struct FpuState([u8; 512]);

impl FpuState {
    /// The state a program starts with: x87 control word 0x037f, MXCSR
    /// 0x1f80 (every exception masked, round to nearest), every register and
    /// tag empty.
    pub const fn initial() -> Self {
        let mut image = [0u8; 512];
        image[0] = 0x7f;
        image[1] = 0x03;
        image[24] = 0x80;
        image[25] = 0x1f;
        Self(image)
    }

    /// The executing CPU's live state, for a thread that starts as a copy of
    /// the caller (fork, clone).
    pub fn capture() -> Self {
        let mut state = Self::initial();
        state.save();
        state
    }

    /// Store the executing CPU's x87/SSE registers here.
    pub fn save(&mut self) {
        // SAFETY: `self` is a 16-byte-aligned 512-byte buffer; FXSAVE writes
        // exactly that and touches no other memory or register.
        unsafe {
            core::arch::asm!(
                "fxsave64 [{}]",
                in(reg) self.0.as_mut_ptr(),
                options(nostack, preserves_flags)
            );
        }
    }

    /// Load the executing CPU's x87/SSE registers from here.
    pub fn restore(&self) {
        // SAFETY: the image is either `initial()` or an FXSAVE result, so
        // MXCSR holds no reserved bit and FXRSTOR cannot fault.
        unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) self.0.as_ptr(),
                options(nostack, preserves_flags, readonly)
            );
        }
    }
}

impl core::fmt::Debug for FpuState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("FpuState")
    }
}

/// No thread: a CPU before its first hand-over.
const NO_OWNER: u64 = u64::MAX;

/// The thread whose state each CPU's x87/SSE registers hold. Each entry is
/// read and written only by its own CPU, with interrupts masked.
static OWNER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(NO_OWNER) }; MAX_CPUS];

/// Give this CPU's x87/SSE registers to thread `incoming`: the thread holding
/// them saves them, `incoming` loads what it saved and becomes the owner.
/// Nothing happens when `incoming` already holds them. Before the CPU's first
/// hand-over the holder is `outgoing`, the thread the dispatch is leaving (a
/// thread that entered user mode without a dispatch, such as init, holds them
/// then).
///
/// Runs under the scheduler lock, with interrupts masked.
pub fn hand_over(sched: &mut Scheduler, outgoing: Option<u64>, incoming: u64) {
    let cpu = crate::per_cpu::cpu_id();
    let owner = match OWNER[cpu].load(Ordering::Relaxed) {
        NO_OWNER => outgoing,
        owner => Some(owner),
    };
    if owner == Some(incoming) {
        return;
    }
    if let Some(thread) = owner.and_then(|owner| sched.get_thread_mut(owner)) {
        thread.fpu.save();
    }
    if let Some(thread) = sched.get_thread(incoming) {
        thread.fpu.restore();
        OWNER[cpu].store(incoming, Ordering::Relaxed);
    }
}

/// Load the starting state into this CPU's registers, for a thread whose
/// program image was just replaced (exec).
pub fn reset_current() {
    FpuState::initial().restore();
}
