//! ARM64 4-level page tables using TTBR0/TTBR1 (lower/upper address spaces).

#![allow(dead_code)]

use crate::arch_impl::traits::{PageFlags, PageTableOps};
use core::ops::BitOr;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Aarch64PageFlags(u64);

impl PageFlags for Aarch64PageFlags {
    fn empty() -> Self {
        Self(0)
    }

    fn present() -> Self {
        Self(1) // Valid bit
    }

    fn writable() -> Self {
        Self(0) // TODO: AP bits
    }

    fn user_accessible() -> Self {
        Self(0) // TODO: AP[1]
    }

    fn no_execute() -> Self {
        Self(0) // TODO: UXN/PXN
    }

    fn cow_marker() -> Self {
        Self(0) // SW bit
    }

    fn no_cache() -> Self {
        Self(0) // TODO: AttrIndx
    }

    fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for Aarch64PageFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

pub struct Aarch64PageTableOps;

impl PageTableOps for Aarch64PageTableOps {
    type Flags = Aarch64PageFlags;
    const PAGE_LEVELS: usize = 4;
    const PAGE_SIZE: usize = 4096;
    const ENTRIES_PER_TABLE: usize = 512;

    fn read_root() -> u64 {
        unimplemented!("ARM64: read_root (TTBR0) not yet implemented")
    }

    unsafe fn write_root(addr: u64) {
        let _ = addr;
        unimplemented!("ARM64: write_root (TTBR0) not yet implemented")
    }

    fn flush_tlb_page(addr: u64) {
        let _ = addr;
        unimplemented!("ARM64: flush_tlb_page not yet implemented")
    }

    fn flush_tlb_all() {
        unimplemented!("ARM64: flush_tlb_all not yet implemented")
    }
}
