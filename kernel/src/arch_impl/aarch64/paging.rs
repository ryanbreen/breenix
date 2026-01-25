//! ARM64 4-level page tables using TTBR0/TTBR1 (lower/upper address spaces).
//!
//! This module provides the low-level page table operations for ARM64.
//! It implements the PageTableOps trait for integration with the HAL,
//! and provides ARM64-specific page flag constants.
//!
//! ## ARM64 Descriptor Format
//!
//! ARM64 uses different descriptor formats at different levels:
//! - L0-L2: Can be table descriptors (next level) or block descriptors (1GB/2MB)
//! - L3: Page descriptors (4KB pages)
//!
//! Attribute bits:
//! - Bits 0: Valid
//! - Bit 1: Table (1) vs Block (0) for L0-L2
//! - Bits 2-4: AttrIndx (MAIR index)
//! - Bit 5: NS (Non-Secure)
//! - Bits 6-7: AP[2:1] (Access Permissions)
//! - Bits 8-9: SH (Shareability)
//! - Bit 10: AF (Access Flag)
//! - Bit 11: nG (not Global)
//! - Bits 53: PXN (Privileged Execute Never)
//! - Bit 54: UXN/XN (User Execute Never)
//! - Bits 55-58: Software use

#![allow(dead_code)]

use crate::arch_impl::traits::{PageFlags, PageTableOps};
use core::ops::BitOr;

// ARM64 descriptor bit definitions
const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_AF: u64 = 1 << 10;
const DESC_SH_INNER: u64 = 0b11 << 8;

// AP[2:1] bits at position 6-7
const DESC_AP_RW_EL1: u64 = 0b00 << 6;    // RW at EL1, no access EL0
const DESC_AP_RW_ALL: u64 = 0b01 << 6;    // RW at EL1/EL0
const DESC_AP_RO_EL1: u64 = 0b10 << 6;    // RO at EL1, no access EL0
const DESC_AP_RO_ALL: u64 = 0b11 << 6;    // RO at EL1/EL0

// Execute permissions
const DESC_PXN: u64 = 1 << 53;
const DESC_UXN: u64 = 1 << 54;

// Memory attributes (MAIR indices)
const DESC_ATTR_DEVICE: u64 = 0 << 2;
const DESC_ATTR_NORMAL: u64 = 1 << 2;

// Software-available bits for kernel use
const DESC_SW_COW: u64 = 1 << 55;

/// Our internal flag representation (matches arch_stub.rs for consistency)
const FLAG_PRESENT: u64 = 1 << 0;
const FLAG_WRITABLE: u64 = 1 << 1;
const FLAG_USER: u64 = 1 << 2;
const FLAG_NO_CACHE: u64 = 1 << 4;
const FLAG_COW: u64 = 1 << 9;
const FLAG_NO_EXECUTE: u64 = 1 << 63;

/// ARM64 page flags
///
/// These flags are stored in an internal representation that matches
/// the arch_stub PageTableFlags for consistency. They are converted
/// to ARM64 descriptor bits when writing to page table entries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Aarch64PageFlags(u64);

impl Aarch64PageFlags {
    /// Convert to ARM64 descriptor bits for page table entries
    pub fn to_descriptor_bits(self, is_page: bool) -> u64 {
        let mut desc: u64 = 0;

        if self.0 & FLAG_PRESENT != 0 {
            desc |= DESC_VALID;
            desc |= DESC_AF;           // Access Flag always set
            desc |= DESC_SH_INNER;     // Inner Shareable
        }

        // Page descriptors at L3 have bit 1 set (table bit)
        if is_page {
            desc |= DESC_TABLE;
        }

        // Memory attributes
        if self.0 & FLAG_NO_CACHE != 0 {
            desc |= DESC_ATTR_DEVICE;
        } else {
            desc |= DESC_ATTR_NORMAL;
        }

        // Access permissions
        let writable = self.0 & FLAG_WRITABLE != 0;
        let user = self.0 & FLAG_USER != 0;

        match (user, writable) {
            (false, true) => desc |= DESC_AP_RW_EL1,
            (false, false) => desc |= DESC_AP_RO_EL1,
            (true, true) => desc |= DESC_AP_RW_ALL,
            (true, false) => desc |= DESC_AP_RO_ALL,
        }

        // Execute permissions
        if self.0 & FLAG_NO_EXECUTE != 0 {
            desc |= DESC_PXN | DESC_UXN;
        } else if !user {
            desc |= DESC_UXN;  // Kernel-only: no user execute
        }

        // COW marker
        if self.0 & FLAG_COW != 0 {
            desc |= DESC_SW_COW;
        }

        desc
    }

    /// Create from ARM64 descriptor bits
    pub fn from_descriptor_bits(desc: u64) -> Self {
        let mut flags: u64 = 0;

        if desc & DESC_VALID != 0 {
            flags |= FLAG_PRESENT;
        }

        // Decode AP bits
        let ap = (desc >> 6) & 0b11;
        match ap {
            0b00 => flags |= FLAG_WRITABLE,
            0b01 => flags |= FLAG_WRITABLE | FLAG_USER,
            0b10 => {}
            0b11 => flags |= FLAG_USER,
            _ => {}
        }

        // Device memory
        if (desc >> 2) & 0x7 == 0 {
            flags |= FLAG_NO_CACHE;
        }

        // Execute permissions
        if desc & (DESC_PXN | DESC_UXN) == (DESC_PXN | DESC_UXN) {
            flags |= FLAG_NO_EXECUTE;
        }

        // COW marker
        if desc & DESC_SW_COW != 0 {
            flags |= FLAG_COW;
        }

        Self(flags)
    }
}

impl PageFlags for Aarch64PageFlags {
    fn empty() -> Self {
        Self(0)
    }

    fn present() -> Self {
        Self(FLAG_PRESENT)
    }

    fn writable() -> Self {
        Self(FLAG_WRITABLE)
    }

    fn user_accessible() -> Self {
        Self(FLAG_USER)
    }

    fn no_execute() -> Self {
        Self(FLAG_NO_EXECUTE)
    }

    fn cow_marker() -> Self {
        Self(FLAG_COW)
    }

    fn no_cache() -> Self {
        Self(FLAG_NO_CACHE)
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
