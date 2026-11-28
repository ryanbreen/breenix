//! UEFI 2.10 §7.2.3 memory-map metadata and descriptor wire format.

use crate::hw_config::UefiMapHandoff;
use crate::phys_memory::{PhysRange, PAGE_SIZE};

pub const BOOT_MAP_MEMORY_TYPE: u32 = 0x8000_4252;
pub const CONVENTIONAL: u32 = 7;
pub const ATTR_RUNTIME: u64 = 1 << 63;
pub const ATTR_SPECIFIC_PURPOSE: u64 = 1 << 18;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaError {
    Address = 1,
    Version = 2,
    Stride = 3,
    Length = 4,
}

pub fn check_meta(version: u32, stride: usize, size: u64, capacity: u64) -> Result<(), MetaError> {
    if version != 1 {
        return Err(MetaError::Version);
    }
    if stride < 40 || stride % 8 != 0 || u32::try_from(stride).is_err() {
        return Err(MetaError::Stride);
    }
    if size < stride as u64
        || size % stride as u64 != 0
        || size > capacity
        || size > isize::MAX as u64
    {
        return Err(MetaError::Length);
    }
    Ok(())
}

pub fn encode_uefi_map(buffer: &[u8], size: usize, stride: usize, version: u32) -> UefiMapHandoff {
    UefiMapHandoff {
        map_phys: buffer.as_ptr() as u64,
        map_size: size as u64,
        descriptor_size: u32::try_from(stride).unwrap_or(0),
        descriptor_version: version,
        loader_check: check_meta(version, stride, size as u64, buffer.len() as u64)
            .err()
            .map_or(0, |error| error as u32),
        _reserved: 0,
    }
}

pub fn check_address(wire: UefiMapHandoff) -> Result<PhysRange, MetaError> {
    let end = wire
        .map_phys
        .checked_add(wire.map_size)
        .ok_or(MetaError::Address)?;
    if wire.map_phys == 0 || wire.map_phys % 8 != 0 {
        return Err(MetaError::Address);
    }
    Ok(PhysRange::new(wire.map_phys, end))
}

#[derive(Clone, Copy)]
pub struct Descriptor {
    pub kind: u32,
    pub start: u64,
    pub pages: u64,
    pub attributes: u64,
}

impl Descriptor {
    pub fn read(record: &[u8]) -> Self {
        Self {
            kind: u32::from_le_bytes(record[..4].try_into().unwrap()),
            start: u64::from_le_bytes(record[8..16].try_into().unwrap()),
            pages: u64::from_le_bytes(record[24..32].try_into().unwrap()),
            attributes: u64::from_le_bytes(record[32..40].try_into().unwrap()),
        }
    }

    pub fn usable_attributes(self) -> bool {
        self.attributes & (ATTR_RUNTIME | ATTR_SPECIFIC_PURPOSE) == 0
    }

    pub fn extent(self) -> Option<PhysRange> {
        let end = self.start.checked_add(self.pages.checked_mul(PAGE_SIZE)?)?;
        (self.pages != 0 && self.start % PAGE_SIZE == 0).then_some(PhysRange::new(self.start, end))
    }

    /// Preserve even malformed reservations. Use the last representable
    /// aligned end since the allocator's ranges have u64 exclusive ends.
    pub fn reservation(self) -> PhysRange {
        let raw_end = self
            .start
            .saturating_add(self.pages.saturating_mul(PAGE_SIZE));
        PhysRange::new(
            self.start & !(PAGE_SIZE - 1),
            raw_end.saturating_add(PAGE_SIZE - 1) & !(PAGE_SIZE - 1),
        )
    }
}

/// Heapsort complete records, keeping firmware-private tail bytes attached.
pub fn sort(bytes: &mut [u8], stride: usize) {
    fn start(bytes: &[u8], stride: usize, index: usize) -> u64 {
        let offset = index * stride + 8;
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }
    fn swap(bytes: &mut [u8], stride: usize, a: usize, b: usize) {
        let (left, right) = bytes.split_at_mut(b * stride);
        left[a * stride..(a + 1) * stride].swap_with_slice(&mut right[..stride]);
    }
    fn sift(bytes: &mut [u8], stride: usize, mut parent: usize, len: usize) {
        while parent < len / 2 {
            let mut child = 2 * parent + 1;
            if child + 1 < len && start(bytes, stride, child) < start(bytes, stride, child + 1) {
                child += 1;
            }
            if start(bytes, stride, parent) >= start(bytes, stride, child) {
                break;
            }
            swap(bytes, stride, parent, child);
            parent = child;
        }
    }
    let count = bytes.len() / stride;
    for parent in (0..count / 2).rev() {
        sift(bytes, stride, parent, count);
    }
    for end in (1..count).rev() {
        swap(bytes, stride, 0, end);
        sift(bytes, stride, 0, end);
    }
}
