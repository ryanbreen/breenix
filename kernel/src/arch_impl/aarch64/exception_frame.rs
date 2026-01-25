//! AArch64 exception frame abstraction.
//!
//! Saved processor context on exceptions and interrupts.

#![allow(dead_code)] // HAL type - part of complete API

use crate::arch_impl::traits::InterruptFrame;

use super::privilege::Aarch64PrivilegeLevel;

#[repr(C)]
pub struct Aarch64ExceptionFrame {
    // General-purpose registers.
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,
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
    pub x29: u64,
    pub x30: u64,
    // Stack pointer (SP).
    pub sp: u64,
    // Program counter (ELR_EL1).
    pub pc: u64,
    // Saved PSTATE (SPSR_EL1).
    pub pstate: u64,
}

impl InterruptFrame for Aarch64ExceptionFrame {
    type Privilege = Aarch64PrivilegeLevel;

    fn instruction_pointer(&self) -> u64 {
        self.pc
    }

    fn stack_pointer(&self) -> u64 {
        self.sp
    }

    fn set_instruction_pointer(&mut self, addr: u64) {
        self.pc = addr;
    }

    fn set_stack_pointer(&mut self, addr: u64) {
        self.sp = addr;
    }

    fn privilege_level(&self) -> Self::Privilege {
        // Check PSTATE.M[3:0] - EL bits
        // EL0 = 0b0000, EL1 = 0b0100
        if (self.pstate & 0xF) == 0 {
            Aarch64PrivilegeLevel::EL0
        } else {
            Aarch64PrivilegeLevel::EL1
        }
    }
}
