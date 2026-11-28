use super::{build, intersect, Emit, RangeError, RangeSet, RangeStorage};
use arm64_boot_contract::{
    hw_config::{accepts_version, HardwareConfigV7, UefiMapHandoff},
    phys_memory::{direct_map_ram_window, fdt, kernel_heap_size, KernelPools, PhysRange, KERNEL_POOL_ALIGN, PAGE_SIZE},
    uefi_map::{self, Descriptor, MetaError, BOOT_MAP_MEMORY_TYPE, CONVENTIONAL},
};
use core::ptr::NonNull;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    NoTranslation,
    ReadOnly,
    WrongPhysicalAddress,
    WrongMemoryType,
}

/// # Safety
/// After successful identity probes, the returned pointer grants exclusive
/// byte access to the firmware map until the caller releases it.
pub unsafe trait BootMemory: RangeStorage {
    fn probe_identity_write(&self, page: u64) -> Result<(), ProbeError>;
    unsafe fn map_buffer(&self, range: PhysRange) -> NonNull<u8>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    Version(u32),
    Metadata {
        loader_rule: u32,
        kernel_rule: Option<MetaError>,
    },
    Translation(u64, ProbeError),
    UnownedBuffer,
    Ranges(RangeError),
    DeviceTree(fdt::FdtError),
}
impl From<RangeError> for MapError {
    fn from(error: RangeError) -> Self {
        Self::Ranges(error)
    }
}
impl From<fdt::FdtError> for MapError {
    fn from(error: fdt::FdtError) -> Self {
        Self::DeviceTree(error)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MapStats {
    pub descriptors: usize,
    pub stride: usize,
    pub usable_bytes: u64,
    pub malformed: usize,
    pub overlap_bytes: u64,
    pub excluded_attributes: u64,
    pub buffer_bytes: u64,
    pub buffer_overlap: u64,
    pub buffer_attributes: u64,
    pub buffer_fixed: u64,
    pub buffer_window: u64,
    pub other_map_allocations: usize,
    pub released_bytes: u64,
    pub excluded_pa_bytes: u64,
    pub excluded_va_bytes: u64,
}

/// RAM eligible for the pool and its smaller, already writable storage window.
#[derive(Clone, Copy)]
pub struct RamWindow {
    mapped: PhysRange,
    pa_limit: u64,
    va_limit: u64,
}

impl RamWindow {
    pub fn new(ram_base_offset: u64, pa_limit: u64, va_limit: u64) -> Self {
        let mut mapped = direct_map_ram_window(ram_base_offset);
        mapped.end = mapped.end.min(pa_limit).min(va_limit);
        Self { mapped, pa_limit, va_limit }
    }

    fn filter(self, range: PhysRange, stats: &mut MapStats) -> PhysRange {
        let pa = intersect(range, PhysRange::new(self.mapped.start, self.pa_limit));
        let va = intersect(pa, PhysRange::new(self.mapped.start, self.va_limit));
        stats.excluded_pa_bytes = stats.excluded_pa_bytes.saturating_add(
            range.end.saturating_sub(range.start.max(self.pa_limit)),
        );
        stats.excluded_va_bytes = stats.excluded_va_bytes.saturating_add(pa.len() - va.len());
        va
    }
}

fn fragments<E>(
    range: PhysRange,
    mut holes: impl FnMut(PhysRange) -> Result<Option<PhysRange>, E>,
    mut keep: impl FnMut(PhysRange) -> Result<(), E>,
) -> Result<(), E> {
    let mut remaining = range;
    while !remaining.is_empty() {
        let Some(hole) = holes(remaining)? else {
            return keep(remaining);
        };
        if remaining.start < hole.start {
            keep(PhysRange::new(remaining.start, hole.start))?;
        }
        remaining.start = hole.end;
    }
    Ok(())
}

fn first_hole(
    remaining: PhysRange,
    holes: impl IntoIterator<Item = PhysRange>,
) -> Option<PhysRange> {
    holes
        .into_iter()
        .map(|range| intersect(remaining, range))
        .filter(|range| !range.is_empty())
        .min_by_key(|range| range.start)
}

pub fn from_fdt(
    memory: &impl RangeStorage,
    blob: &[u8],
    blob_phys: u64,
    fixed: &[PhysRange],
    window: RamWindow,
) -> Result<(RangeSet, MapStats), MapError> {
    let blob_range = PhysRange::from_base_size(blob_phys, blob.len() as u64);
    let mut stats = MapStats::default();
    let set = build(memory, window.mapped, |emit| {
        stats = MapStats::default();
        let mut result: Result<(), MapError> = Ok(());
        fdt::walk(blob, |kind, range| {
            if kind != fdt::RegionKind::Memory || result.is_err() {
                return;
            }
            result = fragments(
                super::align_in(range),
                |remaining| {
                    let mut first =
                        first_hole(remaining, fixed.iter().copied().chain([blob_range]));
                    fdt::walk(blob, |kind, reservation| {
                        if kind == fdt::RegionKind::Reserved {
                            first = first_hole(remaining, first.into_iter().chain([reservation]));
                        }
                    })?;
                    Ok(first)
                },
                |range| {
                    let range = window.filter(super::align_in(range), &mut stats);
                    Ok(emit(range, false)?)
                },
            );
        })?;
        result
    })?;
    stats.usable_bytes = set.total_bytes();
    Ok((set, stats))
}

pub fn from_hardware_config(
    config: &HardwareConfigV7,
    map: Option<UefiMapHandoff>,
    memory: &impl BootMemory,
    window: RamWindow,
    fixed: &[PhysRange],
) -> Result<(RangeSet, MapStats), MapError> {
    if !accepts_version(config.version) {
        return Err(MapError::Version(config.version));
    }
    if config.version == 7 {
        let count = (config.ram_region_count as usize).min(config.ram_regions.len());
        let runtime_count =
            (config.runtime_region_count as usize).min(config.runtime_regions.len());
        let holes = || {
            fixed.iter().copied().chain(
                config.runtime_regions[..runtime_count]
                    .iter()
                    .map(|r| PhysRange::from_base_size(r.base, r.pages.saturating_mul(PAGE_SIZE))),
            )
        };
        let mut stats = MapStats::default();
        let set = build(memory, window.mapped, |emit| {
            stats = MapStats::default();
            for region in &config.ram_regions[..count] {
                fragments(
                    super::align_in(PhysRange::from_base_size(region.base, region.size)),
                    |remaining| Ok(first_hole(remaining, holes())),
                    |range| emit(window.filter(super::align_in(range), &mut stats), false),
                )?;
            }
            Ok::<(), RangeError>(())
        })?;
        stats.usable_bytes = set.total_bytes();
        return Ok((set, stats));
    }
    let wire = map.ok_or(MapError::Version(config.version))?;
    let address = uefi_map::check_address(wire);
    let stride = wire.descriptor_size as usize;
    let kernel_rule = address.as_ref().err().copied().or_else(|| {
        uefi_map::check_meta(
            wire.descriptor_version,
            stride,
            wire.map_size,
            wire.map_size,
        )
        .err()
    });
    if wire.loader_check != 0 || kernel_rule.is_some() {
        return Err(MapError::Metadata {
            loader_rule: wire.loader_check,
            kernel_rule,
        });
    }
    let address = address.unwrap();
    let mut page = address.start & !(PAGE_SIZE - 1);
    while page < address.end {
        memory
            .probe_identity_write(page)
            .map_err(|error| MapError::Translation(page, error))?;
        let Some(next) = page.checked_add(PAGE_SIZE) else {
            break;
        };
        page = next;
    }
    // This reference stays live only while building the set. The firmware
    // buffer is never a candidate for range storage while this runs.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(memory.map_buffer(address).as_ptr(), address.len() as usize)
    };
    uefi_map::sort(bytes, stride);
    let records = || bytes.chunks_exact(stride).map(Descriptor::read);
    let owner = records()
        .position(|d| {
            d.kind == BOOT_MAP_MEMORY_TYPE
                && d.extent()
                    .is_some_and(|r| r.start <= address.start && r.end >= address.end)
        })
        .ok_or(MapError::UnownedBuffer)?;
    let mut stats = MapStats {
        descriptors: bytes.len() / stride,
        stride,
        ..MapStats::default()
    };
    let set = build(memory, window.mapped, |emit| {
        // Only the second pass's snapshot is retained.
        stats = MapStats {
            descriptors: bytes.len() / stride,
            stride,
            ..MapStats::default()
        };
        sweep(records(), owner, window, fixed, &mut stats, emit)
    })?;
    Ok((set, stats))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Reserved,
    Conventional,
    Buffer,
    BufferHeld,
}

fn sweep(
    records: impl Iterator<Item = Descriptor>,
    owner: usize,
    window: RamWindow,
    fixed: &[PhysRange],
    stats: &mut MapStats,
    emit: &mut Emit<'_>,
) -> Result<(), MapError> {
    // At any instant, the two greatest active ends are enough to determine
    // what remains multiply covered and which descriptor survives afterwards.
    let mut first = (0, Use::Reserved);
    let mut second = 0;
    let mut cursor = 0;
    let mut flush = |cursor: &mut u64,
                     next: u64,
                     first: (u64, Use),
                     second: u64,
                     stats: &mut MapStats|
     -> Result<(), RangeError> {
        let overlap_end = next.min(second);
        if *cursor < overlap_end {
            stats.overlap_bytes += overlap_end - *cursor;
            *cursor = overlap_end;
        }
        let unique = PhysRange::new(*cursor, next.min(first.0));
        *cursor = next;
        if unique.is_empty() || matches!(first.1, Use::Reserved | Use::BufferHeld) {
            return Ok(());
        }
        let mut available = 0;
        let mut kept = 0;
        fragments(
            unique,
            |r| Ok(first_hole(r, fixed.iter().copied())),
            |r| {
                available += r.len();
                let r = window.filter(r, stats);
                kept += r.len();
                emit(r, first.1 == Use::Buffer)
            },
        )?;
        if first.1 == Use::Buffer {
            stats.buffer_fixed += unique.len() - available;
            stats.buffer_window += available - kept;
            stats.released_bytes += kept;
        } else {
            stats.usable_bytes += kept;
        }
        Ok(())
    };

    for (index, descriptor) in records.enumerate() {
        let extent = descriptor.extent();
        stats.malformed += usize::from(extent.is_none());
        let range = extent.unwrap_or_else(|| descriptor.reservation());
        if range.is_empty() {
            continue;
        }
        // Rounding malformed start addresses down preserves sorted order.
        flush(&mut cursor, range.start, first, second, stats)?;
        let use_for = if index == owner {
            stats.buffer_bytes = range.len();
            if descriptor.usable_attributes() {
                Use::Buffer
            } else {
                stats.buffer_attributes = range.len();
                Use::BufferHeld
            }
        } else if descriptor.kind == CONVENTIONAL && extent.is_some() {
            if descriptor.usable_attributes() {
                Use::Conventional
            } else {
                stats.excluded_attributes = stats.excluded_attributes.saturating_add(range.len());
                Use::Reserved
            }
        } else {
            stats.other_map_allocations += usize::from(descriptor.kind == BOOT_MAP_MEMORY_TYPE);
            Use::Reserved
        };
        if range.end > first.0 {
            second = first.0;
            first = (range.end, use_for);
        } else {
            second = second.max(range.end);
        }
    }
    flush(&mut cursor, u64::MAX, first, second, stats)?;
    stats.buffer_overlap = stats.buffer_bytes.saturating_sub(
        stats.released_bytes + stats.buffer_fixed + stats.buffer_window + stats.buffer_attributes,
    );
    Ok(())
}

pub fn place_kernel_pools(
    ranges: &mut RangeSet,
    stack_bytes: u64,
    below: u64,
) -> Result<KernelPools, RangeError> {
    let heap = ranges.carve(
        kernel_heap_size(ranges.iter().map(|range| {
            range.end.min(below).saturating_sub(range.start)
        }).sum()),
        KERNEL_POOL_ALIGN,
        below,
    )?;
    let stacks = ranges.carve(stack_bytes, KERNEL_POOL_ALIGN, below)?;
    Ok(KernelPools { heap, stacks })
}
