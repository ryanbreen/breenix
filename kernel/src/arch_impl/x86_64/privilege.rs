use crate::arch_impl::traits::PrivilegeLevel;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum X86PrivilegeLevel {
    Ring0,
    Ring3,
}

impl PrivilegeLevel for X86PrivilegeLevel {
    fn kernel() -> Self {
        X86PrivilegeLevel::Ring0
    }

    fn user() -> Self {
        X86PrivilegeLevel::Ring3
    }

    fn is_kernel(&self) -> bool {
        matches!(self, X86PrivilegeLevel::Ring0)
    }

    fn is_user(&self) -> bool {
        matches!(self, X86PrivilegeLevel::Ring3)
    }
}
