//! AArch64 privilege level abstraction.
//!
//! Note: This is part of the complete HAL API. The enum variants
//! represent ARM64 exception levels (EL0/EL1).

#![allow(dead_code)] // HAL type - part of complete API

use crate::arch_impl::traits::PrivilegeLevel;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Aarch64PrivilegeLevel {
    EL0,
    EL1,
}

impl PrivilegeLevel for Aarch64PrivilegeLevel {
    fn kernel() -> Self {
        Aarch64PrivilegeLevel::EL1
    }

    fn user() -> Self {
        Aarch64PrivilegeLevel::EL0
    }

    fn is_kernel(&self) -> bool {
        matches!(self, Aarch64PrivilegeLevel::EL1)
    }

    fn is_user(&self) -> bool {
        matches!(self, Aarch64PrivilegeLevel::EL0)
    }
}
