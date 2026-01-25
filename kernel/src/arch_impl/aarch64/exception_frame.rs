//! AArch64 exception frame abstraction.
//!
//! Saved processor context on exceptions and interrupts.
//!
//! This frame layout must match the register save/restore order in boot.S.
//! ARM64 syscall ABI: X8=syscall number, X0-X5=args, X0=return value.

#![allow(dead_code)] // HAL type - part of complete API

use crate::arch_impl::traits::{InterruptFrame, SyscallFrame};
use super::privilege::Aarch64PrivilegeLevel;

/// Exception frame matching the layout in boot.S sync_exception_handler.
///
/// The assembly saves registers as:
///   stp x0, x1, [sp, #0]      // x0 at offset 0
///   ...
///   stp x28, x29, [sp, #224]
///   stp x30, elr, [sp, #240]  // x30 at 240, elr at 248
///   str spsr, [sp, #256]      // spsr at 256
///
/// Total size: 272 bytes (34 u64 fields, but we only store 33 + padding)
#[repr(C)]
pub struct Aarch64ExceptionFrame {
    // General-purpose registers x0-x29 (saved by stp)
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,   // Syscall number (ARM64 ABI)
    pub x9: u64,
    pub x10: u64,
    pub x11: u64,
    pub x12: u64,
    pub x13: u64,
    pub x14: u64,
    pub x15: u64,
    pub x16: u64,
    pub x17: u64,
    pub x18: u64,
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,  // Frame pointer
    pub x30: u64,  // Link register (LR)
    pub elr: u64,  // Exception Link Register (return address)
    pub spsr: u64, // Saved Program Status Register
}

impl InterruptFrame for Aarch64ExceptionFrame {
    type Privilege = Aarch64PrivilegeLevel;

    fn instruction_pointer(&self) -> u64 {
        self.elr
    }

    fn stack_pointer(&self) -> u64 {
        // Note: The user SP is saved in SP_EL0 by the CPU, not in the frame.
        // We'd need to read SP_EL0 for the true user stack pointer.
        // For kernel exceptions, we can't easily access the pre-exception SP.
        0 // Placeholder - need proper handling
    }

    fn set_instruction_pointer(&mut self, addr: u64) {
        self.elr = addr;
    }

    fn set_stack_pointer(&mut self, _addr: u64) {
        // TODO: For signal delivery, we need to modify SP_EL0
        // This requires assembly support
    }

    fn privilege_level(&self) -> Self::Privilege {
        // SPSR_EL1.M[3:0] contains the exception level and SP selection
        // EL0t = 0b0000 (EL0 with SP_EL0)
        // EL1t = 0b0100 (EL1 with SP_EL0)
        // EL1h = 0b0101 (EL1 with SP_EL1)
        let mode = self.spsr & 0xF;
        if mode == 0 {
            Aarch64PrivilegeLevel::EL0
        } else {
            Aarch64PrivilegeLevel::EL1
        }
    }
}

impl SyscallFrame for Aarch64ExceptionFrame {
    /// ARM64 ABI: syscall number in X8
    fn syscall_number(&self) -> u64 {
        self.x8
    }

    /// ARM64 ABI: first argument in X0
    fn arg1(&self) -> u64 {
        self.x0
    }

    /// ARM64 ABI: second argument in X1
    fn arg2(&self) -> u64 {
        self.x1
    }

    /// ARM64 ABI: third argument in X2
    fn arg3(&self) -> u64 {
        self.x2
    }

    /// ARM64 ABI: fourth argument in X3
    fn arg4(&self) -> u64 {
        self.x3
    }

    /// ARM64 ABI: fifth argument in X4
    fn arg5(&self) -> u64 {
        self.x4
    }

    /// ARM64 ABI: sixth argument in X5
    fn arg6(&self) -> u64 {
        self.x5
    }

    /// ARM64 ABI: return value in X0
    fn set_return_value(&mut self, value: u64) {
        self.x0 = value;
    }

    /// ARM64 ABI: return value in X0
    fn return_value(&self) -> u64 {
        self.x0
    }
}
