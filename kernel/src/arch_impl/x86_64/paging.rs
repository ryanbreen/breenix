//! x86_64 page table operations.
//!
//! Implements the PageTableOps and PageFlags traits for x86_64's 4-level
//! page table hierarchy.
//!
//! Note: This is part of the complete HAL API. Helper functions like
//! index extractors are defined for API completeness.

#![allow(dead_code)] // HAL module - complete API for x86_64 paging

use crate::arch_impl::traits::{PageFlags, PageTableOps};
use core::ops::BitOr;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::PageTableFlags;

/// x86_64 page flags wrapper.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct X86PageFlags(PageTableFlags);

impl X86PageFlags {
    /// Create from raw x86_64 crate flags.
    #[inline(always)]
    pub const fn from_raw(flags: PageTableFlags) -> Self {
        Self(flags)
    }

    /// Get the underlying x86_64 crate flags.
    #[inline(always)]
    pub const fn as_raw(&self) -> PageTableFlags {
        self.0
    }
}

impl PageFlags for X86PageFlags {
    #[inline(always)]
    fn empty() -> Self {
        Self(PageTableFlags::empty())
    }

    #[inline(always)]
    fn present() -> Self {
        Self(PageTableFlags::PRESENT)
    }

    #[inline(always)]
    fn writable() -> Self {
        Self(PageTableFlags::WRITABLE)
    }

    #[inline(always)]
    fn user_accessible() -> Self {
        Self(PageTableFlags::USER_ACCESSIBLE)
    }

    #[inline(always)]
    fn no_execute() -> Self {
        Self(PageTableFlags::NO_EXECUTE)
    }

    #[inline(always)]
    fn cow_marker() -> Self {
        // Use BIT_9 (OS-available) for Copy-on-Write marking
        Self(PageTableFlags::BIT_9)
    }

    #[inline(always)]
    fn no_cache() -> Self {
        Self(PageTableFlags::NO_CACHE)
    }

    #[inline(always)]
    fn contains(&self, other: Self) -> bool {
        self.0.contains(other.0)
    }
}

impl BitOr for X86PageFlags {
    type Output = Self;

    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// x86_64 page table operations implementation.
pub struct X86PageTableOps;

impl PageTableOps for X86PageTableOps {
    type Flags = X86PageFlags;

    const PAGE_LEVELS: usize = 4;
    const PAGE_SIZE: usize = 4096;
    const ENTRIES_PER_TABLE: usize = 512;

    #[inline(always)]
    fn read_root() -> u64 {
        let (frame, _flags) = Cr3::read();
        frame.start_address().as_u64()
    }

    #[inline(always)]
    unsafe fn write_root(addr: u64) {
        use x86_64::registers::control::Cr3Flags;
        use x86_64::structures::paging::PhysFrame;
        use x86_64::PhysAddr;

        let frame = PhysFrame::containing_address(PhysAddr::new(addr));
        Cr3::write(frame, Cr3Flags::empty());
    }

    #[inline(always)]
    fn flush_tlb_page(addr: u64) {
        use x86_64::VirtAddr;
        x86_64::instructions::tlb::flush(VirtAddr::new(addr));
    }

    #[inline(always)]
    fn flush_tlb_all() {
        x86_64::instructions::tlb::flush_all();
    }
}

// Additional x86-specific page table helpers that don't fit in the trait

/// Extract PML4 index from virtual address.
#[inline(always)]
pub fn pml4_index(addr: u64) -> usize {
    ((addr >> 39) & 0x1FF) as usize
}

/// Extract PDPT index from virtual address.
#[inline(always)]
pub fn pdpt_index(addr: u64) -> usize {
    ((addr >> 30) & 0x1FF) as usize
}

/// Extract PD index from virtual address.
#[inline(always)]
pub fn pd_index(addr: u64) -> usize {
    ((addr >> 21) & 0x1FF) as usize
}

/// Extract PT index from virtual address.
#[inline(always)]
pub fn pt_index(addr: u64) -> usize {
    ((addr >> 12) & 0x1FF) as usize
}

/// Check if an address is in kernel space (upper half).
#[inline(always)]
pub fn is_kernel_address(addr: u64) -> bool {
    // Kernel addresses have bit 47 set (sign-extended to bits 48-63)
    addr >= 0xFFFF_8000_0000_0000
}

/// Check if an address is in user space (lower half).
#[inline(always)]
pub fn is_user_address(addr: u64) -> bool {
    addr < 0x0000_8000_0000_0000
}

// CR4 control register operations

/// Enable global pages support (CR4.PGE).
///
/// This allows the CPU to keep kernel pages in the TLB across CR3 changes,
/// significantly improving performance during context switches.
///
/// # Safety
/// Should be called after kernel page tables are set up but before userspace processes start.
pub unsafe fn enable_global_pages() {
    use x86_64::registers::control::{Cr4, Cr4Flags};

    let mut cr4 = Cr4::read();
    if !cr4.contains(Cr4Flags::PAGE_GLOBAL) {
        cr4 |= Cr4Flags::PAGE_GLOBAL;
        Cr4::write(cr4);
    }
}

/// Check if global pages are enabled.
pub fn global_pages_enabled() -> bool {
    use x86_64::registers::control::{Cr4, Cr4Flags};
    Cr4::read().contains(Cr4Flags::PAGE_GLOBAL)
}

/// Read CR2 (page fault linear address).
///
/// Returns the faulting address from the last page fault.
/// Returns None if reading fails (shouldn't happen on valid x86_64).
#[inline(always)]
pub fn read_page_fault_address() -> Option<u64> {
    use x86_64::registers::control::Cr2;
    Cr2::read().ok().map(|addr| addr.as_u64())
}
