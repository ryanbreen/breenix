//! The firmware's description of physical memory, reduced to the page-aligned
//! ranges the ARM64 kernel's frame allocator may hand out.
//!
//! Two boot paths describe RAM: QEMU's direct ELF boot leaves a flattened
//! device tree at the start of RAM, and the UEFI loader passes the
//! conventional-memory regions of the UEFI memory map in `HardwareConfig`.
//! Either description is first clipped to the window the boot page tables map
//! as normal memory, then the reservations are subtracted from it. When a set
//! runs out of slots it drops memory and keeps the reservations.

/// Select free pages from the final UEFI memory map. Descriptor tuples are
/// (type, attributes, physical start, page count), from UEFI 2.10 section 7.2,
/// Tables 7.5 and 7.6. Type 7 is EfiConventionalMemory; attribute bit 63 is
/// EFI_MEMORY_RUNTIME. Boot-services and loader pages remain unavailable while
/// firmware DMA can still own them, even after ExitBootServices.
pub fn final_uefi_conventional_ranges(
    descriptors: impl IntoIterator<Item = (u32, u64, u64, u64)>,
) -> RangeSet {
    let mut ranges = RangeSet::new();
    for (kind, attributes, base, pages) in descriptors {
        if kind == 7 && attributes & (1 << 63) == 0 {
            ranges.insert(PhysRange::from_base_size(base, pages.saturating_mul(PAGE_SIZE)));
        }
    }
    ranges.align_to_pages();
    ranges
}

/// Granule of the frame allocator.
pub const PAGE_SIZE: u64 = 4096;

/// Ranges a [`RangeSet`] holds. The kernel's allocator accepts more, but a
/// firmware map that needs more than this after merging is dropped from the
/// top rather than overflowing.
pub const MAX_RANGES: usize = 32;

/// A half-open physical address range `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysRange {
    pub start: u64,
    pub end: u64,
}

impl PhysRange {
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// The range `[base, base + size)`, saturating at the top of the address space.
    pub const fn from_base_size(base: u64, size: u64) -> Self {
        Self {
            start: base,
            end: base.saturating_add(size),
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub const fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// Sorted, disjoint, non-adjacent ranges.
#[derive(Clone, Copy, Debug)]
pub struct RangeSet {
    ranges: [PhysRange; MAX_RANGES],
    len: usize,
    truncated: bool,
}

impl Default for RangeSet {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeSet {
    pub const fn new() -> Self {
        Self {
            ranges: [PhysRange::new(0, 0); MAX_RANGES],
            len: 0,
            truncated: false,
        }
    }

    pub fn as_slice(&self) -> &[PhysRange] {
        &self.ranges[..self.len]
    }

    /// Whether memory was dropped because the set ran out of slots.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn total_bytes(&self) -> u64 {
        self.as_slice().iter().map(PhysRange::len).sum()
    }

    /// Add `range`, merging it with any range it overlaps or touches. When no
    /// slot is free the highest range in the result is dropped.
    pub fn insert(&mut self, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        let mut merged = range;
        // First range that ends at or after the new start can merge or follows.
        let first = self.ranges[..self.len]
            .iter()
            .position(|r| r.end >= merged.start)
            .unwrap_or(self.len);
        let mut last = first;
        while last < self.len && self.ranges[last].start <= merged.end {
            merged.start = merged.start.min(self.ranges[last].start);
            merged.end = merged.end.max(self.ranges[last].end);
            last += 1;
        }
        let removed = last - first;
        if removed == 0 {
            if self.len == MAX_RANGES {
                self.truncated = true;
                if first == MAX_RANGES {
                    return;
                }
                self.len -= 1;
            }
            self.ranges.copy_within(first..self.len, first + 1);
            self.len += 1;
        } else {
            self.ranges.copy_within(last..self.len, first + 1);
            self.len -= removed - 1;
        }
        self.ranges[first] = merged;
    }

    /// Remove `hole` from the set. A split that needs a slot the set does
    /// not have drops the piece above the hole.
    pub fn subtract(&mut self, hole: PhysRange) {
        if hole.is_empty() {
            return;
        }
        let mut index = 0;
        while index < self.len {
            let range = self.ranges[index];
            if range.end <= hole.start || range.start >= hole.end {
                index += 1;
                continue;
            }
            let below = PhysRange::new(range.start, hole.start.min(range.end));
            let above = PhysRange::new(hole.end.max(range.start), range.end);
            match (below.is_empty(), above.is_empty()) {
                (true, true) => {
                    self.ranges.copy_within(index + 1..self.len, index);
                    self.len -= 1;
                }
                (false, true) => {
                    self.ranges[index] = below;
                    index += 1;
                }
                (true, false) => {
                    self.ranges[index] = above;
                    index += 1;
                }
                (false, false) => {
                    self.ranges[index] = below;
                    if self.len == MAX_RANGES {
                        self.truncated = true;
                        if index + 1 == MAX_RANGES {
                            return;
                        }
                        self.len -= 1;
                    }
                    self.ranges.copy_within(index + 1..self.len, index + 2);
                    self.len += 1;
                    self.ranges[index + 1] = above;
                    index += 2;
                }
            }
        }
    }

    /// Keep only what lies inside `window`.
    pub fn clamp(&mut self, window: PhysRange) {
        self.subtract(PhysRange::new(0, window.start));
        self.subtract(PhysRange::new(window.end, u64::MAX));
    }

    /// Shrink each range to whole pages, dropping ranges without a whole page.
    pub fn align_to_pages(&mut self) {
        let mut kept = 0;
        for index in 0..self.len {
            let range = self.ranges[index];
            let start = match range.start.checked_add(PAGE_SIZE - 1) {
                Some(rounded) => rounded & !(PAGE_SIZE - 1),
                None => continue,
            };
            let end = range.end & !(PAGE_SIZE - 1);
            if start < end {
                self.ranges[kept] = PhysRange::new(start, end);
                kept += 1;
            }
        }
        self.len = kept;
    }
}

/// Build the frame pool from RAM regions, keeping only what lies inside
/// `window` and outside the `reserved` ranges.
pub fn frame_pool(
    ram: impl IntoIterator<Item = PhysRange>,
    reserved: impl IntoIterator<Item = PhysRange>,
    window: PhysRange,
) -> RangeSet {
    let mut pool = RangeSet::new();
    for range in ram {
        let clipped = PhysRange::new(range.start.max(window.start), range.end.min(window.end));
        pool.insert(clipped);
    }
    for hole in reserved {
        pool.subtract(hole);
    }
    pool.align_to_pages();
    pool
}

/// Build the frame pool from a flattened device tree: its memory nodes, less
/// its reservation block, its `/reserved-memory` children, the blob itself
/// (at `blob_phys`) and `reserved`.
pub fn frame_pool_from_fdt(
    blob: &[u8],
    blob_phys: u64,
    reserved: impl IntoIterator<Item = PhysRange>,
    window: PhysRange,
) -> Result<RangeSet, FdtError> {
    let mut pool = RangeSet::new();
    fdt::walk(blob, |kind, range| {
        if kind == fdt::RegionKind::Memory {
            pool.insert(PhysRange::new(
                range.start.max(window.start),
                range.end.min(window.end),
            ));
        }
    })?;
    fdt::walk(blob, |kind, range| {
        if kind == fdt::RegionKind::Reserved {
            pool.subtract(range);
        }
    })?;
    pool.subtract(PhysRange::from_base_size(blob_phys, blob.len() as u64));
    for hole in reserved {
        pool.subtract(hole);
    }
    pool.align_to_pages();
    Ok(pool)
}

pub use fdt::FdtError;

/// A reader for the parts of a flattened device tree (Devicetree
/// Specification v0.4, chapter 5) that describe physical memory.
pub mod fdt {
    use super::PhysRange;

    pub const MAGIC: u32 = 0xd00d_feed;
    /// Size of the version-17 header.
    pub const HEADER_SIZE: usize = 40;
    /// Newest structure-block version this reader implements. A tree whose
    /// `last_comp_version` is newer needs a newer reader.
    const READER_VERSION: u32 = 17;

    const FDT_BEGIN_NODE: u32 = 1;
    const FDT_END_NODE: u32 = 2;
    const FDT_PROP: u32 = 3;
    const FDT_NOP: u32 = 4;
    const FDT_END: u32 = 9;

    /// Nodes deeper than this are skipped without being interpreted: memory
    /// nodes are children of the root, and reserved regions are children of
    /// `/reserved-memory`.
    const MAX_DEPTH: usize = 3;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FdtError {
        BadMagic,
        UnsupportedVersion,
        Truncated,
        BadOffset,
        BadToken,
        BadCells,
    }

    impl FdtError {
        pub const fn as_str(self) -> &'static str {
            match self {
                FdtError::BadMagic => "no device tree magic",
                FdtError::UnsupportedVersion => "device tree version too new",
                FdtError::Truncated => "device tree truncated",
                FdtError::BadOffset => "device tree block offset out of range",
                FdtError::BadToken => "malformed device tree structure",
                FdtError::BadCells => "unsupported device tree cell sizes",
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RegionKind {
        /// A `reg` entry of an enabled `device_type = "memory"` node.
        Memory,
        /// A memory-reservation-block entry or a `reg` entry of a
        /// `/reserved-memory` child.
        Reserved,
    }

    fn be32(blob: &[u8], offset: usize) -> Result<u32, FdtError> {
        let end = offset.checked_add(4).ok_or(FdtError::Truncated)?;
        let bytes = blob.get(offset..end).ok_or(FdtError::Truncated)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn be64(blob: &[u8], offset: usize) -> Result<u64, FdtError> {
        Ok(((be32(blob, offset)? as u64) << 32) | be32(blob, offset + 4)? as u64)
    }

    /// The blob's total size from its header, or why the header is not a
    /// device tree this reader accepts.
    pub fn total_size(header: &[u8]) -> Result<usize, FdtError> {
        if be32(header, 0)? != MAGIC {
            return Err(FdtError::BadMagic);
        }
        if be32(header, 24)? > READER_VERSION {
            return Err(FdtError::UnsupportedVersion);
        }
        let size = be32(header, 4)? as usize;
        if size < HEADER_SIZE {
            return Err(FdtError::Truncated);
        }
        Ok(size)
    }

    fn align4(offset: usize) -> usize {
        (offset + 3) & !3
    }

    fn cstr(blob: &[u8], offset: usize, limit: usize) -> Result<&[u8], FdtError> {
        let tail = blob.get(offset..limit).ok_or(FdtError::BadOffset)?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(FdtError::Truncated)?;
        Ok(&tail[..len])
    }

    /// The value of a string property without its terminating NUL.
    fn string_value(value: &[u8]) -> &[u8] {
        value.split(|&b| b == 0).next().unwrap_or(&[])
    }

    fn cells_value(value: &[u8]) -> Result<u32, FdtError> {
        if value.len() != 4 {
            return Err(FdtError::BadCells);
        }
        be32(value, 0)
    }

    fn read_cells(value: &[u8], offset: usize, cells: u32) -> Result<u64, FdtError> {
        match cells {
            1 => Ok(be32(value, offset)? as u64),
            2 => be64(value, offset),
            _ => Err(FdtError::BadCells),
        }
    }

    /// Report each `(base, size)` entry of a `reg` value.
    fn each_reg(
        value: &[u8],
        address_cells: u32,
        size_cells: u32,
        kind: RegionKind,
        visit: &mut impl FnMut(RegionKind, PhysRange),
    ) -> Result<(), FdtError> {
        if !(1..=2).contains(&address_cells) || !(1..=2).contains(&size_cells) {
            return Err(FdtError::BadCells);
        }
        let entry = (address_cells + size_cells) as usize * 4;
        if value.len() % entry != 0 {
            return Err(FdtError::BadCells);
        }
        let mut offset = 0;
        while offset < value.len() {
            let base = read_cells(value, offset, address_cells)?;
            let size = read_cells(value, offset + address_cells as usize * 4, size_cells)?;
            visit(kind, PhysRange::from_base_size(base, size));
            offset += entry;
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct Node<'a> {
        /// `#address-cells` / `#size-cells` this node's children use.
        child_address_cells: u32,
        child_size_cells: u32,
        is_memory: bool,
        is_reserved_memory: bool,
        disabled: bool,
        reg: Option<&'a [u8]>,
    }

    impl Node<'_> {
        const fn new(address_cells: u32, size_cells: u32) -> Self {
            Self {
                child_address_cells: address_cells,
                child_size_cells: size_cells,
                is_memory: false,
                is_reserved_memory: false,
                disabled: false,
                reg: None,
            }
        }
    }

    /// Report the memory and reserved ranges `blob` describes. `blob` must
    /// span the whole tree (`total_size` bytes).
    pub fn walk(blob: &[u8], mut visit: impl FnMut(RegionKind, PhysRange)) -> Result<(), FdtError> {
        let size = total_size(blob)?;
        let blob = blob.get(..size).ok_or(FdtError::Truncated)?;
        let struct_offset = be32(blob, 8)? as usize;
        let strings_offset = be32(blob, 12)? as usize;
        let reserve_offset = be32(blob, 16)? as usize;
        if struct_offset >= size || strings_offset > size || reserve_offset >= size {
            return Err(FdtError::BadOffset);
        }

        let mut offset = reserve_offset;
        loop {
            let base = be64(blob, offset)?;
            let len = be64(blob, offset + 8)?;
            if base == 0 && len == 0 {
                break;
            }
            visit(RegionKind::Reserved, PhysRange::from_base_size(base, len));
            offset += 16;
        }

        // The root's own cells default to 2 and 1 (specification 2.3.5).
        let mut stack = [Node::new(2, 1); MAX_DEPTH + 1];
        let mut depth = 0usize;
        let mut offset = struct_offset;
        loop {
            let token = be32(blob, offset)?;
            offset += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let name = cstr(blob, offset, size)?;
                    offset = align4(offset + name.len() + 1);
                    depth += 1;
                    if depth <= MAX_DEPTH {
                        let parent = stack[depth - 1];
                        let unit = name.split(|&b| b == b'@').next().unwrap_or(&[]);
                        // Children inherit their parent's cells unless they set their own.
                        let mut node =
                            Node::new(parent.child_address_cells, parent.child_size_cells);
                        node.is_reserved_memory = depth == 2 && unit == b"reserved-memory";
                        stack[depth] = node;
                    }
                }
                FDT_END_NODE => {
                    if depth == 0 {
                        return Err(FdtError::BadToken);
                    }
                    if depth <= MAX_DEPTH {
                        let node = stack[depth];
                        let parent = stack[depth - 1];
                        if let Some(reg) = node.reg {
                            let kind = if depth == 2 && node.is_memory && !node.disabled {
                                Some(RegionKind::Memory)
                            } else if depth == 3 && parent.is_reserved_memory {
                                Some(RegionKind::Reserved)
                            } else {
                                None
                            };
                            if let Some(kind) = kind {
                                each_reg(
                                    reg,
                                    parent.child_address_cells,
                                    parent.child_size_cells,
                                    kind,
                                    &mut visit,
                                )?;
                            }
                        }
                    }
                    depth -= 1;
                }
                FDT_PROP => {
                    let len = be32(blob, offset)? as usize;
                    let name_offset = be32(blob, offset + 4)? as usize;
                    let value_start = offset + 8;
                    let value_end = value_start.checked_add(len).ok_or(FdtError::Truncated)?;
                    let value = blob
                        .get(value_start..value_end)
                        .ok_or(FdtError::Truncated)?;
                    offset = align4(value_end);
                    if depth == 0 {
                        return Err(FdtError::BadToken);
                    }
                    if depth > MAX_DEPTH {
                        continue;
                    }
                    let name_at = strings_offset
                        .checked_add(name_offset)
                        .ok_or(FdtError::BadOffset)?;
                    let name = cstr(blob, name_at, size)?;
                    let node = &mut stack[depth];
                    match name {
                        b"#address-cells" => node.child_address_cells = cells_value(value)?,
                        b"#size-cells" => node.child_size_cells = cells_value(value)?,
                        b"device_type" => node.is_memory = string_value(value) == b"memory",
                        b"status" => {
                            let status = string_value(value);
                            node.disabled = status != b"okay" && status != b"ok";
                        }
                        b"reg" => node.reg = Some(value),
                        _ => {}
                    }
                }
                FDT_NOP => {}
                FDT_END => {
                    return if depth == 0 {
                        Ok(())
                    } else {
                        Err(FdtError::BadToken)
                    };
                }
                _ => return Err(FdtError::BadToken),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::fdt::{self, FdtError, RegionKind};
    use super::*;
    use std::vec::Vec;

    fn r(start: u64, end: u64) -> PhysRange {
        PhysRange::new(start, end)
    }

    #[test]
    fn final_uefi_map_keeps_dma_loader_and_runtime_pages_unavailable() {
        let ranges = final_uefi_conventional_ranges([
            (7, 0, 0x4400_0000, 1),
            (4, 0, 0x4400_1000, 1), // Boot-services DMA buffer.
            (3, 0, 0x4400_2000, 1), // Boot-services code.
            (2, 0, 0x4400_3000, 1), // Loader allocation made after initial map.
            (1, 0, 0x4400_4000, 1), // Loader code.
            (6, 1 << 63, 0x4400_5000, 1), // Runtime data.
            (7, 1 << 63, 0x4400_6000, 1), // Runtime attribute wins over type.
            (7, 0, 0x4400_7000, 2),
        ]);
        assert_eq!(ranges.as_slice(), &[
            r(0x4400_0000, 0x4400_1000),
            r(0x4400_7000, 0x4400_9000),
        ]);
        assert!(final_uefi_conventional_ranges([]).as_slice().is_empty());
    }

    #[test]
    fn insert_merges_overlapping_and_adjacent_ranges_in_order() {
        let mut set = RangeSet::new();
        set.insert(r(0x5000, 0x6000));
        set.insert(r(0x1000, 0x2000));
        set.insert(r(0x3000, 0x4000));
        set.insert(r(0x2000, 0x3000));
        set.insert(r(0x0, 0x0));
        assert_eq!(set.as_slice(), &[r(0x1000, 0x4000), r(0x5000, 0x6000)]);
        set.insert(r(0x3800, 0x5800));
        assert_eq!(set.as_slice(), &[r(0x1000, 0x6000)]);
        assert!(!set.truncated());
    }

    #[test]
    fn subtract_splits_trims_and_removes() {
        let mut set = RangeSet::new();
        set.insert(r(0x1000, 0x9000));
        set.insert(r(0xa000, 0xc000));
        set.subtract(r(0x3000, 0x4000));
        assert_eq!(
            set.as_slice(),
            &[r(0x1000, 0x3000), r(0x4000, 0x9000), r(0xa000, 0xc000)]
        );
        set.subtract(r(0x0, 0x2000));
        set.subtract(r(0x8000, 0xb000));
        assert_eq!(
            set.as_slice(),
            &[r(0x2000, 0x3000), r(0x4000, 0x8000), r(0xb000, 0xc000)]
        );
        set.subtract(r(0x2000, 0x3000));
        assert_eq!(set.as_slice(), &[r(0x4000, 0x8000), r(0xb000, 0xc000)]);
    }

    #[test]
    fn a_full_set_drops_memory_never_a_reservation() {
        let mut set = RangeSet::new();
        for i in 0..MAX_RANGES as u64 {
            set.insert(r(i * 0x10000, i * 0x10000 + 0x8000));
        }
        assert_eq!(set.as_slice().len(), MAX_RANGES);
        set.insert(r(0x1000_0000, 0x1000_1000));
        assert!(set.truncated());
        assert_eq!(set.as_slice().last(), Some(&r(0x1f_0000, 0x1f_8000)));

        let mut set = RangeSet::new();
        for i in 0..MAX_RANGES as u64 {
            set.insert(r(i * 0x10000, i * 0x10000 + 0x8000));
        }
        // Splitting the first range needs a slot: the top range goes, the hole stays.
        set.subtract(r(0x2000, 0x3000));
        assert!(set.truncated());
        assert_eq!(&set.as_slice()[..2], &[r(0x0, 0x2000), r(0x3000, 0x8000)]);
        assert!(set
            .as_slice()
            .iter()
            .all(|x| x.end <= 0x2000 || x.start >= 0x3000));
        // Splitting the last slot drops the piece above the hole.
        let last = *set.as_slice().last().unwrap();
        set.subtract(r(last.start + 0x1000, last.start + 0x2000));
        assert_eq!(
            set.as_slice().last(),
            Some(&r(last.start, last.start + 0x1000))
        );
    }

    #[test]
    fn clamp_and_align_keep_whole_pages_inside_the_window() {
        let mut set = RangeSet::new();
        set.insert(r(0x0, 0x1800));
        set.insert(r(0x2001, 0x5fff));
        set.insert(r(0x8000, 0x8800));
        set.clamp(r(0x1000, 0x8400));
        set.align_to_pages();
        assert_eq!(set.as_slice(), &[r(0x3000, 0x5000)]);
    }

    #[test]
    fn frame_pool_clips_subtracts_and_aligns() {
        let pool = frame_pool(
            [r(0x4000_0000, 0x6000_0000)],
            [r(0x4000_0000, 0x4400_0000), r(0x5000_0000, 0x5620_0000)],
            r(0x4000_0000, 0x1_0000_0000),
        );
        assert_eq!(
            pool.as_slice(),
            &[r(0x4400_0000, 0x5000_0000), r(0x5620_0000, 0x6000_0000)]
        );
        assert_eq!(pool.total_bytes(), 0x0c00_0000 + 0x09e0_0000);

        // RAM above what the page tables map, and below the window, is left out.
        let pool = frame_pool(
            [r(0x1000_0000, 0x2000_0000), r(0x8000_0000, 0x2_0000_0000)],
            [r(0x8000_0000, 0x8400_0000)],
            r(0x8000_0000, 0x1_0000_0000),
        );
        assert_eq!(pool.as_slice(), &[r(0x8400_0000, 0x1_0000_0000)]);
    }

    /// Builds a flattened device tree.
    struct Fdt {
        structure: Vec<u8>,
        strings: Vec<u8>,
        reserve: Vec<(u64, u64)>,
        last_compatible: u32,
    }

    impl Fdt {
        fn new() -> Self {
            Self {
                structure: Vec::new(),
                strings: Vec::new(),
                reserve: Vec::new(),
                last_compatible: 16,
            }
        }

        fn token(&mut self, token: u32) -> &mut Self {
            self.structure.extend_from_slice(&token.to_be_bytes());
            self
        }

        fn begin(&mut self, name: &str) -> &mut Self {
            self.token(1);
            self.structure.extend_from_slice(name.as_bytes());
            self.structure.push(0);
            while self.structure.len() % 4 != 0 {
                self.structure.push(0);
            }
            self
        }

        fn end(&mut self) -> &mut Self {
            self.token(2)
        }

        fn prop(&mut self, name: &str, value: &[u8]) -> &mut Self {
            let name_offset = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.token(3);
            self.structure
                .extend_from_slice(&(value.len() as u32).to_be_bytes());
            self.structure.extend_from_slice(&name_offset.to_be_bytes());
            self.structure.extend_from_slice(value);
            while self.structure.len() % 4 != 0 {
                self.structure.push(0);
            }
            self
        }

        fn prop_str(&mut self, name: &str, value: &str) -> &mut Self {
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            self.prop(name, &bytes)
        }

        fn prop_cells(&mut self, name: &str, cells: &[u32]) -> &mut Self {
            let bytes: Vec<u8> = cells.iter().flat_map(|c| c.to_be_bytes()).collect();
            self.prop(name, &bytes)
        }

        fn build(&mut self) -> Vec<u8> {
            self.token(9);
            let reserve_offset = fdt::HEADER_SIZE;
            let struct_offset = reserve_offset + (self.reserve.len() + 1) * 16;
            let strings_offset = struct_offset + self.structure.len();
            let total = strings_offset + self.strings.len();
            let mut blob = Vec::new();
            for word in [
                fdt::MAGIC,
                total as u32,
                struct_offset as u32,
                strings_offset as u32,
                reserve_offset as u32,
                17,
                self.last_compatible,
                0,
                self.strings.len() as u32,
                self.structure.len() as u32,
            ] {
                blob.extend_from_slice(&word.to_be_bytes());
            }
            for &(base, size) in self.reserve.iter().chain([(0, 0)].iter()) {
                blob.extend_from_slice(&base.to_be_bytes());
                blob.extend_from_slice(&size.to_be_bytes());
            }
            blob.extend_from_slice(&self.structure);
            blob.extend_from_slice(&self.strings);
            blob
        }
    }

    fn regions(blob: &[u8]) -> Result<Vec<(RegionKind, PhysRange)>, FdtError> {
        let mut out = Vec::new();
        fdt::walk(blob, |kind, range| out.push((kind, range)))?;
        Ok(out)
    }

    /// The memory description QEMU's `virt` machine generates for `-m 512`.
    fn qemu_virt_512() -> Vec<u8> {
        let mut t = Fdt::new();
        t.begin("")
            .prop_cells("#address-cells", &[2])
            .prop_cells("#size-cells", &[2])
            .prop_str("compatible", "linux,dummy-virt");
        t.begin("memory@40000000")
            .prop_cells("reg", &[0, 0x4000_0000, 0, 0x2000_0000])
            .prop_str("device_type", "memory")
            .end();
        t.begin("pl011@9000000")
            .prop_cells("reg", &[0, 0x0900_0000, 0, 0x1000])
            .end();
        t.end();
        t.build()
    }

    #[test]
    fn reads_qemu_virt_memory_with_device_type_after_reg() {
        let blob = qemu_virt_512();
        assert_eq!(fdt::total_size(&blob), Ok(blob.len()));
        assert_eq!(
            regions(&blob).unwrap(),
            [(RegionKind::Memory, r(0x4000_0000, 0x6000_0000))]
        );
    }

    #[test]
    fn reads_reservations_and_skips_disabled_and_nested_nodes() {
        let mut t = Fdt::new();
        t.reserve.push((0x4800_0000, 0x10_0000));
        t.begin("")
            .prop_cells("#address-cells", &[1])
            .prop_cells("#size-cells", &[1]);
        t.begin("memory@80000000")
            .prop_str("device_type", "memory")
            .prop_cells("reg", &[0x8000_0000, 0x1000_0000, 0xa000_0000, 0x1000_0000])
            .end();
        t.begin("memory@c0000000")
            .prop_str("device_type", "memory")
            .prop_str("status", "disabled")
            .prop_cells("reg", &[0xc000_0000, 0x1000_0000])
            .end();
        t.begin("reserved-memory")
            .prop_cells("#address-cells", &[2])
            .prop_cells("#size-cells", &[2]);
        t.begin("secure@84000000")
            .prop_cells("reg", &[0, 0x8400_0000, 0, 0x20_0000])
            .prop("no-map", &[])
            .end();
        t.begin("pool").prop_cells("size", &[0, 0x40_0000]).end();
        t.end();
        t.begin("soc")
            .prop_cells("#address-cells", &[1])
            .prop_cells("#size-cells", &[1]);
        t.begin("memory@90000000")
            .prop_str("device_type", "memory")
            .prop_cells("reg", &[0x9000_0000, 0x1000])
            .end();
        t.end();
        t.end();
        let blob = t.build();
        assert_eq!(
            regions(&blob).unwrap(),
            [
                (RegionKind::Reserved, r(0x4800_0000, 0x4810_0000)),
                (RegionKind::Memory, r(0x8000_0000, 0x9000_0000)),
                (RegionKind::Memory, r(0xa000_0000, 0xb000_0000)),
                (RegionKind::Reserved, r(0x8400_0000, 0x8420_0000)),
            ]
        );

        let pool = frame_pool_from_fdt(
            &blob,
            0x8800_0000,
            [r(0x8000_0000, 0x8100_0000)],
            r(0, u64::MAX),
        )
        .unwrap();
        assert_eq!(
            pool.as_slice(),
            &[
                r(0x8100_0000, 0x8400_0000),
                r(0x8420_0000, 0x8800_0000),
                r(0x8800_1000, 0x9000_0000),
                r(0xa000_0000, 0xb000_0000),
            ]
        );
    }

    #[test]
    fn qemu_virt_pool_excludes_the_kernel_and_the_blob() {
        let blob = qemu_virt_512();
        let pool = frame_pool_from_fdt(
            &blob,
            0x4000_0000,
            [r(0x4000_0000, 0x4400_0000), r(0x5000_0000, 0x5620_0000)],
            r(0x4000_0000, 0x1_0000_0000),
        )
        .unwrap();
        assert_eq!(
            pool.as_slice(),
            &[r(0x4400_0000, 0x5000_0000), r(0x5620_0000, 0x6000_0000)]
        );
    }

    #[test]
    fn rejects_malformed_trees() {
        let good = qemu_virt_512();
        let mut bad = good.clone();
        bad[0] = 0;
        assert_eq!(fdt::total_size(&bad), Err(FdtError::BadMagic));
        assert_eq!(regions(&bad), Err(FdtError::BadMagic));
        assert_eq!(regions(&good[..good.len() - 1]), Err(FdtError::Truncated));

        let mut t = Fdt::new();
        t.begin("").end();
        assert_eq!(regions(&t.build()), Ok(Vec::new()));
        t = Fdt::new();
        t.last_compatible = 18;
        t.begin("").end();
        assert_eq!(regions(&t.build()), Err(FdtError::UnsupportedVersion));

        let mut t = Fdt::new();
        t.begin("").prop_cells("#address-cells", &[3]);
        t.begin("memory@0")
            .prop_str("device_type", "memory")
            .prop_cells("reg", &[0, 0, 0, 0])
            .end();
        t.end();
        assert_eq!(regions(&t.build()), Err(FdtError::BadCells));

        let mut t = Fdt::new();
        t.begin("")
            .prop_cells("#address-cells", &[2])
            .prop_cells("#size-cells", &[2]);
        t.begin("memory@0")
            .prop_str("device_type", "memory")
            .prop_cells("reg", &[0, 0, 0])
            .end();
        t.end();
        assert_eq!(regions(&t.build()), Err(FdtError::BadCells));

        // A tree whose root node is not closed.
        let mut t = Fdt::new();
        t.begin("");
        assert_eq!(regions(&t.build()), Err(FdtError::BadToken));

        let mut t = Fdt::new();
        t.begin("").end().token(7);
        assert_eq!(regions(&t.build()), Err(FdtError::BadToken));
    }
}
