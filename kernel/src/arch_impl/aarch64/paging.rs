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

    #[inline(always)]
    fn read_root() -> u64 {
        let ttbr: u64;
        unsafe {
            core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr, options(nomem, nostack));
        }
        ttbr & 0x0000_FFFF_FFFF_F000
    }

    #[inline(always)]
    unsafe fn write_root(addr: u64) {
        let aligned = addr & 0x0000_FFFF_FFFF_F000;
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {0}",
            "dsb ish",
            "isb",
            in(reg) aligned,
            options(nostack)
        );
    }

    #[inline(always)]
    fn flush_tlb_page(addr: u64) {
        let page = addr >> 12;
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "tlbi vae1is, {0}",
                "dsb ish",
                "isb",
                in(reg) page,
                options(nostack)
            );
        }
    }

    #[inline(always)]
    fn flush_tlb_all() {
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                options(nostack)
            );
        }
    }
}
