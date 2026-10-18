//! A linear framebuffer: memory the display controller scans out, written by
//! the CPU. The x86 firmware framebuffer, mapped write-combining (`attach`),
//! and the ARM64 GOP framebuffer on Parallels and VMware. Writes are drained
//! (`mfence`, `dsb sy`) before a band is released, and before memory is read
//! back; the controller shows them without a flush command.

use super::super::raster::Layout;
use super::{drain_writes, write_bands, Source};
#[cfg(target_arch = "x86_64")]
use super::holding_screen;
use conquer_once::spin::OnceCell;
use core::fmt;
use core::ops::Range;

/// Where the framebuffer is.
#[derive(Clone, Copy)]
pub enum Place {
    /// In a PCI display function's memory BAR.
    #[cfg(target_arch = "x86_64")]
    Bar { bus: u8, device: u8, function: u8, index: usize },
    /// The framebuffer the firmware's Graphics Output Protocol set up.
    #[cfg(target_arch = "aarch64")]
    Gop,
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(target_arch = "x86_64")]
            Place::Bar { bus, device, function, index } => {
                write!(f, "display BAR{} of {:02x}:{:02x}.{}", index, bus, device, function)
            }
            #[cfg(target_arch = "aarch64")]
            Place::Gop => f.write_str("the GOP framebuffer"),
        }
    }
}

pub struct Linear {
    buffer: *mut u8,
    /// The mapping's length; a display owner's flush is clipped to it.
    #[cfg(target_arch = "x86_64")]
    len: usize,
    layout: Layout,
    /// The framebuffer's physical address and where it is, once known.
    located: OnceCell<(u64, Place)>,
}

// SAFETY: the mapping lives for the kernel's lifetime, and its rows are written
// only under the screen hold (`holding_screen`).
unsafe impl Send for Linear {}
unsafe impl Sync for Linear {}

impl Linear {
    /// Fails when `layout` does not fit in `len` bytes.
    ///
    /// # Safety
    /// `buffer` must map `len` bytes of framebuffer for the kernel's lifetime.
    pub unsafe fn new(buffer: *mut u8, len: usize, layout: Layout) -> Option<Self> {
        (layout.bytes() <= len).then(|| Self {
            buffer,
            #[cfg(target_arch = "x86_64")]
            len,
            layout,
            located: OnceCell::uninit(),
        })
    }

    /// Record the framebuffer's physical address and where it is.
    pub fn locate(&self, phys: u64, place: Place) {
        let _ = self.located.try_init_once(|| (phys, place));
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        // SAFETY: the mapping holds at least `layout.bytes()` bytes (`new`).
        unsafe { write_bands(self.buffer, &self.layout, shadow, lines, allowed) }
    }

    pub fn settle(&self) -> Result<Source, &'static str> {
        drain_writes();
        let &(phys, place) = self.located.get().ok_or("the framebuffer's physical address is not known")?;
        Ok(Source::Scanout { phys, place })
    }

    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        let start = self.layout.line(y).start;
        // Eight bytes per read where aligned: each read of video memory is a
        // bus transaction of its own.
        let mut index = 0;
        while index < out.len() {
            // SAFETY: inside the framebuffer mapping; the wide read is aligned.
            unsafe {
                let source = self.buffer.add(start + index);
                if index + 8 <= out.len() && source as usize % 8 == 0 {
                    let word = core::ptr::read_volatile(source as *const u64);
                    out[index..index + 8].copy_from_slice(&word.to_le_bytes());
                    index += 8;
                } else {
                    out[index] = core::ptr::read_volatile(source);
                    index += 1;
                }
            }
        }
    }

    /// Copy rows `rows` and columns `columns` of a pane that starts at x 0 onto
    /// the framebuffer. `pane` is the pane's first byte and `pane_stride` its
    /// row length in bytes; its pixels are in the framebuffer's own format.
    /// Returns false when the render thread is writing a row (the caller
    /// reports EBUSY and the owner flushes again).
    ///
    /// # Safety
    /// `pane` must point at `rows.end` readable rows of `pane_stride` bytes,
    /// each at least `columns.end` pixels wide.
    #[cfg(target_arch = "x86_64")]
    pub unsafe fn copy_pane(
        &self,
        pane: *const u8,
        pane_stride: usize,
        rows: Range<usize>,
        columns: Range<usize>,
    ) -> bool {
        let layout = self.layout;
        let bpp = layout.bytes_per_pixel;
        let columns = columns.start.min(layout.width)..columns.end.min(layout.width);
        let len = columns.len() * bpp;
        // One row at a time with interrupts masked, so no holder of the screen
        // is ever switched out and a whole-screen flush does not defer a timer
        // or wake IPI for the length of the copy.
        for y in rows.start..rows.end.min(layout.height) {
            let target = y * layout.stride_bytes + columns.start * bpp;
            let copied = holding_screen(|| {
                if len != 0 && target + len <= self.len {
                    // SAFETY: the caller's pane holds this row; the target is
                    // inside the framebuffer mapping.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            pane.add(y * pane_stride + columns.start * bpp),
                            self.buffer.add(target),
                            len,
                        );
                    }
                }
            });
            if !copied {
                return false;
            }
        }
        true
    }

    /// One framebuffer pixel's three colour bytes in memory order, read from
    /// device memory. Absent outside the framebuffer or while a row is written.
    #[cfg(target_arch = "x86_64")]
    pub fn read_pixel(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let layout = self.layout;
        if x >= layout.width || y >= layout.height {
            return None;
        }
        let offset = y * layout.stride_bytes + x * layout.bytes_per_pixel;
        if offset.checked_add(3)? > self.len {
            return None;
        }
        let mut pixel = [0; 3];
        let held = holding_screen(|| {
            // Drain write-combining stores before reading the device memory.
            drain_writes();
            // SAFETY: inside the framebuffer mapping (checked above).
            unsafe {
                let source = self.buffer.add(offset);
                pixel = [source.read_volatile(), source.add(1).read_volatile(), source.add(2).read_volatile()];
            }
        });
        held.then_some(pixel)
    }
}

// ---------------------------------------------------------------------------
// x86: the display BAR and write-combining.
// ---------------------------------------------------------------------------

/// What `attach` found and did.
#[cfg(target_arch = "x86_64")]
pub struct Attached {
    pub write_combining: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>,
}

/// Find the PCI display function whose memory BAR holds the framebuffer, and
/// map that BAR write-combining wherever the kernel maps it. Runs on the boot
/// CPU after PCI enumeration and its per-CPU init (so after it programmed the
/// PAT) and before the other CPUs start, so no other TLB holds the old types.
#[cfg(target_arch = "x86_64")]
pub fn attach(display: &Linear, pixel_format: bootloader_api::info::PixelFormat, stride: usize) -> Option<Attached> {
    let layout = display.layout;
    let offset = crate::memory::physical_memory_offset();
    let phys = {
        use x86_64::structures::paging::Translate;
        // SAFETY: the active tables, read through the physical memory map.
        let mapper = unsafe { crate::memory::paging::get_mapper_with_offset(offset) };
        let first = mapper.translate_addr(x86_64::VirtAddr::new(display.buffer as u64));
        let last = mapper.translate_addr(x86_64::VirtAddr::new(display.buffer as u64 + layout.bytes() as u64 - 1));
        match (first, last) {
            (Some(first), Some(last)) if last.as_u64() - first.as_u64() == layout.bytes() as u64 - 1 => first.as_u64(),
            _ => {
                log::warn!("[fbcon] no screen console: the framebuffer mapping is not physically contiguous");
                return None;
            }
        }
    };
    let end = phys + layout.bytes() as u64;
    let bar = crate::drivers::pci::get_devices().and_then(|devices| {
        devices.into_iter().find_map(|device| {
            let index = device.bars.iter().position(|bar| {
                bar.is_valid() && !bar.is_io && bar.address <= phys && end <= bar.address + bar.size
            })?;
            (device.raw_class == 0x03).then_some((device, index))
        })
    });
    let Some((device, index)) = bar else {
        log::warn!("[fbcon] no screen console: framebuffer at {:#x} is in no display function's BAR", phys);
        return None;
    };
    let bar = device.bars[index];
    log::info!(
        "[fbcon] framebuffer in display BAR: {}x{} {:?} {} bytes/pixel stride {} at {:#x}, {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}/{:02x} BAR{} {:#x}/{:#x}{}",
        layout.width,
        layout.height,
        pixel_format,
        layout.bytes_per_pixel,
        stride,
        phys,
        device.bus,
        device.device,
        device.function,
        device.vendor_id,
        device.device_id,
        device.raw_class,
        device.subclass,
        index,
        bar.address,
        bar.size,
        if bar.prefetchable { " prefetchable" } else { "" }
    );
    display.locate(phys, Place::Bar { bus: device.bus, device: device.device, function: device.function, index });
    let write_combining = map_write_combining(bar.address, bar.address + bar.size, bar.prefetchable);
    Some(Attached { write_combining })
}

/// Page tables by physical address, through the physical memory map.
#[cfg(target_arch = "x86_64")]
struct KernelTables {
    offset: u64,
}

#[cfg(target_arch = "x86_64")]
impl crate::arch_impl::x86_64::pagewalk::Tables for KernelTables {
    fn read(&self, table: u64, index: usize) -> u64 {
        // SAFETY: `table` is a page table's frame, mapped by the physical map.
        unsafe { core::ptr::read_volatile(((self.offset + table) as *const u64).add(index)) }
    }

    fn write(&mut self, table: u64, index: usize, entry: u64) {
        // SAFETY: as `read`; the caller flushes the TLBs afterwards.
        unsafe { core::ptr::write_volatile(((self.offset + table) as *mut u64).add(index), entry) }
    }

    fn allocate(&mut self) -> Option<u64> {
        let frame = crate::memory::frame_allocator::allocate_frame()?;
        let phys = frame.start_address().as_u64();
        // SAFETY: a frame this walk now owns, mapped by the physical map.
        unsafe { core::ptr::write_bytes((self.offset + phys) as *mut u8, 0, 4096) };
        Some(phys)
    }
}

#[cfg(target_arch = "x86_64")]
fn map_write_combining(
    start: u64,
    end: u64,
    prefetchable: bool,
) -> Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str> {
    use crate::arch_impl::x86_64::{pagewalk, pat, smp};
    if !prefetchable {
        return Err("the BAR is not prefetchable");
    }
    if !pat::supported() {
        return Err("the processor has no PAT");
    }
    if pat::programmed_mask() & 1 == 0 {
        return Err("the boot CPU's PAT did not read back");
    }
    if smp::cpus_online() > 1 {
        return Err("other CPUs are already running");
    }
    let mut tables = KernelTables { offset: crate::memory::physical_memory_offset().as_u64() };
    let root = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    let report = crate::arch_without_interrupts(|| {
        pagewalk::set_write_combining(&mut tables, root, start & !0xfff, (end + 0xfff) & !0xfff)
    });
    pat::flush_after_type_change();
    report
}

/// Say whether the aperture ended up write-combining on each online CPU.
#[cfg(target_arch = "x86_64")]
pub fn report_write_combining(result: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>) {
    use crate::arch_impl::x86_64::{pat, smp};
    let online = smp::online_mask();
    let programmed = pat::programmed_mask() & online;
    match result {
        Ok(report) if programmed == online && report.mappings > 0 => log::info!(
            "[fbcon] aperture mapped write-combining: PAT entry {} on {} of {} CPUs; {} mappings, {} 4K, {} 2M, {} 1G pages, {} splits",
            pat::WRITE_COMBINING_ENTRY,
            programmed.count_ones(),
            online.count_ones(),
            report.mappings,
            report.pages_4k,
            report.pages_2m,
            report.pages_1g,
            report.splits
        ),
        Ok(report) => log::warn!(
            "[fbcon] aperture not write-combining: PAT layout on {} of {} CPUs, {} mappings",
            programmed.count_ones(),
            online.count_ones(),
            report.mappings
        ),
        Err(reason) => log::warn!("[fbcon] aperture not write-combining: {}", reason),
    }
}
