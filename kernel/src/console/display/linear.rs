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
#[cfg(target_arch = "x86_64")]
use core::sync::atomic::{AtomicBool, Ordering};

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
    #[cfg(target_arch = "x86_64")]
    native: OnceCell<crate::gk208::ConsoleSurface>,
    #[cfg(target_arch = "x86_64")]
    disabled: AtomicBool,
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
            #[cfg(target_arch = "x86_64")]
            native: OnceCell::uninit(),
            #[cfg(target_arch = "x86_64")]
            disabled: AtomicBool::new(false),
        })
    }

    /// Record the framebuffer's physical address and where it is.
    pub fn locate(&self, phys: u64, place: Place) {
        let _ = self.located.try_init_once(|| (phys, place));
    }

    pub fn layout(&self) -> Layout {
        #[cfg(target_arch = "x86_64")]
        if let Some(s)=self.native.get() {
            return Layout {width:s.width,height:s.height,stride_bytes:s.pitch,bytes_per_pixel:4,
                reserved:0,bgr:true,font:super::super::raster::Font::for_display(s.width,s.height)};
        }
        self.layout
    }
    fn buffer(&self)->*mut u8 {
        #[cfg(target_arch = "x86_64")]
        if let Some(s)=self.native.get() {return s.pointer as *mut u8;}
        self.buffer
    }
    #[cfg(target_arch = "x86_64")]
    fn length(&self)->usize {self.native.get().map_or(self.len,|s|s.bytes)}
    /// Called once, on the boot CPU, before prepare allocates the console shadow.
    #[cfg(target_arch = "x86_64")]
    pub fn install_native(&self,surface:crate::gk208::ConsoleSurface)->Result<(),&'static str> {
        self.native.try_init_once(||surface).map_err(|_|"native console already attached")
    }

    /// A failed native takeover may have replaced BAR1 or the active surface.
    /// Disable framebuffer access instead of reusing that inherited mapping.
    #[cfg(target_arch = "x86_64")]
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
    }
    pub fn available(&self) -> bool {
        #[cfg(target_arch = "x86_64")]
        { !self.disabled.load(Ordering::Acquire) }
        #[cfg(target_arch = "aarch64")]
        { true }
    }

    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        if !self.available() { return false; }
        // SAFETY: the mapping holds at least `layout.bytes()` bytes (`new`).
        unsafe { write_bands(self.buffer(), &self.layout(), shadow, lines, allowed) }
    }

    pub fn settle(&self) -> Result<Source, &'static str> {
        if !self.available() { return Err("display disabled after native takeover failure"); }
        drain_writes();
        let &(phys, place) = self.located.get().ok_or("the framebuffer's physical address is not known")?;
        #[cfg(target_arch = "x86_64")]
        if let Some(s)=self.native.get() {
            crate::gk208::console_binding(s)?;
            let writes=crate::firmware_writes::count().checked_sub(s.firmware_baseline).ok_or("firmware write counter reversed")?;
            if writes!=0 {return Err("firmware writes after native handoff");}
            crate::serial_println!("GT710_NATIVE_CONSOLE surface={} firmware_writes={}",s.vram,writes);
            return Ok(Source::Scanout {phys:s.phys,place});
        }
        Ok(Source::Scanout { phys, place })
    }

    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        if !self.available() { out.fill(0); return; }
        let start = self.layout().line(y).start;
        // The widest read the mapping allows: each read of video memory is a
        // bus transaction of its own. On x86 a whole 64-byte line where
        // aligned (`read_64`), else eight bytes where aligned.
        #[cfg(target_arch = "x86_64")]
        let lines = streaming_loads();
        let mut index = 0;
        while index < out.len() {
            // SAFETY: inside the framebuffer mapping; the wide reads are aligned.
            unsafe {
                let source = self.buffer().add(start + index);
                #[cfg(target_arch = "x86_64")]
                if lines && index + 64 <= out.len() && source as usize % 64 == 0 {
                    read_64(source, out[index..index + 64].as_mut_ptr());
                    index += 64;
                    continue;
                }
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
        if !self.available() { return false; }
        let layout = self.layout();
        let bpp = layout.bytes_per_pixel;
        let columns = columns.start.min(layout.width)..columns.end.min(layout.width);
        let len = columns.len() * bpp;
        // One row at a time with interrupts masked, so no holder of the screen
        // is ever switched out and a whole-screen flush does not defer a timer
        // or wake IPI for the length of the copy.
        for y in rows.start..rows.end.min(layout.height) {
            let target = y * layout.stride_bytes + columns.start * bpp;
            let copied = holding_screen(|| {
                if len != 0 && target + len <= self.length() {
                    crate::firmware_writes::note_range(unsafe {self.buffer().add(target)},len);
                    // SAFETY: the caller's pane holds this row; the target is
                    // inside the framebuffer mapping.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            pane.add(y * pane_stride + columns.start * bpp),
                            self.buffer().add(target),
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
        if !self.available() { return None; }
        let layout = self.layout();
        if x >= layout.width || y >= layout.height {
            return None;
        }
        let offset = y * layout.stride_bytes + x * layout.bytes_per_pixel;
        if offset.checked_add(3)? > self.length() {
            return None;
        }
        let mut pixel = [0; 3];
        let held = holding_screen(|| {
            // Drain write-combining stores before reading the device memory.
            drain_writes();
            // SAFETY: inside the framebuffer mapping (checked above).
            unsafe {
                let source = self.buffer().add(offset);
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
        let last = mapper.translate_addr(x86_64::VirtAddr::new(display.buffer as u64 + display.len as u64 - 1));
        match (first, last) {
            (Some(first), Some(last)) if last.as_u64() - first.as_u64() == display.len as u64 - 1 => first.as_u64(),
            _ => {
                log::warn!("[fbcon] no screen console: the framebuffer mapping is not physically contiguous");
                return None;
            }
        }
    };
    let end = phys + display.len as u64;
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
        "[fbcon] framebuffer in display BAR: {}x{} {:?} {} bytes/pixel stride {} at {:#x} length {}, {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}/{:02x} BAR{} {:#x}/{:#x}{}",
        layout.width,
        layout.height,
        pixel_format,
        layout.bytes_per_pixel,
        stride,
        phys,
        display.len,
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
    crate::drivers::device_class::display_initialized(format_args!(
        "pci {:02x}:{:02x}.{} [{:04x}:{:04x}] driver=firmware-framebuffer BAR{}",
        device.bus, device.device, device.function, device.vendor_id, device.device_id, index
    ));
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

/// Whether `read_64` may run: the CPU has SSE4.1 (CPUID.01H:ECX bit 19) and
/// this CPU has SSE enabled (CR4.OSFXSR, set by `cpu_init::init_fpu`).
#[cfg(target_arch = "x86_64")]
fn streaming_loads() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};
    use x86_64::registers::control::{Cr4, Cr4Flags};
    // 0 not yet asked, 1 present, 2 absent.
    static SSE41: AtomicU8 = AtomicU8::new(0);
    let present = match SSE41.load(Ordering::Relaxed) {
        0 => {
            // SAFETY: CPUID leaf 1 is architecturally present on x86_64.
            let ecx = unsafe { core::arch::x86_64::__cpuid(1) }.ecx;
            let present = ecx & (1 << 19) != 0;
            SSE41.store(if present { 1 } else { 2 }, Ordering::Relaxed);
            present
        }
        known => known == 1,
    };
    present && Cr4::read().contains(Cr4Flags::OSFXSR)
}

/// Read the 64 bytes at `source` into `out` with four SSE4.1 streaming loads
/// (MOVNTDQA, Intel SDM Vol. 2B). From write-combining memory the first load
/// fetches the whole line in one bus read and the other three are served from
/// that fetch, where ordinary loads make eight uncached reads. The kernel is
/// built without SSE: the registers used here are the running thread's, saved
/// and restored with it when it is switched out and in (`arch_impl::x86_64::fpu`).
///
/// # Safety
/// `source` must be 64-byte aligned and readable for 64 bytes, `out` writable
/// for 64 bytes, and `streaming_loads` true on this CPU.
#[cfg(target_arch = "x86_64")]
unsafe fn read_64(source: *const u8, out: *mut u8) {
    // SAFETY: the caller's contract; only xmm0-3 change, and they are declared.
    unsafe {
        core::arch::asm!(
            "movntdqa xmm0, [{source}]",
            "movntdqa xmm1, [{source} + 16]",
            "movntdqa xmm2, [{source} + 32]",
            "movntdqa xmm3, [{source} + 48]",
            "movdqu [{out}], xmm0",
            "movdqu [{out} + 16], xmm1",
            "movdqu [{out} + 32], xmm2",
            "movdqu [{out} + 48], xmm3",
            source = in(reg) source,
            out = in(reg) out,
            out("xmm0") _,
            out("xmm1") _,
            out("xmm2") _,
            out("xmm3") _,
            options(nostack, preserves_flags),
        );
    }
}

/// Retype the BAR aliases after adding the bounded native mapping.
#[cfg(target_arch = "x86_64")]
pub fn map_native_write_combining(start:u64,size:u64)->Result<(),&'static str> {
    map_write_combining(start,start+size,true).map(|_|())
}
