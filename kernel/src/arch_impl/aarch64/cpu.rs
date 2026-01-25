//! ARM64 CPU operations.
//!
//! Handles interrupt enable/disable via the DAIF (Debug, SError, IRQ, FIQ) register,
//! and CPU halt via WFI (Wait For Interrupt).
//!
//! DAIF register layout:
//! - Bit 9 (D): Debug exception mask
//! - Bit 8 (A): SError (async) exception mask
//! - Bit 7 (I): IRQ mask (1 = masked/disabled, 0 = unmasked/enabled)
//! - Bit 6 (F): FIQ mask
//!
//! Instructions:
//! - `msr daifset, #imm`: Set specified DAIF bits (disable interrupts)
//! - `msr daifclr, #imm`: Clear specified DAIF bits (enable interrupts)
//! - `mrs reg, daif`: Read DAIF register
//! - `wfi`: Wait For Interrupt (low-power halt until interrupt)

#![allow(dead_code)]

use crate::arch_impl::traits::CpuOps;

/// DAIF bit positions
const DAIF_IRQ_BIT: u64 = 1 << 7;  // I bit
const DAIF_FIQ_BIT: u64 = 1 << 6;  // F bit

/// Immediate values for daifset/daifclr (bits 3:0 map to DAIF bits 9:6)
/// Bit 1 = I (IRQ), Bit 0 = F (FIQ)
const DAIF_IRQ_IMM: u32 = 0x2;  // Just IRQ
const DAIF_ALL_IMM: u32 = 0xF;  // D, A, I, F

pub struct Aarch64Cpu;

impl CpuOps for Aarch64Cpu {
    /// Enable IRQ interrupts by clearing the I bit in DAIF
    #[inline]
    unsafe fn enable_interrupts() {
        // daifclr with #2 clears the I bit (enables IRQs)
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }

    /// Disable IRQ interrupts by setting the I bit in DAIF
    #[inline]
    unsafe fn disable_interrupts() {
        // daifset with #2 sets the I bit (disables IRQs)
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }

    /// Check if IRQ interrupts are enabled (I bit is clear)
    #[inline]
    fn interrupts_enabled() -> bool {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        }
        // IRQs are enabled when the I bit (bit 7) is clear
        (daif & DAIF_IRQ_BIT) == 0
    }

    /// Halt the CPU until an interrupt occurs
    ///
    /// WFI (Wait For Interrupt) puts the CPU in a low-power state until
    /// an interrupt (or other wake event) occurs. The CPU will wake even
    /// if interrupts are masked, but won't take the interrupt.
    #[inline]
    fn halt() {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }

    /// Enable interrupts and halt in a single operation
    ///
    /// This is the ARM64 equivalent of x86's STI+HLT pattern.
    /// We enable IRQs then immediately WFI, ensuring we don't miss
    /// an interrupt that arrives between the two instructions.
    #[inline]
    fn halt_with_interrupts() {
        unsafe {
            // Enable IRQs and immediately wait
            // Any pending interrupt will be taken before WFI completes
            core::arch::asm!(
                "msr daifclr, #2",  // Enable IRQs
                "wfi",              // Wait for interrupt
                options(nomem, nostack)
            );
        }
    }

    /// Execute a closure with interrupts disabled.
    ///
    /// Saves the current DAIF state, disables IRQs, runs the closure,
    /// and restores the previous DAIF state.
    #[inline]
    fn without_interrupts<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // Save current DAIF state
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        }

        // Disable IRQs
        unsafe {
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        }

        // Execute the closure
        let result = f();

        // Restore previous DAIF state (only restore IRQ bit to avoid affecting other flags)
        if (daif & DAIF_IRQ_BIT) == 0 {
            // IRQs were enabled before, re-enable them
            unsafe {
                core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
            }
        }
        // If IRQs were disabled, leave them disabled (don't change anything)

        result
    }
}

// =============================================================================
// Additional CPU utilities
// =============================================================================

/// Read the current exception level (0-3)
#[inline]
pub fn current_el() -> u8 {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, currentel", out(reg) el, options(nomem, nostack));
    }
    // CurrentEL is in bits [3:2]
    ((el >> 2) & 0x3) as u8
}

/// Read the CPU ID from MPIDR_EL1
///
/// Returns the Aff0 field which is typically the CPU ID within a cluster.
#[inline]
pub fn cpu_id() -> u64 {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
    }
    // Aff0 is in bits [7:0], but we also check Aff1 for multi-cluster systems
    // For most systems, Aff0 alone is sufficient
    mpidr & 0xFF
}

/// Issue an Instruction Synchronization Barrier
///
/// Ensures all previous instructions have completed before continuing.
/// Similar to x86 LFENCE but stronger.
#[inline]
pub fn isb() {
    unsafe {
        core::arch::asm!("isb", options(nomem, nostack));
    }
}

/// Issue a Data Synchronization Barrier (full system)
///
/// Ensures all previous memory accesses have completed.
/// Similar to x86 MFENCE.
#[inline]
pub fn dsb_sy() {
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
    }
}

/// Issue a Data Memory Barrier (full system)
///
/// Ensures ordering of memory accesses.
#[inline]
pub fn dmb_sy() {
    unsafe {
        core::arch::asm!("dmb sy", options(nomem, nostack));
    }
}
