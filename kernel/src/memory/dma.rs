//! The DMA API (E1 plan, 5.6). E1-7 adds `bus_address`, the checked
//! translation that replaces the drivers' private `virt_to_phys` helpers;
//! E1-8 adds `dma::alloc` with address limits. The check is in
//! `dma_core.rs`, which the host tests include.

#[path = "dma_core.rs"]
mod core_impl;
pub use core_impl::DmaError;
use core_impl::{AddressSpace, Region};

/// The kernel's address space: the VA windows, the kernel image and the
/// direct map, translated by the CPU (`AT S1E1R` on ARM64, whose output is
/// the IPA a device sees in a virtual machine, and a table walk on x86).
struct KernelSpace;

/// The kernel image's VA range, from its linker symbols.
fn image() -> (u64, u64) {
    extern "C" {
        #[cfg(target_arch = "aarch64")]
        static __kernel_virt_start: u8;
        #[cfg(target_arch = "x86_64")]
        static __ehdr_start: u8;
        static _end: u8;
    }
    // Only the symbols' addresses are taken, which needs no `unsafe`.
    #[cfg(target_arch = "aarch64")]
    let start = core::ptr::addr_of!(__kernel_virt_start) as u64;
    #[cfg(target_arch = "x86_64")]
    let start = core::ptr::addr_of!(__ehdr_start) as u64;
    (start, core::ptr::addr_of!(_end) as u64)
}

impl AddressSpace for KernelSpace {
    fn region(&self, va: u64) -> Region {
        if let Some(id) = super::kva::window_of(va) {
            return Region::Window { window: id as u8 };
        }
        let (start, end) = image();
        if va >= start && va < end {
            return Region::Image;
        }
        // Direct-map RAM is the RAM the frame allocator tracks: each such
        // frame has a metadata entry (4.2).
        let ram = super::direct_map_offset()
            .and_then(|offset| va.checked_sub(offset))
            .map(|pa| super::PhysFrame::containing_address(super::PhysAddr::new(pa)))
            .and_then(super::frame_allocator::frame_entry);
        if ram.is_some() {
            Region::DirectMap
        } else {
            Region::Other
        }
    }

    fn bus(&self, va: u64) -> Option<u64> {
        super::PhysAddrWrapper::from_kernel_virt(va as usize).ok()
    }
}

/// The device address of `[virt, virt + len)`. It must be one physically
/// contiguous run in the direct map or the kernel image; a span or vmap
/// address, a range that leaves usable RAM, and a range whose pages do not
/// follow each other in the device's address space are refused.
pub fn bus_address(virt: *const u8, len: usize) -> Result<u64, DmaError> {
    core_impl::bus_address(&KernelSpace, virt as u64, len as u64)
}
