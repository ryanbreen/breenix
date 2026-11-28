//! The DMA bus-address check (E1 plan, 5.6), with no kernel dependencies, so
//! `tools/kmem-host-tests` compiles this file as is; `memory/dma.rs` is the
//! kernel side.
//!
//! A device reads and writes physical memory in one run from the address it
//! is given, so a buffer handed to it must be one physically contiguous run.
//! The direct map and the kernel image can hold such runs. A span or vmap
//! address cannot be assumed to: those windows map order-0 frames one page
//! at a time (6.4, 6.5). The check translates each page the range touches
//! through the CPU's own translation and requires each to follow the one
//! before it, so a hole, an alias or a relocated window in the range is
//! refused at the call rather than written to by the device.

pub const PAGE: u64 = 4096;

/// What a kernel VA lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// The direct map of physical memory.
    DirectMap,
    /// The kernel image's own statics.
    Image,
    /// A kernel VA window (`kva_window::WindowId` as its number).
    Window { window: u8 },
    /// Anything else.
    Other,
}

/// Why a range has no bus address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaError {
    /// The range is empty or runs past the end of the address space.
    BadRange,
    /// The range lies in a kernel VA window, whose pages are not one
    /// physical run: allocate the buffer with `dma::alloc` (E1-8) instead.
    Window { window: u8 },
    /// The range is neither direct-map nor kernel-image memory, or leaves
    /// the region it starts in.
    NotDmaMemory,
    /// The page at `offset` bytes into the range does not translate.
    Unmapped { offset: u64 },
    /// The page at `offset` bytes into the range does not follow the one
    /// before it in the device's address space.
    Discontiguous { offset: u64 },
}

/// The kernel image's memory as its linker symbols bound it: the loaded
/// image `[start, loaded_end)` and a section the linker places apart from
/// it, `[dma_start, dma_end)`, with `end` (`_end`) past both. On ARM64 that
/// section is `.dma`, at physical `0x5000_0000` past a gap (`linker.ld`);
/// x86 has none, and passes an empty one at `end`.
#[derive(Clone, Copy, Debug)]
pub struct ImageLayout {
    pub start: u64,
    pub loaded_end: u64,
    pub dma_start: u64,
    pub dma_end: u64,
    pub end: u64,
}

/// The VA ranges `[start, end)` the check counts as kernel-image memory:
/// the loaded image and the separate section, each on its own, so the gap
/// between them (on ARM64 the loader's boot stack and unused RAM, which the
/// boot direct map translates) is not image memory.
pub fn image_ranges(image: ImageLayout) -> [(u64, u64); 2] {
    [(image.start, image.loaded_end), (image.dma_start, image.dma_end)]
}

/// Whether `va` lies in one of `ranges`.
pub fn in_ranges(ranges: &[(u64, u64)], va: u64) -> bool {
    ranges.iter().any(|&(start, end)| va >= start && va < end)
}

/// The kernel's address space as the check reads it.
pub trait AddressSpace {
    /// The region `va` lies in.
    fn region(&self, va: u64) -> Region;
    /// The device-visible address the CPU's translation of `va` gives: the
    /// physical address, or the IPA in a virtual machine.
    fn bus(&self, va: u64) -> Option<u64>;
}

/// The device address of `[virt, virt + len)`, which must be one contiguous
/// run of direct-map or kernel-image memory.
pub fn bus_address(space: &impl AddressSpace, virt: u64, len: u64) -> Result<u64, DmaError> {
    let last = len.checked_sub(1).and_then(|tail| virt.checked_add(tail)).ok_or(DmaError::BadRange)?;
    let region = space.region(virt);
    match region {
        Region::Window { window } => return Err(DmaError::Window { window }),
        Region::Other => return Err(DmaError::NotDmaMemory),
        Region::DirectMap | Region::Image => {}
    }
    let first = space.bus(virt).ok_or(DmaError::Unmapped { offset: 0 })?;
    let mut page = (virt & !(PAGE - 1)).checked_add(PAGE);
    while let Some(va) = page.filter(|&va| va <= last) {
        let offset = va - virt;
        match space.region(va) {
            Region::Window { window } => return Err(DmaError::Window { window }),
            found if found != region => return Err(DmaError::NotDmaMemory),
            _ => {}
        }
        let bus = space.bus(va).ok_or(DmaError::Unmapped { offset })?;
        if Some(bus) != first.checked_add(offset) {
            return Err(DmaError::Discontiguous { offset });
        }
        page = va.checked_add(PAGE);
    }
    Ok(first)
}
