//! Owned coherent system pages. No page is exposed to an engine in M5.3.
use super::dma_policy;
use crate::memory::frame_allocator;
use alloc::vec::Vec;
use x86_64::structures::paging::PhysFrame;
static PLATFORM: spin::Once<(Option<u64>, u64)> = spin::Once::new();
pub fn set_platform(rsdp: Option<u64>, offset: u64) {
    PLATFORM.call_once(|| (rsdp, offset));
}
/// Observe firmware remapping state without changing it. Active translation is
/// refused: this driver has no platform IOVA mapping service yet.
pub fn translation_status() -> Result<Vec<u32>, &'static str> {
    let &(rsdp, offset) = PLATFORM.get().ok_or("DMA platform context unavailable")?;
    let table = crate::arch_impl::x86_64::acpi::dma_remapping_table(rsdp, offset)?;
    let mut statuses = Vec::new();
    if !table.is_empty() {
        for base in dma_policy::dmar_units(&table)? {
            let mapped = crate::memory::map_mmio(base, 4096)?;
            // SAFETY: UC mapping of the DRHD register page, GSTS at offset 1Ch.
            let status = unsafe { ((mapped + 0x1c) as *const u32).read_volatile() };
            if status == u32::MAX {
                return Err("unreadable VT-d global status");
            }
            statuses.push(status);
        }
    }
    dma_policy::identity_page(4096, &statuses)?;
    Ok(statuses)
}
pub struct Pages {
    frames: Vec<PhysFrame>,
}
impl Pages {
    pub fn new(count: usize, statuses: &[u32]) -> Result<Self, &'static str> {
        if count == 0 || count > 256 {
            return Err("bounded DMA page request required");
        }
        dma_policy::identity_page(4096, statuses)?;
        let mut pages = Self { frames: Vec::new() };
        pages
            .frames
            .try_reserve_exact(count)
            .map_err(|_| "DMA frame list allocation failed")?;
        for _ in 0..count {
            let frame = frame_allocator::allocate_frame().ok_or("no system DMA frame")?;
            let address = frame.start_address().as_u64();
            if let Err(e) = dma_policy::identity_page(address, statuses) {
                frame_allocator::deallocate_frame(frame);
                return Err(e);
            }
            pages.frames.push(frame);
            // SAFETY: the fresh WB direct-map frame belongs to this allocation.
            unsafe {
                core::ptr::write_bytes(pages.va(pages.frames.len() - 1) as *mut u8, 0, 4096);
            }
        }
        Ok(pages)
    }
    pub fn address(&self, index: usize) -> u64 {
        self.frames[index].start_address().as_u64()
    }
    pub fn va(&self, index: usize) -> u64 {
        crate::memory::physical_memory_offset().as_u64() + self.address(index)
    }
    pub fn publish(&self) {
        for i in 0..self.frames.len() {
            crate::gpu_cache::clean(self.va(i), 4096);
        }
    }
    pub fn observe(&self) {
        for i in 0..self.frames.len() {
            crate::gpu_cache::invalidate(self.va(i), 4096);
        }
    }
}
impl Drop for Pages {
    fn drop(&mut self) {
        for f in self.frames.drain(..) {
            frame_allocator::deallocate_frame(f);
        }
    }
}
