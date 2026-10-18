//! ARM64 QEMU's `virtio-gpu-device`: the console writes the guest backing of
//! the 2D resource, then has the device copy the rows written into the host
//! resource (TRANSFER_TO_HOST_2D) and show them (RESOURCE_FLUSH).
//!
//! The scanout is a host resource the guest cannot read, so a read-back reads
//! the backing, after a transfer and flush of the whole screen completed, and
//! only when the device's last accepted SET_SCANOUT names the resource whose
//! backing is read (`settle`).

use super::super::raster::{Font, Layout};
use super::{phys_of, write_bands, Source};
use crate::drivers::virtio::gpu_mmio;
use core::ops::Range;

pub struct Virtio2d {
    backing: *mut u8,
    layout: Layout,
}

// SAFETY: the backing is a static buffer of the driver's, written only under
// the screen hold.
unsafe impl Send for Virtio2d {}
unsafe impl Sync for Virtio2d {}

impl Virtio2d {
    /// The device's 2D resource, once the driver has set it up.
    pub fn new() -> Option<Self> {
        let (width, height) = gpu_mmio::dimensions()?;
        let backing = gpu_mmio::framebuffer()?;
        let (width, height) = (width as usize, height as usize);
        let layout = Layout {
            width,
            height,
            bytes_per_pixel: 4,
            stride_bytes: width * 4,
            // B8G8R8A8: opaque.
            reserved: 0xff,
            font: Font::for_display(width, height),
        };
        (layout.bytes() <= backing.len()).then(|| Self { backing: backing.as_mut_ptr(), layout })
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        // SAFETY: the backing holds at least `layout.bytes()` bytes (`new`).
        unsafe { write_bands(self.backing, &self.layout, shadow, lines, allowed) }
    }

    pub fn flush(&self, lines: Range<usize>) -> Result<(), &'static str> {
        gpu_mmio::flush_rect(0, lines.start as u32, self.layout.width as u32, lines.len() as u32)
    }

    pub fn settle(&self) -> Result<Source, &'static str> {
        let flushed = gpu_mmio::flush_rect_completed(0, 0, self.layout.width as u32, self.layout.height as u32)?;
        let binding = gpu_mmio::scanout_binding().ok_or("the device has accepted no scanout with a backing")?;
        if binding.resource_id != flushed.resource_id {
            return Err("the flush went to a resource other than the one bound to scanout 0");
        }
        let phys = phys_of(self.backing);
        if binding.backing_phys != phys || binding.backing_len < self.layout.bytes() {
            return Err("the backing read is not the bound resource's backing");
        }
        Ok(Source::Backing { resource: binding.resource_id, phys, flush: flushed.number })
    }

    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        // SAFETY: row `y` of the backing, which holds `layout.bytes()` bytes.
        unsafe { core::ptr::copy_nonoverlapping(self.backing.add(self.layout.line(y).start), out.as_mut_ptr(), out.len()) };
    }
}
