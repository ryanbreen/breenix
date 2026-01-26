//! ARM64 (AArch64) Memory Management Implementation
//!
//! This module provides real ARM64 memory management functionality including:
//! - TTBR0_EL1/TTBR1_EL1 (Translation Table Base Register) operations
//! - 4KB granule, 4-level page table support
//! - TLB (Translation Lookaside Buffer) management
//! - Page table mapping and unmapping
//! - Virtual-to-physical address translation
//!
//! ## ARM64 MMU Architecture
//!
//! ARM64 uses a split address space:
//! - TTBR0_EL1: User space (lower addresses, starting with 0x0000...)
//! - TTBR1_EL1: Kernel space (upper addresses, starting with 0xFFFF...)
//!
//! The page table format with 4KB granules:
//! - L0 (PGD): Bits 39-47, 512GB per entry
//! - L1 (PUD): Bits 30-38, 1GB per entry (can be block)
//! - L2 (PMD): Bits 21-29, 2MB per entry (can be block)
//! - L3 (PTE): Bits 12-20, 4KB per entry
//!
//! ## Descriptor Format (Stage 1)
//!
//! ```text
//! Bits 63-51: Upper attributes (UXN, PXN, Contiguous, etc.)
//! Bits 50-48: Reserved
//! Bits 47-12: Output address (aligned to 4KB)
//! Bits 11-2:  Lower attributes (SH, AP, NS, AttrIndx, etc.)
//! Bit 1:      Table/Block indicator (1=table at L0-L2)
//! Bit 0:      Valid bit
//! ```

use core::fmt;
use core::marker::PhantomData;
use core::ops::{Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, Sub, SubAssign};

/// Thread privilege level (maps to ARM64 exception levels)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPrivilege {
    /// Kernel thread (EL1 on ARM64)
    Kernel,
    /// User thread (EL0 on ARM64)
    User,
}

// =============================================================================
// Constants
// =============================================================================

const PAGE_SIZE: u64 = 4096;
#[allow(dead_code)] // Part of memory constants for future use
const PAGE_SHIFT: u64 = 12;
const ENTRIES_PER_TABLE: usize = 512;

/// Mask to extract physical address from descriptor (bits 47:12)
const DESC_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

// ARM64 descriptor bits
const DESC_VALID: u64 = 1 << 0;           // Valid bit
const DESC_TABLE: u64 = 1 << 1;           // Table descriptor (vs block)
const DESC_AF: u64 = 1 << 10;             // Access Flag
const DESC_SH_INNER: u64 = 0b11 << 8;     // Inner Shareable
const DESC_AP_RW_EL1: u64 = 0b00 << 6;    // AP[2:1] = RW at EL1, no access at EL0
const DESC_AP_RW_ALL: u64 = 0b01 << 6;    // AP[2:1] = RW at EL1/EL0
const DESC_AP_RO_EL1: u64 = 0b10 << 6;    // AP[2:1] = RO at EL1, no access at EL0
const DESC_AP_RO_ALL: u64 = 0b11 << 6;    // AP[2:1] = RO at EL1/EL0
const DESC_PXN: u64 = 1 << 53;            // Privileged Execute Never
const DESC_UXN: u64 = 1 << 54;            // User Execute Never (also called XN)

// Memory attribute indices (MAIR_EL1 configured during boot)
const DESC_ATTR_DEVICE: u64 = 0 << 2;     // Device-nGnRnE (index 0 in MAIR)
const DESC_ATTR_NORMAL: u64 = 1 << 2;     // Normal memory (index 1 in MAIR)

// OS-available bits for software use
const DESC_SW_BIT_55: u64 = 1 << 55;      // Software bit (used for COW marker)

// =============================================================================
// VirtAddr
// =============================================================================

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtAddr(u64);

impl VirtAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn zero() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline]
    pub fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    #[inline]
    pub fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Get L0 (PGD) index from virtual address (bits 39-47)
    #[inline]
    pub const fn l0_index(self) -> usize {
        ((self.0 >> 39) & 0x1FF) as usize
    }

    /// Get L1 (PUD) index from virtual address (bits 30-38)
    #[inline]
    pub const fn l1_index(self) -> usize {
        ((self.0 >> 30) & 0x1FF) as usize
    }

    /// Get L2 (PMD) index from virtual address (bits 21-29)
    #[inline]
    pub const fn l2_index(self) -> usize {
        ((self.0 >> 21) & 0x1FF) as usize
    }

    /// Get L3 (PTE) index from virtual address (bits 12-20)
    #[inline]
    pub const fn l3_index(self) -> usize {
        ((self.0 >> 12) & 0x1FF) as usize
    }

    /// Get page offset (bits 0-11)
    #[inline]
    pub const fn page_offset(self) -> u64 {
        self.0 & 0xFFF
    }

    /// Check if this is a kernel (TTBR1) address
    #[inline]
    pub const fn is_kernel_address(self) -> bool {
        // Kernel addresses have upper bits set (0xFFFF...)
        (self.0 >> 48) == 0xFFFF
    }

    /// Align down to page boundary
    #[inline]
    pub const fn align_down(self) -> Self {
        Self(self.0 & !0xFFF)
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VirtAddr({:#x})", self.0)
    }
}

impl fmt::LowerHex for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl Add<u64> for VirtAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0.wrapping_add(rhs))
    }
}

impl Sub<u64> for VirtAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0.wrapping_sub(rhs))
    }
}

impl AddAssign<u64> for VirtAddr {
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl SubAssign<u64> for VirtAddr {
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

// =============================================================================
// PhysAddr
// =============================================================================

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysAddr(u64);

impl PhysAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysAddr({:#x})", self.0)
    }
}

impl fmt::LowerHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl Add<u64> for PhysAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0.wrapping_add(rhs))
    }
}

impl Sub<u64> for PhysAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0.wrapping_sub(rhs))
    }
}

impl AddAssign<u64> for PhysAddr {
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl SubAssign<u64> for PhysAddr {
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

// =============================================================================
// Page Size Markers
// =============================================================================

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Size4KiB;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Size2MiB;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Size1GiB;

// =============================================================================
// PhysFrame
// =============================================================================

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PhysFrame<S = Size4KiB> {
    start: PhysAddr,
    _marker: PhantomData<S>,
}

impl<S> PhysFrame<S> {
    #[inline]
    pub const fn containing_address(addr: PhysAddr) -> Self {
        Self {
            start: PhysAddr::new(addr.as_u64() & !(PAGE_SIZE - 1)),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub const fn start_address(self) -> PhysAddr {
        self.start
    }
}

impl<S> fmt::Debug for PhysFrame<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysFrame({:#x})", self.start.as_u64())
    }
}

// =============================================================================
// Page
// =============================================================================

#[derive(Copy, Clone)]
pub struct Page<S = Size4KiB> {
    start: VirtAddr,
    _marker: PhantomData<S>,
}

// Manual Eq/PartialEq implementations that don't require S: Eq
impl<S> PartialEq for Page<S> {
    fn eq(&self, other: &Self) -> bool {
        self.start.as_u64() == other.start.as_u64()
    }
}

impl<S> Eq for Page<S> {}

impl<S> Page<S> {
    #[inline]
    pub const fn containing_address(addr: VirtAddr) -> Self {
        Self {
            start: VirtAddr::new(addr.as_u64() & !(PAGE_SIZE - 1)),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub const fn start_address(self) -> VirtAddr {
        self.start
    }

    #[inline]
    pub fn range_inclusive(start: Self, end: Self) -> PageRangeInclusive<S> {
        PageRangeInclusive {
            current: start.start.as_u64(),
            end: end.start.as_u64(),
            _marker: PhantomData,
        }
    }
}

impl<S> fmt::Debug for Page<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Page({:#x})", self.start.as_u64())
    }
}

impl<S> PartialOrd for Page<S> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<S> Ord for Page<S> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.start.as_u64().cmp(&other.start.as_u64())
    }
}

impl<S> core::ops::Add<u64> for Page<S> {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self {
            start: VirtAddr::new(self.start.as_u64() + rhs * PAGE_SIZE),
            _marker: PhantomData,
        }
    }
}

impl<S> core::ops::AddAssign<u64> for Page<S> {
    fn add_assign(&mut self, rhs: u64) {
        self.start = VirtAddr::new(self.start.as_u64() + rhs * PAGE_SIZE);
    }
}

pub struct PageRangeInclusive<S = Size4KiB> {
    current: u64,
    end: u64,
    _marker: PhantomData<S>,
}

impl<S> Iterator for PageRangeInclusive<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current > self.end {
            return None;
        }
        let addr = self.current;
        self.current = self.current.saturating_add(PAGE_SIZE);
        Some(Page {
            start: VirtAddr::new(addr),
            _marker: PhantomData,
        })
    }
}

// =============================================================================
// PageTableFlags - ARM64 specific
// =============================================================================

/// ARM64 page table entry flags
///
/// These flags map to the ARM64 descriptor format. The bitfield is designed
/// to match x86_64 semantics where possible for compatibility with shared code.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PageTableFlags(u64);

impl PageTableFlags {
    /// Page is present (valid)
    pub const PRESENT: Self = Self(DESC_VALID);

    /// Page is writable (RW at EL1, RW at EL0 if USER_ACCESSIBLE)
    /// On ARM64, this is represented by AP[2:1] bits
    pub const WRITABLE: Self = Self(1 << 1); // Stored in our flag, translated during mapping

    /// Page is accessible from EL0 (userspace)
    pub const USER_ACCESSIBLE: Self = Self(1 << 2); // Stored, translated to AP bits

    /// Write-through caching
    pub const WRITE_THROUGH: Self = Self(1 << 3);

    /// Disable caching (for MMIO)
    pub const NO_CACHE: Self = Self(1 << 4);

    /// Huge page (2MB at L2, 1GB at L1)
    pub const HUGE_PAGE: Self = Self(1 << 7);

    /// Global page (not flushed on ASID change)
    pub const GLOBAL: Self = Self(1 << 8);

    /// OS-available bit 9 (used for COW marker)
    pub const BIT_9: Self = Self(1 << 9);

    /// No execute (XN/UXN on ARM64)
    pub const NO_EXECUTE: Self = Self(1 << 63);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline]
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Convert our generic flags to ARM64 descriptor bits
    fn to_arm64_descriptor(self, is_table: bool) -> u64 {
        let mut desc: u64 = 0;

        // Valid bit
        if self.contains(Self::PRESENT) {
            desc |= DESC_VALID;
        }

        // Table vs block descriptor
        if is_table {
            desc |= DESC_TABLE;
        }

        // Access flag - always set for valid entries
        desc |= DESC_AF;

        // Shareability - inner shareable for normal memory
        desc |= DESC_SH_INNER;

        // Memory attributes
        if self.contains(Self::NO_CACHE) {
            desc |= DESC_ATTR_DEVICE;
        } else {
            desc |= DESC_ATTR_NORMAL;
        }

        // Access permissions based on writable and user_accessible
        let writable = self.contains(Self::WRITABLE);
        let user = self.contains(Self::USER_ACCESSIBLE);

        match (user, writable) {
            (false, true) => desc |= DESC_AP_RW_EL1,   // Kernel RW, no user access
            (false, false) => desc |= DESC_AP_RO_EL1,  // Kernel RO, no user access
            (true, true) => desc |= DESC_AP_RW_ALL,    // Both RW
            (true, false) => desc |= DESC_AP_RO_ALL,   // Both RO
        }

        // Execute permissions
        if self.contains(Self::NO_EXECUTE) {
            desc |= DESC_PXN | DESC_UXN;  // Neither kernel nor user can execute
        } else if !user {
            // Kernel-only page: disable user execute
            desc |= DESC_UXN;
        }

        // COW marker in software-available bit
        if self.contains(Self::BIT_9) {
            desc |= DESC_SW_BIT_55;
        }

        desc
    }
}

impl fmt::Debug for PageTableFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageTableFlags({:#x})", self.0)
    }
}

impl BitOr for PageTableFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for PageTableFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for PageTableFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for PageTableFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

// =============================================================================
// PageTableEntry
// =============================================================================

/// ARM64 page table entry
///
/// Represents a single entry in an ARM64 page table. The entry format depends
/// on the level:
/// - L0-L2: Can be table descriptors (point to next level) or block descriptors
/// - L3: Always page descriptors (point to 4KB pages)
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PageTableEntry {
    entry: u64,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        Self { entry: 0 }
    }

    /// Get our generic flags from the raw entry
    #[inline]
    pub fn flags(&self) -> PageTableFlags {
        let mut flags = PageTableFlags::empty();

        if self.entry & DESC_VALID != 0 {
            flags.insert(PageTableFlags::PRESENT);
        }

        // Decode AP bits for writable/user
        let ap = (self.entry >> 6) & 0b11;
        match ap {
            0b00 => flags.insert(PageTableFlags::WRITABLE), // RW EL1 only
            0b01 => {
                flags.insert(PageTableFlags::WRITABLE);
                flags.insert(PageTableFlags::USER_ACCESSIBLE);
            }
            0b10 => {} // RO EL1 only
            0b11 => flags.insert(PageTableFlags::USER_ACCESSIBLE), // RO all
            _ => {}
        }

        // Check for device memory (no cache)
        if (self.entry >> 2) & 0x7 == 0 {
            flags.insert(PageTableFlags::NO_CACHE);
        }

        // Check execute permissions
        if self.entry & (DESC_PXN | DESC_UXN) == (DESC_PXN | DESC_UXN) {
            flags.insert(PageTableFlags::NO_EXECUTE);
        }

        // Check huge page
        if self.entry & DESC_TABLE == 0 && self.entry & DESC_VALID != 0 {
            // Valid but not a table = block descriptor
            flags.insert(PageTableFlags::HUGE_PAGE);
        }

        // COW marker
        if self.entry & DESC_SW_BIT_55 != 0 {
            flags.insert(PageTableFlags::BIT_9);
        }

        flags
    }

    /// Get the physical address from this entry
    #[inline]
    pub fn addr(&self) -> PhysAddr {
        PhysAddr::new(self.entry & DESC_ADDR_MASK)
    }

    #[inline]
    pub fn is_unused(&self) -> bool {
        self.entry == 0
    }

    #[inline]
    pub fn set_unused(&mut self) {
        self.entry = 0;
    }

    /// Check if this is a valid table descriptor (points to next level)
    #[inline]
    pub fn is_table(&self) -> bool {
        (self.entry & (DESC_VALID | DESC_TABLE)) == (DESC_VALID | DESC_TABLE)
    }

    /// Check if this is a valid block/page descriptor
    #[inline]
    pub fn is_block(&self) -> bool {
        (self.entry & DESC_VALID) != 0 && (self.entry & DESC_TABLE) == 0
    }

    #[inline]
    pub fn set_frame<S>(&mut self, frame: PhysFrame<S>, flags: PageTableFlags) {
        // For 4KB pages at L3, we always use page descriptors (table bit set)
        let desc = flags.to_arm64_descriptor(true);
        self.entry = (frame.start_address().as_u64() & DESC_ADDR_MASK) | desc;
    }

    #[inline]
    pub fn set_addr(&mut self, addr: PhysAddr, flags: PageTableFlags) {
        let desc = flags.to_arm64_descriptor(true);
        self.entry = (addr.as_u64() & DESC_ADDR_MASK) | desc;
    }

    /// Set as a table descriptor pointing to next-level page table
    #[inline]
    pub fn set_table(&mut self, addr: PhysAddr) {
        // Table descriptors have minimal attributes - just valid and table bits
        self.entry = (addr.as_u64() & DESC_ADDR_MASK) | DESC_VALID | DESC_TABLE;
    }

    /// Get the frame mapped by this entry
    #[inline]
    pub fn frame(&self) -> Result<PhysFrame<Size4KiB>, FrameError> {
        if self.is_unused() {
            Err(FrameError::FrameNotPresent)
        } else if self.is_block() && !self.flags().contains(PageTableFlags::HUGE_PAGE) {
            // It's a block descriptor but we're treating it as 4KB
            Ok(PhysFrame::containing_address(self.addr()))
        } else {
            Ok(PhysFrame {
                start: self.addr(),
                _marker: PhantomData,
            })
        }
    }

    /// Get raw entry value (for debugging)
    #[inline]
    pub fn raw(&self) -> u64 {
        self.entry
    }
}

/// Error type for frame operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The entry is not present
    FrameNotPresent,
    /// The entry has the huge page flag set
    HugeFrame,
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PageTableEntry {{ entry: {:#x}, addr: {:#x}, flags: {:?} }}",
            self.entry,
            self.addr().as_u64(),
            self.flags()
        )
    }
}

// =============================================================================
// PageTable
// =============================================================================

/// ARM64 page table (512 entries, 4KB aligned)
#[repr(C, align(4096))]
#[derive(Clone)]
pub struct PageTable {
    entries: [PageTableEntry; ENTRIES_PER_TABLE],
}

impl Default for PageTable {
    fn default() -> Self {
        Self {
            entries: [PageTableEntry::new(); ENTRIES_PER_TABLE],
        }
    }
}

impl core::ops::Index<usize> for PageTable {
    type Output = PageTableEntry;

    fn index(&self, index: usize) -> &Self::Output {
        &self.entries[index]
    }
}

impl core::ops::IndexMut<usize> for PageTable {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.entries[index]
    }
}

// =============================================================================
// FrameAllocator trait
// =============================================================================

pub unsafe trait FrameAllocator<S = Size4KiB> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>>;
}

// =============================================================================
// Cr3 (TTBR equivalent)
// =============================================================================

/// ARM64 Translation Table Base Register operations
///
/// Provides an x86_64-compatible interface for TTBR0_EL1 operations.
/// Note: ARM64 has separate TTBR0 (user) and TTBR1 (kernel) registers.
/// This implementation focuses on TTBR0 for user-space page tables.
pub struct Cr3;

/// TTBR flags (ASID and other control bits)
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Cr3Flags(u64);

impl Cr3Flags {
    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Create with ASID (Address Space Identifier)
    #[inline]
    pub const fn with_asid(asid: u16) -> Self {
        Self((asid as u64) << 48)
    }
}

impl Cr3 {
    /// Read the current TTBR0_EL1 value
    ///
    /// Returns the physical frame containing the L0 page table and flags (ASID).
    #[cfg(target_arch = "aarch64")]
    pub fn read() -> (PhysFrame<Size4KiB>, Cr3Flags) {
        let ttbr: u64;
        unsafe {
            core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr, options(nomem, nostack));
        }
        let addr = ttbr & DESC_ADDR_MASK;
        let flags = Cr3Flags((ttbr >> 48) << 48); // Extract ASID
        (PhysFrame::containing_address(PhysAddr::new(addr)), flags)
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    pub fn read() -> (PhysFrame<Size4KiB>, Cr3Flags) {
        (
            PhysFrame::containing_address(PhysAddr::new(0)),
            Cr3Flags::empty(),
        )
    }

    /// Write to TTBR0_EL1
    ///
    /// Updates the user-space page table root. Includes necessary barriers.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - The frame contains a valid page table hierarchy
    /// - All mappings in the page table are valid
    #[cfg(target_arch = "aarch64")]
    pub unsafe fn write(frame: PhysFrame<Size4KiB>, flags: Cr3Flags) {
        let addr = frame.start_address().as_u64();
        let asid = flags.0 & 0xFFFF_0000_0000_0000;
        let ttbr = addr | asid;

        core::arch::asm!(
            "dsb ishst",           // Ensure all stores are visible
            "msr ttbr0_el1, {0}",  // Write new page table root
            "dsb ish",             // Ensure write completes
            "isb",                 // Synchronize instruction stream
            in(reg) ttbr,
            options(nostack)
        );
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    pub unsafe fn write(_frame: PhysFrame<Size4KiB>, _flags: Cr3Flags) {}

    /// Read TTBR1_EL1 (kernel page table)
    #[cfg(target_arch = "aarch64")]
    pub fn read_kernel() -> (PhysFrame<Size4KiB>, Cr3Flags) {
        let ttbr: u64;
        unsafe {
            core::arch::asm!("mrs {}, ttbr1_el1", out(reg) ttbr, options(nomem, nostack));
        }
        let addr = ttbr & DESC_ADDR_MASK;
        let flags = Cr3Flags((ttbr >> 48) << 48);
        (PhysFrame::containing_address(PhysAddr::new(addr)), flags)
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    pub fn read_kernel() -> (PhysFrame<Size4KiB>, Cr3Flags) {
        (
            PhysFrame::containing_address(PhysAddr::new(0)),
            Cr3Flags::empty(),
        )
    }

    /// Write to TTBR1_EL1 (kernel page table)
    ///
    /// # Safety
    /// The caller must ensure the frame contains a valid page table hierarchy.
    #[cfg(target_arch = "aarch64")]
    pub unsafe fn write_kernel(frame: PhysFrame<Size4KiB>, flags: Cr3Flags) {
        let addr = frame.start_address().as_u64();
        let asid = flags.0 & 0xFFFF_0000_0000_0000;
        let ttbr = addr | asid;

        core::arch::asm!(
            "dsb ishst",
            "msr ttbr1_el1, {0}",
            "dsb ish",
            "isb",
            in(reg) ttbr,
            options(nostack)
        );
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    pub unsafe fn write_kernel(_frame: PhysFrame<Size4KiB>, _flags: Cr3Flags) {}
}

// =============================================================================
// TLB Operations
// =============================================================================

pub mod tlb {
    use super::VirtAddr;

    /// Flush a single page from the TLB
    ///
    /// Uses TLBI VAE1IS instruction to invalidate by virtual address
    /// in the Inner Shareable domain.
    #[cfg(target_arch = "aarch64")]
    #[inline]
    pub fn flush(addr: VirtAddr) {
        // The TLBI VAE1IS instruction expects the address shifted right by 12
        let page = addr.as_u64() >> 12;
        unsafe {
            core::arch::asm!(
                "dsb ishst",           // Ensure stores complete
                "tlbi vae1is, {0}",    // Invalidate by VA, EL1, Inner Shareable
                "dsb ish",             // Ensure TLB invalidation completes
                "isb",                 // Sync instruction stream
                in(reg) page,
                options(nostack)
            );
        }
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    #[inline]
    pub fn flush(_addr: VirtAddr) {}

    /// Flush the entire TLB
    ///
    /// Uses TLBI VMALLE1IS to invalidate all entries in the
    /// Inner Shareable domain.
    #[cfg(target_arch = "aarch64")]
    #[inline]
    pub fn flush_all() {
        unsafe {
            core::arch::asm!(
                "dsb ishst",           // Ensure stores complete
                "tlbi vmalle1is",      // Invalidate all EL1 entries
                "dsb ish",             // Ensure invalidation completes
                "isb",                 // Sync instruction stream
                options(nostack)
            );
        }
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    #[inline]
    pub fn flush_all() {}

    /// Flush TLB entries for a specific ASID
    #[cfg(target_arch = "aarch64")]
    #[inline]
    pub fn flush_asid(asid: u16) {
        let asid_shifted = (asid as u64) << 48;
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "tlbi aside1is, {0}",  // Invalidate by ASID
                "dsb ish",
                "isb",
                in(reg) asid_shifted,
                options(nostack)
            );
        }
    }

    /// Stub for non-ARM64 builds
    #[cfg(not(target_arch = "aarch64"))]
    #[inline]
    pub fn flush_asid(_asid: u16) {}
}

// =============================================================================
// MapperFlush
// =============================================================================

pub struct MapperFlush<S = Size4KiB> {
    page: Page<S>,
}

impl<S> MapperFlush<S> {
    fn new(page: Page<S>) -> Self {
        Self { page }
    }

    #[inline]
    pub fn flush(self) {
        tlb::flush(self.page.start_address());
    }

    #[inline]
    pub fn ignore(self) {
        // Intentionally do nothing
    }
}

// =============================================================================
// Error Types
// =============================================================================

pub struct UnmapError;
pub struct TranslateError;

pub mod mapper {
    use super::{PageTableFlags, PhysFrame, Size4KiB};

    #[derive(Debug)]
    pub enum MapToError {
        FrameAllocationFailed,
        ParentEntryHugePage,
        PageAlreadyMapped(PhysFrame<Size4KiB>),
    }

    #[derive(Debug)]
    pub enum TranslateResult {
        Mapped {
            frame: PhysFrame<Size4KiB>,
            offset: u64,
            flags: PageTableFlags,
        },
        NotMapped,
    }
}

// =============================================================================
// Mapper and Translate traits
// =============================================================================

pub trait Mapper<S = Size4KiB> {
    unsafe fn map_to<A>(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: &mut A,
    ) -> Result<MapperFlush<S>, mapper::MapToError>
    where
        A: FrameAllocator<S>;

    unsafe fn map_to_with_table_flags<A>(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        table_flags: PageTableFlags,
        frame_allocator: &mut A,
    ) -> Result<MapperFlush<S>, mapper::MapToError>
    where
        A: FrameAllocator<S>;

    fn unmap(&mut self, page: Page<S>) -> Result<(PhysFrame<S>, MapperFlush<S>), UnmapError>;

    fn translate_page(&self, page: Page<S>) -> Result<PhysFrame<S>, TranslateError>;
}

pub trait Translate {
    fn translate(&self, addr: VirtAddr) -> mapper::TranslateResult;
    fn translate_addr(&self, addr: VirtAddr) -> Option<PhysAddr>;
}

// =============================================================================
// OffsetPageTable - Real Implementation
// =============================================================================

/// Helper function to get or create a page table entry
///
/// Returns the physical address of the next-level table.
unsafe fn get_or_create_table_inner<A>(
    entry: &mut PageTableEntry,
    phys_offset: VirtAddr,
    allocator: &mut A,
) -> Result<PhysAddr, mapper::MapToError>
where
    A: FrameAllocator<Size4KiB>,
{
    if entry.is_table() {
        // Entry already points to a table
        Ok(entry.addr())
    } else if entry.is_unused() {
        // Need to allocate a new table
        let frame = allocator
            .allocate_frame()
            .ok_or(mapper::MapToError::FrameAllocationFailed)?;

        // Zero the new table
        let table_virt = VirtAddr::new(frame.start_address().as_u64() + phys_offset.as_u64());
        let table = &mut *(table_virt.as_mut_ptr() as *mut PageTable);
        for i in 0..ENTRIES_PER_TABLE {
            table[i].set_unused();
        }

        // Set the entry to point to the new table
        entry.set_table(frame.start_address());

        Ok(frame.start_address())
    } else {
        // Entry is a block descriptor - can't traverse
        Err(mapper::MapToError::ParentEntryHugePage)
    }
}

/// ARM64 page table mapper using direct physical memory mapping
///
/// This provides the same interface as x86_64's OffsetPageTable but implements
/// ARM64-specific page table walking and manipulation.
pub struct OffsetPageTable<'a> {
    /// Pointer to the L0 (PGD) page table
    l0_table: &'a mut PageTable,
    /// Virtual address where physical memory is mapped (offset mapping)
    phys_offset: VirtAddr,
}

impl<'a> OffsetPageTable<'a> {
    /// Create a new OffsetPageTable
    ///
    /// # Safety
    /// - `level_4_table` must point to a valid L0 page table
    /// - Physical memory must be mapped at `offset`
    pub unsafe fn new(level_4_table: &'a mut PageTable, offset: VirtAddr) -> Self {
        Self {
            l0_table: level_4_table,
            phys_offset: offset,
        }
    }

    /// Get a mutable reference to the L0 table
    pub fn level_4_table(&mut self) -> &mut PageTable {
        self.l0_table
    }

    /// Convert physical address to virtual using the offset mapping
    #[inline]
    #[allow(dead_code)] // Part of OffsetPageTable API
    fn phys_to_virt(&self, phys: PhysAddr) -> VirtAddr {
        VirtAddr::new(phys.as_u64() + self.phys_offset.as_u64())
    }
}

impl<'a> Mapper<Size4KiB> for OffsetPageTable<'a> {
    unsafe fn map_to<A>(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        frame_allocator: &mut A,
    ) -> Result<MapperFlush<Size4KiB>, mapper::MapToError>
    where
        A: FrameAllocator<Size4KiB>,
    {
        self.map_to_with_table_flags(
            page,
            frame,
            flags,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            frame_allocator,
        )
    }

    unsafe fn map_to_with_table_flags<A>(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        _table_flags: PageTableFlags,
        frame_allocator: &mut A,
    ) -> Result<MapperFlush<Size4KiB>, mapper::MapToError>
    where
        A: FrameAllocator<Size4KiB>,
    {
        let virt = page.start_address();
        let phys_offset = self.phys_offset;

        // Get indices for each level
        let l0_idx = virt.l0_index();
        let l1_idx = virt.l1_index();
        let l2_idx = virt.l2_index();
        let l3_idx = virt.l3_index();

        // Walk/create the page table hierarchy
        // L0 -> L1
        let l1_phys = get_or_create_table_inner(
            &mut self.l0_table[l0_idx],
            phys_offset,
            frame_allocator,
        )?;

        // L1 -> L2
        let l1_table = &mut *(VirtAddr::new(l1_phys.as_u64() + phys_offset.as_u64()).as_mut_ptr() as *mut PageTable);
        let l2_phys = get_or_create_table_inner(
            &mut l1_table[l1_idx],
            phys_offset,
            frame_allocator,
        )?;

        // L2 -> L3
        let l2_table = &mut *(VirtAddr::new(l2_phys.as_u64() + phys_offset.as_u64()).as_mut_ptr() as *mut PageTable);
        let l3_phys = get_or_create_table_inner(
            &mut l2_table[l2_idx],
            phys_offset,
            frame_allocator,
        )?;

        // Map the page in L3
        let l3_table = &mut *(VirtAddr::new(l3_phys.as_u64() + phys_offset.as_u64()).as_mut_ptr() as *mut PageTable);
        let entry = &mut l3_table[l3_idx];

        // Check if page is already mapped
        if !entry.is_unused() {
            return Err(mapper::MapToError::PageAlreadyMapped(
                PhysFrame::containing_address(entry.addr()),
            ));
        }

        // Map the page
        entry.set_frame(frame, flags);

        Ok(MapperFlush::new(page))
    }

    fn unmap(&mut self, page: Page<Size4KiB>) -> Result<(PhysFrame<Size4KiB>, MapperFlush<Size4KiB>), UnmapError> {
        let virt = page.start_address();
        let phys_offset = self.phys_offset;

        let l0_idx = virt.l0_index();
        let l1_idx = virt.l1_index();
        let l2_idx = virt.l2_index();
        let l3_idx = virt.l3_index();

        // Walk the page tables
        let l0_entry = &self.l0_table[l0_idx];
        if !l0_entry.is_table() {
            return Err(UnmapError);
        }

        let l1_table = unsafe {
            let addr = VirtAddr::new(l0_entry.addr().as_u64() + phys_offset.as_u64());
            &mut *(addr.as_mut_ptr() as *mut PageTable)
        };

        let l1_entry = &l1_table[l1_idx];
        if !l1_entry.is_table() {
            return Err(UnmapError);
        }

        let l2_table = unsafe {
            let addr = VirtAddr::new(l1_entry.addr().as_u64() + phys_offset.as_u64());
            &mut *(addr.as_mut_ptr() as *mut PageTable)
        };

        let l2_entry = &l2_table[l2_idx];
        if !l2_entry.is_table() {
            return Err(UnmapError);
        }

        let l3_table = unsafe {
            let addr = VirtAddr::new(l2_entry.addr().as_u64() + phys_offset.as_u64());
            &mut *(addr.as_mut_ptr() as *mut PageTable)
        };

        let l3_entry = &mut l3_table[l3_idx];
        if l3_entry.is_unused() {
            return Err(UnmapError);
        }

        let frame = PhysFrame::containing_address(l3_entry.addr());
        l3_entry.set_unused();

        Ok((frame, MapperFlush::new(page)))
    }

    fn translate_page(&self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, TranslateError> {
        let virt = page.start_address();
        let phys_offset = self.phys_offset;

        let l0_idx = virt.l0_index();
        let l1_idx = virt.l1_index();
        let l2_idx = virt.l2_index();
        let l3_idx = virt.l3_index();

        // Walk the page tables
        let l0_entry = &self.l0_table[l0_idx];
        if !l0_entry.is_table() {
            return Err(TranslateError);
        }

        let l1_table = unsafe {
            let addr = VirtAddr::new(l0_entry.addr().as_u64() + phys_offset.as_u64());
            &*(addr.as_ptr() as *const PageTable)
        };

        let l1_entry = &l1_table[l1_idx];
        if l1_entry.is_block() {
            // 1GB block mapping
            let base = l1_entry.addr().as_u64() & !0x3FFFFFFF;
            return Ok(PhysFrame::containing_address(PhysAddr::new(
                base + (virt.as_u64() & 0x3FFFFFFF),
            )));
        }
        if !l1_entry.is_table() {
            return Err(TranslateError);
        }

        let l2_table = unsafe {
            let addr = VirtAddr::new(l1_entry.addr().as_u64() + phys_offset.as_u64());
            &*(addr.as_ptr() as *const PageTable)
        };

        let l2_entry = &l2_table[l2_idx];
        if l2_entry.is_block() {
            // 2MB block mapping
            let base = l2_entry.addr().as_u64() & !0x1FFFFF;
            return Ok(PhysFrame::containing_address(PhysAddr::new(
                base + (virt.as_u64() & 0x1FFFFF),
            )));
        }
        if !l2_entry.is_table() {
            return Err(TranslateError);
        }

        let l3_table = unsafe {
            let addr = VirtAddr::new(l2_entry.addr().as_u64() + phys_offset.as_u64());
            &*(addr.as_ptr() as *const PageTable)
        };

        let l3_entry = &l3_table[l3_idx];
        if l3_entry.is_unused() {
            return Err(TranslateError);
        }

        Ok(PhysFrame::containing_address(l3_entry.addr()))
    }
}

impl<'a> Translate for OffsetPageTable<'a> {
    fn translate(&self, addr: VirtAddr) -> mapper::TranslateResult {
        let page = Page::<Size4KiB>::containing_address(addr);
        let phys_offset = self.phys_offset;

        match self.translate_page(page) {
            Ok(frame) => {
                // Walk again to get flags
                let virt = addr;
                let l0_idx = virt.l0_index();
                let l1_idx = virt.l1_index();
                let l2_idx = virt.l2_index();
                let l3_idx = virt.l3_index();

                // Get the L3 entry to read flags
                let l0_entry = &self.l0_table[l0_idx];
                let l1_table = unsafe {
                    let a = VirtAddr::new(l0_entry.addr().as_u64() + phys_offset.as_u64());
                    &*(a.as_ptr() as *const PageTable)
                };
                let l1_entry = &l1_table[l1_idx];
                let l2_table = unsafe {
                    let a = VirtAddr::new(l1_entry.addr().as_u64() + phys_offset.as_u64());
                    &*(a.as_ptr() as *const PageTable)
                };
                let l2_entry = &l2_table[l2_idx];
                let l3_table = unsafe {
                    let a = VirtAddr::new(l2_entry.addr().as_u64() + phys_offset.as_u64());
                    &*(a.as_ptr() as *const PageTable)
                };
                let l3_entry = &l3_table[l3_idx];

                mapper::TranslateResult::Mapped {
                    frame,
                    offset: addr.page_offset(),
                    flags: l3_entry.flags(),
                }
            }
            Err(_) => mapper::TranslateResult::NotMapped,
        }
    }

    fn translate_addr(&self, addr: VirtAddr) -> Option<PhysAddr> {
        match self.translate(addr) {
            mapper::TranslateResult::Mapped { frame, offset, .. } => {
                Some(PhysAddr::new(frame.start_address().as_u64() + offset))
            }
            mapper::TranslateResult::NotMapped => None,
        }
    }
}
