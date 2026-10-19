//! The GPU core's display (ARM64: Parallels with the core, and QEMU's
//! `virtio-gpu-gl-pci`): the console writes the guest backing of the kernel's
//! display-sized scanout target, and the core uploads the rows written, binds
//! the target to scanout 0 when another frame is shown, and flushes them
//! (`gpu::screen`). Device waits block the caller, so only the render thread
//! draws here (`Display::boot_drawable`).
//!
//! A read-back reads that backing after the core presented the whole screen,
//! required the target to be the completed binding of scanout 0, poisoned the
//! backing and transferred the target back from the device (`settle`).

use super::super::raster::{Font, Layout};
use super::{copy_bands, Source};
use crate::gpu::screen;
use core::ops::Range;

pub struct GpuCore {
    layout: Layout,
}

impl GpuCore {
    /// The kernel's screen, on a boot whose display the GPU core owns.
    pub fn new() -> Option<Self> {
        let (width, height) = screen::open()?;
        let layout = Layout {
            width,
            height,
            bytes_per_pixel: 4,
            stride_bytes: width * 4,
            // B8G8R8A8: opaque.
            reserved: 0xff,
            bgr: true,
            font: Font::for_display(width, height),
        };
        Some(Self { layout })
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        copy_bands(&self.layout, shadow, lines, allowed, |bytes, from| screen::write(bytes.start, from))
    }

    pub fn flush(&self, lines: Range<usize>) -> Result<(), &'static str> {
        screen::present(lines).map(drop)
    }

    pub fn settle(&self) -> Result<Source, &'static str> {
        let (resource, flush) = screen::settle()?;
        Ok(Source::Device { resource, flush })
    }

    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        screen::read(self.layout.line(y).start, out);
    }
}
