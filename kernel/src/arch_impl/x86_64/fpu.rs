//! Per-thread x87/SSE register state.
//!
//! The kernel is built soft-float and executes no x87 or SSE instruction, so
//! whatever these registers hold belongs to the thread that is current on the
//! CPU. Every place that changes a CPU's current thread on the way back out of
//! an interrupt (`switch_to_thread`, and the rollback in
//! `abort_dispatch_and_resume`) hands the registers over with `hand_over_to`:
//! the outgoing thread saves them, the incoming one loads what it saved.
//! Kernel threads and idle take part like any other thread; the state they
//! carry is never used.
//!
//! `cpu_init::init_fpu` enables x87 and SSE in XCR0 and nothing wider, which
//! is exactly the state FXSAVE covers.

use crate::arch_impl::PerCpuOps;
use crate::task::thread::Thread;

use super::percpu::X86PerCpu;

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

/// Give this CPU's x87/SSE registers to `incoming`, which is about to become
/// its current thread: the per-CPU current thread saves what it left in them
/// and `incoming`'s saved state is loaded. Nothing happens when `incoming` is
/// already current.
///
/// # Safety
/// Interrupts are masked, and the per-CPU current thread pointer is null or
/// names a live scheduler thread distinct from any reference the caller holds
/// other than `incoming`.
pub unsafe fn hand_over_to(incoming: &mut Thread) {
    let outgoing = X86PerCpu::current_thread_ptr() as *mut Thread;
    if core::ptr::eq(outgoing, incoming) {
        return;
    }
    if let Some(outgoing) = outgoing.as_mut() {
        outgoing.fpu.save();
    }
    incoming.fpu.restore();
}

/// Load the starting state into this CPU's registers, for a thread whose
/// program image was just replaced (exec).
pub fn reset_current() {
    FpuState::initial().restore();
}
