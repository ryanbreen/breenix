//! Virtual Memory Area (VMA) management
//!
//! This module provides data structures and functions for managing virtual memory regions
//! in a process's address space. VMAs represent contiguous mapped regions with specific
//! permissions and flags.

use alloc::vec::Vec;
use x86_64::VirtAddr;

/// Start of mmap allocation region (below stack)
#[allow(dead_code)]
pub const MMAP_REGION_START: u64 = 0x7000_0000_0000;
/// End of mmap allocation region (gap before stack)
pub const MMAP_REGION_END: u64 = 0x7FFF_FE00_0000;

/// Page size constant for alignment checks
#[allow(dead_code)]
const PAGE_SIZE: u64 = 4096;

/// Memory protection flags (PROT_* constants from mmap)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection(u32);

impl Protection {
    #[allow(dead_code)]
    pub const NONE: Self = Self(0);
    #[allow(dead_code)]
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    #[allow(dead_code)]
    pub const EXEC: Self = Self(4);

    #[allow(dead_code)]
    pub fn bits(&self) -> u32 {
        self.0
    }

    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn from_bits_truncate(bits: u32) -> Self {
        Self(bits)
    }
}

/// Memory mapping flags (MAP_* constants from mmap)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmapFlags(u32);

impl MmapFlags {
    #[allow(dead_code)]
    pub const SHARED: Self = Self(0x01);
    pub const PRIVATE: Self = Self(0x02);
    pub const FIXED: Self = Self(0x10);
    pub const ANONYMOUS: Self = Self(0x20);

    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self(0)
    }

    #[allow(dead_code)]
    pub fn bits(&self) -> u32 {
        self.0
    }

    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn from_bits_truncate(bits: u32) -> Self {
        Self(bits)
    }
}

/// A Virtual Memory Area represents a contiguous mapped region
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Vma {
    /// Start address (page-aligned)
    pub start: VirtAddr,
    /// End address (exclusive, page-aligned)
    pub end: VirtAddr,
    /// Memory protection flags
    pub prot: Protection,
    /// Memory mapping flags
    pub flags: MmapFlags,
}

impl Vma {
    /// Create a new VMA with the given parameters
    pub fn new(start: VirtAddr, end: VirtAddr, prot: Protection, flags: MmapFlags) -> Self {
        Self {
            start,
            end,
            prot,
            flags,
        }
    }

    /// Check if this VMA contains the given address
    #[allow(dead_code)]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr >= self.start && addr < self.end
    }

    /// Check if this VMA overlaps with another VMA
    #[allow(dead_code)]
    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Get the size of this VMA in bytes
    #[allow(dead_code)]
    pub fn size(&self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }
}

/// Errors that can occur during VMA operations
#[derive(Debug)]
#[allow(dead_code)]
pub enum VmaError {
    /// The VMA overlaps with an existing VMA
    Overlap,
    /// The address range is invalid (start >= end, not aligned, etc.)
    InvalidRange,
    /// The requested VMA was not found
    NotFound,
}

/// List of Virtual Memory Areas for a process
#[allow(dead_code)]
pub struct VmaList {
    vmas: Vec<Vma>,
}

impl VmaList {
    /// Create a new empty VMA list
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self { vmas: Vec::new() }
    }

    /// Find a free region of the given size, optionally starting from a hint address
    ///
    /// This searches from MMAP_REGION_END downward (like stack allocation) to find
    /// a gap of sufficient size. If a hint is provided, tries that address first.
    #[allow(dead_code)]
    pub fn find_free_region(&self, size: u64, hint: Option<VirtAddr>) -> Option<VirtAddr> {
        // Ensure size is page-aligned
        let aligned_size = align_up(size, PAGE_SIZE);

        // If hint is provided, try it first
        if let Some(hint_addr) = hint {
            if is_page_aligned(hint_addr.as_u64()) {
                let end_addr = hint_addr.as_u64().checked_add(aligned_size)?;
                if end_addr <= MMAP_REGION_END {
                    let test_vma = Vma::new(
                        hint_addr,
                        VirtAddr::new(end_addr),
                        Protection::NONE,
                        MmapFlags::empty(),
                    );
                    if !self.overlaps_any(&test_vma) {
                        return Some(hint_addr);
                    }
                }
            }
        }

        // Search from high addresses downward
        let mut search_end = MMAP_REGION_END;

        // Sort VMAs by start address for efficient searching
        for vma in self.vmas.iter().rev() {
            // Check if there's enough space between this VMA's start and our search end
            if vma.start.as_u64() >= aligned_size {
                let potential_start = vma.start.as_u64() - aligned_size;
                if potential_start >= MMAP_REGION_START && potential_start + aligned_size <= search_end {
                    return Some(VirtAddr::new(potential_start));
                }
            }
            // Move search end down to this VMA's start
            search_end = vma.start.as_u64();
        }

        // Check if there's space at the beginning of the region
        if search_end >= MMAP_REGION_START + aligned_size {
            let potential_start = search_end - aligned_size;
            if potential_start >= MMAP_REGION_START {
                return Some(VirtAddr::new(potential_start));
            }
        }

        None
    }

    /// Insert a VMA into the list, maintaining sorted order by start address
    #[allow(dead_code)]
    pub fn insert(&mut self, vma: Vma) -> Result<(), VmaError> {
        // Validate the VMA
        if vma.start >= vma.end {
            return Err(VmaError::InvalidRange);
        }
        if !is_page_aligned(vma.start.as_u64()) || !is_page_aligned(vma.end.as_u64()) {
            return Err(VmaError::InvalidRange);
        }

        // Check for overlaps
        if self.overlaps_any(&vma) {
            return Err(VmaError::Overlap);
        }

        // Find insertion point to maintain sorted order
        let insert_pos = self
            .vmas
            .binary_search_by_key(&vma.start.as_u64(), |v| v.start.as_u64())
            .unwrap_or_else(|pos| pos);

        self.vmas.insert(insert_pos, vma);
        Ok(())
    }

    /// Remove VMAs in the given range, returning the removed VMAs
    ///
    /// This handles partial overlaps by splitting VMAs if necessary
    #[allow(dead_code)]
    pub fn remove(&mut self, start: VirtAddr, end: VirtAddr) -> Result<Vec<Vma>, VmaError> {
        if start >= end {
            return Err(VmaError::InvalidRange);
        }
        if !is_page_aligned(start.as_u64()) || !is_page_aligned(end.as_u64()) {
            return Err(VmaError::InvalidRange);
        }

        let mut removed = Vec::new();
        let mut to_insert = Vec::new();

        // Collect indices of VMAs to remove
        let mut i = 0;
        while i < self.vmas.len() {
            let vma = &self.vmas[i];

            // Check if this VMA overlaps with the removal range
            if vma.end <= start || vma.start >= end {
                // No overlap, keep it
                i += 1;
                continue;
            }

            // VMA overlaps with removal range
            let removed_vma = self.vmas.remove(i);

            // If VMA extends before removal range, keep that part
            if removed_vma.start < start {
                to_insert.push(Vma::new(
                    removed_vma.start,
                    start,
                    removed_vma.prot,
                    removed_vma.flags,
                ));
            }

            // If VMA extends after removal range, keep that part
            if removed_vma.end > end {
                to_insert.push(Vma::new(
                    end,
                    removed_vma.end,
                    removed_vma.prot,
                    removed_vma.flags,
                ));
            }

            removed.push(removed_vma);
            // Don't increment i since we removed an element
        }

        // Re-insert the split parts
        for vma in to_insert {
            self.insert(vma).expect("Re-inserting split VMA should never fail");
        }

        if removed.is_empty() {
            Err(VmaError::NotFound)
        } else {
            Ok(removed)
        }
    }

    /// Find the VMA containing the given address
    #[allow(dead_code)]
    pub fn find(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vmas.iter().find(|vma| vma.contains(addr))
    }

    /// Iterate over all VMAs
    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.iter()
    }

    /// Check if the given VMA overlaps with any existing VMA
    #[allow(dead_code)]
    fn overlaps_any(&self, vma: &Vma) -> bool {
        self.vmas.iter().any(|existing| existing.overlaps(vma))
    }
}

impl Default for VmaList {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if an address is page-aligned
#[allow(dead_code)]
fn is_page_aligned(addr: u64) -> bool {
    addr % PAGE_SIZE == 0
}

/// Align a value up to the nearest multiple of align
#[allow(dead_code)]
fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vma_contains() {
        let vma = Vma::new(
            VirtAddr::new(0x1000),
            VirtAddr::new(0x2000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );

        assert!(vma.contains(VirtAddr::new(0x1000)));
        assert!(vma.contains(VirtAddr::new(0x1500)));
        assert!(vma.contains(VirtAddr::new(0x1fff)));
        assert!(!vma.contains(VirtAddr::new(0x2000)));
        assert!(!vma.contains(VirtAddr::new(0x500)));
    }

    #[test]
    fn test_vma_overlaps() {
        let vma1 = Vma::new(
            VirtAddr::new(0x1000),
            VirtAddr::new(0x2000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        let vma2 = Vma::new(
            VirtAddr::new(0x1500),
            VirtAddr::new(0x2500),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        let vma3 = Vma::new(
            VirtAddr::new(0x3000),
            VirtAddr::new(0x4000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );

        assert!(vma1.overlaps(&vma2));
        assert!(vma2.overlaps(&vma1));
        assert!(!vma1.overlaps(&vma3));
        assert!(!vma3.overlaps(&vma1));
    }

    #[test]
    fn test_vma_list_insert() {
        let mut list = VmaList::new();

        let vma1 = Vma::new(
            VirtAddr::new(0x1000),
            VirtAddr::new(0x2000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        assert!(list.insert(vma1).is_ok());

        // Try to insert overlapping VMA
        let vma2 = Vma::new(
            VirtAddr::new(0x1500),
            VirtAddr::new(0x2500),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        assert!(matches!(list.insert(vma2), Err(VmaError::Overlap)));

        // Insert non-overlapping VMA
        let vma3 = Vma::new(
            VirtAddr::new(0x3000),
            VirtAddr::new(0x4000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        assert!(list.insert(vma3).is_ok());
    }

    #[test]
    fn test_vma_list_find() {
        let mut list = VmaList::new();

        let vma1 = Vma::new(
            VirtAddr::new(0x1000),
            VirtAddr::new(0x2000),
            Protection::READ,
            MmapFlags::PRIVATE,
        );
        list.insert(vma1).unwrap();

        assert!(list.find(VirtAddr::new(0x1500)).is_some());
        assert!(list.find(VirtAddr::new(0x3000)).is_none());
    }

    #[test]
    fn test_alignment() {
        assert!(is_page_aligned(0));
        assert!(is_page_aligned(4096));
        assert!(is_page_aligned(8192));
        assert!(!is_page_aligned(1));
        assert!(!is_page_aligned(4097));

        assert_eq!(align_up(0, 4096), 0);
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
    }
}
