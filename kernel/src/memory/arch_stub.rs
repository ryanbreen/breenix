//! Minimal stubs for x86_64 paging types when building on non-x86_64 targets.
//!
//! These are intentionally lightweight and only support the APIs used by the
//! memory subsystem. Functionality is stubbed; the goal is compilation.

use core::fmt;
use core::marker::PhantomData;
use core::ops::{Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, Sub, SubAssign};

const PAGE_SIZE: u64 = 4096;

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

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Size4KiB;

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

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Page<S = Size4KiB> {
    start: VirtAddr,
    _marker: PhantomData<S>,
}

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

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PageTableFlags(u64);

impl PageTableFlags {
    pub const PRESENT: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER_ACCESSIBLE: Self = Self(1 << 2);
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    pub const NO_CACHE: Self = Self(1 << 4);
    pub const HUGE_PAGE: Self = Self(1 << 7);
    pub const GLOBAL: Self = Self(1 << 8);
    pub const BIT_9: Self = Self(1 << 9);
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

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PageTableEntry {
    addr: PhysAddr,
    flags: PageTableFlags,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        Self {
            addr: PhysAddr::new(0),
            flags: PageTableFlags::empty(),
        }
    }

    #[inline]
    pub fn flags(&self) -> PageTableFlags {
        self.flags
    }

    #[inline]
    pub fn addr(&self) -> PhysAddr {
        self.addr
    }

    #[inline]
    pub fn is_unused(&self) -> bool {
        self.flags.bits() == 0
    }

    #[inline]
    pub fn set_unused(&mut self) {
        self.addr = PhysAddr::new(0);
        self.flags = PageTableFlags::empty();
    }

    #[inline]
    pub fn set_frame<S>(&mut self, frame: PhysFrame<S>, flags: PageTableFlags) {
        self.addr = frame.start_address();
        self.flags = flags;
    }

    #[inline]
    pub fn set_addr(&mut self, addr: PhysAddr, flags: PageTableFlags) {
        self.addr = addr;
        self.flags = flags;
    }

    #[inline]
    pub fn frame<S>(&self) -> Option<PhysFrame<S>> {
        if self.is_unused() {
            None
        } else {
            Some(PhysFrame {
                start: self.addr,
                _marker: PhantomData,
            })
        }
    }
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PageTableEntry {{ addr: {:#x}, flags: {:?} }}",
            self.addr.as_u64(),
            self.flags
        )
    }
}

#[derive(Clone)]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl Default for PageTable {
    fn default() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
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

pub unsafe trait FrameAllocator<S = Size4KiB> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>>;
}

pub struct OffsetPageTable<'a> {
    _marker: PhantomData<&'a mut PageTable>,
}

impl<'a> OffsetPageTable<'a> {
    pub unsafe fn new(_level_4_table: &'a mut PageTable, _offset: VirtAddr) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

pub struct MapperFlush<S = Size4KiB> {
    _marker: PhantomData<S>,
}

impl<S> MapperFlush<S> {
    #[inline]
    pub fn flush(self) {}
}

pub struct UnmapError;
pub struct TranslateError;

pub trait Mapper<S = Size4KiB> {
    unsafe fn map_to<A>(
        &mut self,
        _page: Page<S>,
        _frame: PhysFrame<S>,
        _flags: PageTableFlags,
        _frame_allocator: &mut A,
    ) -> Result<MapperFlush<S>, mapper::MapToError>
    where
        A: FrameAllocator<S>;

    fn unmap(&mut self, _page: Page<S>) -> Result<(PhysFrame<S>, MapperFlush<S>), UnmapError>;

    fn translate_page(&self, _page: Page<S>) -> Result<PhysFrame<S>, TranslateError>;
}

pub trait Translate {
    fn translate(&self, _addr: VirtAddr) -> mapper::TranslateResult;
    fn translate_addr(&self, _addr: VirtAddr) -> Option<PhysAddr>;
}

impl<'a> Mapper for OffsetPageTable<'a> {
    unsafe fn map_to<A>(
        &mut self,
        _page: Page<Size4KiB>,
        _frame: PhysFrame<Size4KiB>,
        _flags: PageTableFlags,
        _frame_allocator: &mut A,
    ) -> Result<MapperFlush<Size4KiB>, mapper::MapToError>
    where
        A: FrameAllocator<Size4KiB>,
    {
        Err(mapper::MapToError::FrameAllocationFailed)
    }

    fn unmap(
        &mut self,
        _page: Page<Size4KiB>,
    ) -> Result<(PhysFrame<Size4KiB>, MapperFlush<Size4KiB>), UnmapError> {
        Err(UnmapError)
    }

    fn translate_page(&self, _page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, TranslateError> {
        Err(TranslateError)
    }
}

impl<'a> Translate for OffsetPageTable<'a> {
    fn translate(&self, _addr: VirtAddr) -> mapper::TranslateResult {
        mapper::TranslateResult::NotMapped
    }

    fn translate_addr(&self, _addr: VirtAddr) -> Option<PhysAddr> {
        None
    }
}

pub mod mapper {
    use super::{PhysFrame, Size4KiB, PageTableFlags};

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

pub struct Cr3;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Cr3Flags(u64);

impl Cr3Flags {
    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }
}

impl Cr3 {
    pub fn read() -> (PhysFrame<Size4KiB>, Cr3Flags) {
        (
            PhysFrame::containing_address(PhysAddr::new(0)),
            Cr3Flags::empty(),
        )
    }

    pub fn write(_frame: PhysFrame<Size4KiB>, _flags: Cr3Flags) {}
}

pub mod tlb {
    use super::VirtAddr;

    #[inline]
    pub fn flush(_addr: VirtAddr) {}

    #[inline]
    pub fn flush_all() {}
}
