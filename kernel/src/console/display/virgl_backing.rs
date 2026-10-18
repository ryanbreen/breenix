//! Parallels with VirGL: the kernel's screens draw into the guest backing of
//! the resource the virtio-gpu PCI driver scans out (the VirGL 3D resource once
//! VirGL holds the scanout, the 2D resource before) and present it through the
//! driver, as the boot screen does, until userspace takes the display. Which
//! backing that is can change while the console runs, so it is asked for at
//! each write.
//!
//! A read-back reads that backing after a present of the whole screen
//! completed, and only when it belongs to the resource the device last
//! accepted for scanout 0 (`settle`).

use super::super::raster::{Font, Layout};
use super::{phys_of, write_bands, Source};
use crate::drivers::virtio::gpu_pci;
use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

pub struct VirglBacking {
    layout: Layout,
    /// Presents of the console's rows the driver completed.
    presents: AtomicU64,
}

impl VirglBacking {
    pub fn new() -> Option<Self> {
        let (width, height) = gpu_pci::dimensions()?;
        let (width, height) = (width as usize, height as usize);
        let layout = Layout {
            width,
            height,
            bytes_per_pixel: 4,
            stride_bytes: width * 4,
            reserved: 0xff,
            font: Font::for_display(width, height),
        };
        Some(Self { layout, presents: AtomicU64::new(0) })
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The backing the display scans out, when it holds the whole screen.
    fn backing(&self) -> Option<*mut u8> {
        gpu_pci::framebuffer()
            .filter(|backing| backing.len() >= self.layout.bytes())
            .map(|backing| backing.as_mut_ptr())
    }

    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        let Some(backing) = self.backing() else {
            return false;
        };
        // SAFETY: the backing holds at least `layout.bytes()` bytes (`backing`).
        unsafe { write_bands(backing, &self.layout, shadow, lines, allowed) }
    }

    /// Present pixel rows `lines`; the driver returns once the device completed
    /// the present. Returns its number among the console's completed presents.
    fn present(&self, lines: Range<usize>) -> Result<u64, &'static str> {
        gpu_pci::flush_rect(0, lines.start as u32, self.layout.width as u32, lines.len() as u32)?;
        Ok(self.presents.fetch_add(1, Ordering::AcqRel) + 1)
    }

    pub fn flush(&self, lines: Range<usize>) -> Result<(), &'static str> {
        if lines.is_empty() {
            return Ok(());
        }
        self.present(lines).map(drop)
    }

    pub fn settle(&self) -> Result<Source, &'static str> {
        let flush = self.present(0..self.layout.height)?;
        let backing = self.backing().ok_or("no backing holds the screen")?;
        let (resource, bound, len) = gpu_pci::scanout_binding().ok_or("the device has accepted no scanout")?;
        if bound != backing as *const u8 || len < self.layout.bytes() {
            return Err("the backing read is not the bound resource's backing");
        }
        Ok(Source::Backing { resource, phys: phys_of(backing), flush })
    }

    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        if let Some(backing) = self.backing() {
            // SAFETY: row `y` of a backing that holds `layout.bytes()` bytes.
            unsafe { core::ptr::copy_nonoverlapping(backing.add(self.layout.line(y).start), out.as_mut_ptr(), out.len()) };
        }
    }
}
