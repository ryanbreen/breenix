//! The screen console on the firmware's framebuffer (x86_64 kernels without
//! `interactive`).
//!
//! Kernel log records and the console TTY's output are written, as text, into
//! one retained character grid (`grid`). Writers take the grid's lock with
//! interrupts masked, for one record or one write, so they can come from
//! interrupt handlers as well as threads, and they do not touch the screen.
//!
//! The screen has a single writer, the `fbcon` kernel thread. It draws the rows
//! that changed since its last pass into a RAM shadow of the framebuffer and
//! copies only those rows' pixels into video memory, which the kernel maps
//! write-combining (`attach`). Drawing does not read video memory; the
//! read-back check (`/proc/fbcon/check`, and once at kernel idle) reads it and
//! compares it with the shadow and with pixels worked out from the font and the
//! text alone (`raster::expected_intensity`).
//!
//! The console starts in init's session. While it is there, or in no session,
//! kernel log records of INFO and above are drawn; while another session has
//! it as its controlling terminal (a getty and the login it starts), only
//! warnings and errors are, so the console's user is not shown the per-second
//! census lines. Serial keeps each record. Once a
//! process takes the display (`sys_take_over_display`) the console stops drawing.

pub mod grid;
pub mod pagewalk;
pub mod raster;

use alloc::string::String;
use alloc::vec::Vec;
use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use conquer_once::spin::OnceCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use grid::Grid;
use raster::Layout;
use spin::Mutex;

/// The bootloader's framebuffer: its mapping and layout.
struct Display {
    buffer: *mut u8,
    len: usize,
    info: FrameBufferInfo,
    /// Absent for a pixel format the console does not draw.
    layout: Option<Layout>,
}

// SAFETY: the mapping lives for the kernel's lifetime; only the render thread
// and the display owner's flush (one row at a time, under `VRAM_BUSY`) write it.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

static DISPLAY: OnceCell<Display> = OnceCell::uninit();
static GRID: Mutex<Grid> = Mutex::new(Grid::new());

/// Grid writes given up because its lock stayed held (a holder that faulted).
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// A session other than init's has the console as its controlling terminal.
static SESSION: AtomicBool = AtomicBool::new(false);
/// Last session state recorded by the render thread. Notices go to serial; filtering reads
/// `SESSION` directly so it takes effect before getty can print its prompt.
static QUIET: AtomicBool = AtomicBool::new(false);

/// A process has the display.
static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);
/// Held, with interrupts masked, by whoever is writing a row of video memory.
static VRAM_BUSY: AtomicBool = AtomicBool::new(false);

/// What `attach` set up, for the render thread.
struct Attached {
    shadow: Vec<u8>,
    write_combining: Result<pagewalk::Report, &'static str>,
}

static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The idle read-back has been asked for.
static IDLE_CHECK: AtomicBool = AtomicBool::new(false);
/// The token of the `/proc/fbcon/check` request waiting, or 0.
static PROC_CHECK: AtomicU32 = AtomicU32::new(0);
static PROC_TOKEN: AtomicU32 = AtomicU32::new(0);
static PROC_BUSY: AtomicBool = AtomicBool::new(false);
static PROC_DONE: crate::task::completion::Completion = crate::task::completion::Completion::new();
static PROC_RESULT: Mutex<Option<Check>> = Mutex::new(None);

/// How often the render thread looks at the grid.
const PASS_INTERVAL_MS: u64 = 20;

/// Attempts at the grid's lock before a write is dropped. Holders keep it for
/// one record with interrupts masked, a few microseconds; this is milliseconds.
const GRID_SPINS: usize = 100_000;

/// Record the bootloader's framebuffer and size the grid to it. Log records
/// are kept in the grid from here; the render thread draws them once it runs.
pub fn init_early(buffer: &'static mut [u8], info: FrameBufferInfo) {
    let layout = match (info.pixel_format, info.bytes_per_pixel) {
        (PixelFormat::Rgb | PixelFormat::Bgr, 3 | 4) => Some(Layout {
            width: info.width,
            height: info.height,
            bytes_per_pixel: info.bytes_per_pixel,
            stride_bytes: info.stride * info.bytes_per_pixel,
        }),
        _ => None,
    }
    .filter(|layout| layout.bytes() <= buffer.len() && layout.cols() > 0 && layout.rows() > 0);
    if let Some(layout) = layout {
        with_grid(|grid| grid.set_size(layout.cols(), layout.rows()));
    }
    let len = buffer.len();
    let buffer = buffer.as_mut_ptr();
    let _ = DISPLAY.get_or_init(|| Display { buffer, len, info, layout });
}

/// Run `f` on the grid with interrupts masked; no result when its lock stayed held.
fn with_grid<R>(f: impl FnOnce(&mut Grid) -> R) -> Option<R> {
    crate::arch_without_interrupts(|| {
        for _ in 0..GRID_SPINS {
            if let Some(mut grid) = GRID.try_lock() {
                return Some(f(&mut grid));
            }
            core::hint::spin_loop();
        }
        DROPPED.fetch_add(1, Ordering::Relaxed);
        None
    })
}

/// A log line formatted into fixed storage; longer lines are cut short.
struct Line {
    bytes: [u8; 256],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Put a kernel log record on the console, as its own line.
pub fn log_record(level: log::Level, args: &fmt::Arguments) {
    if DISPLAY_TAKEN.load(Ordering::Acquire)
        || (SESSION.load(Ordering::Acquire) && level > log::Level::Warn)
    {
        return;
    }
    let mut line = Line { bytes: [0; 256], len: 0 };
    let _ = write!(line, "{:>5}: {}", level, args);
    with_grid(|grid| {
        grid.begin_line();
        grid.write(&line.bytes[..line.len]);
        grid.write(b"\r\n");
    });
}

/// Put console TTY output on the console.
pub fn write_tty(bytes: &[u8]) {
    if !DISPLAY_TAKEN.load(Ordering::Acquire) {
        with_grid(|grid| grid.write(bytes));
    }
}

/// A session other than init's took the console as its controlling terminal
/// (`true`), or the console is back in init's session or out of sessions (`false`).
pub fn console_session(held: bool) {
    SESSION.store(held, Ordering::Release);
}

/// A process has the display: stop drawing on it.
pub fn take_display() {
    DISPLAY_TAKEN.store(true, Ordering::Release);
}

/// The framebuffer's layout: width, height and stride in pixels, bytes per
/// pixel, and whether pixels are BGR. No layout without a framebuffer, or for
/// one whose pixels are not 3- or 4-byte RGB or BGR.
pub fn display_geometry() -> Option<(usize, usize, usize, usize, bool)> {
    let display = DISPLAY.get()?;
    display.layout?;
    let info = display.info;
    Some((info.width, info.height, info.stride, info.bytes_per_pixel, info.pixel_format == PixelFormat::Bgr))
}

/// Copy rows `rows` and columns `columns` of a pane that starts at x 0 onto the
/// framebuffer. `pane` is the pane's first byte and `pane_stride` its row length
/// in bytes; its pixels are in the framebuffer's own format. Returns false when
/// the render thread is writing a row (the caller reports EBUSY and the owner
/// flushes again).
///
/// # Safety
/// `pane` must point at `rows.end` readable rows of `pane_stride` bytes, each at
/// least `columns.end` pixels wide.
pub unsafe fn copy_to_display(
    pane: *const u8,
    pane_stride: usize,
    rows: core::ops::Range<usize>,
    columns: core::ops::Range<usize>,
) -> bool {
    let Some(display) = DISPLAY.get() else {
        return false;
    };
    let info = display.info;
    let bpp = info.bytes_per_pixel;
    let row_bytes = info.stride * bpp;
    let columns = columns.start.min(info.width)..columns.end.min(info.width);
    let len = columns.len() * bpp;
    // One row at a time with interrupts masked, so no holder of `VRAM_BUSY` is
    // ever switched out and a whole-screen flush does not defer a timer or wake
    // IPI for the length of the copy.
    for y in rows.start..rows.end.min(info.height) {
        let copied = crate::arch_without_interrupts(|| {
            if VRAM_BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                return false;
            }
            let target = y * row_bytes + columns.start * bpp;
            if len != 0 && target + len <= display.len {
                // SAFETY: the caller's pane holds this row; the target is inside
                // the framebuffer mapping.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        pane.add(y * pane_stride + columns.start * bpp),
                        display.buffer.add(target),
                        len,
                    );
                }
            }
            VRAM_BUSY.store(false, Ordering::Release);
            true
        });
        if !copied {
            return false;
        }
    }
    true
}

/// Read one hardware framebuffer pixel as 0x00RRGGBB directly from device memory.
/// Invalid coordinates and busy/unavailable displays return an empty result.
pub fn read_display_pixel(x: usize, y: usize) -> Option<u32> {
    let display = DISPLAY.get()?;
    let info = display.info;
    if x >= info.width || y >= info.height {
        return None;
    }
    let offset = (y * info.stride + x) * info.bytes_per_pixel;
    if offset.checked_add(3)? > display.len {
        return None;
    }
    let (c0, c1, c2) = crate::arch_without_interrupts(|| {
        if VRAM_BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return None;
        }
        // Drain write-combining stores from Flush before reading the device memory.
        // SAFETY: inside the framebuffer mapping (checked above).
        let pixel = unsafe {
            core::arch::asm!("mfence", options(nostack, preserves_flags));
            let pixel = display.buffer.add(offset);
            (pixel.read_volatile(), pixel.add(1).read_volatile(), pixel.add(2).read_volatile())
        };
        VRAM_BUSY.store(false, Ordering::Release);
        Some(pixel)
    })?;
    let (r, b) = match info.pixel_format {
        PixelFormat::Rgb => (c0, c2),
        PixelFormat::Bgr => (c2, c0),
        _ => return None,
    };
    Some((u32::from(r) << 16) | (u32::from(c1) << 8) | u32::from(b))
}

// ---------------------------------------------------------------------------
// Attaching to the display: the display BAR, write-combining, the shadow.
// ---------------------------------------------------------------------------

/// Find the PCI display function whose memory BAR holds the framebuffer, map
/// that BAR write-combining wherever the kernel maps it, and allocate the
/// shadow. Runs on the boot CPU after PCI enumeration and its per-CPU init (so
/// after it programmed the PAT) and before the other CPUs start, so no other
/// TLB holds the old types.
pub fn attach() {
    let Some(display) = DISPLAY.get() else {
        return;
    };
    let Some(layout) = display.layout else {
        log::warn!(
            "[fbcon] no screen console: pixel format {:?}, {} bytes per pixel",
            display.info.pixel_format,
            display.info.bytes_per_pixel
        );
        return;
    };
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
                return;
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
        return;
    };
    let bar = device.bars[index];
    log::info!(
        "[fbcon] framebuffer in display BAR: {}x{} {:?} {} bytes/pixel stride {} at {:#x}, {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}/{:02x} BAR{} {:#x}/{:#x}{}",
        layout.width,
        layout.height,
        display.info.pixel_format,
        layout.bytes_per_pixel,
        display.info.stride,
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

    let write_combining = map_write_combining(bar.address, bar.address + bar.size, bar.prefetchable);
    let mut shadow = Vec::new();
    if shadow.try_reserve_exact(layout.bytes()).is_err() {
        log::warn!("[fbcon] no screen console: no memory for a {} byte shadow", layout.bytes());
        return;
    }
    shadow.resize(layout.bytes(), 0);
    *ATTACHED.lock() = Some(Attached { shadow, write_combining });
}

/// Page tables by physical address, through the physical memory map.
struct KernelTables {
    offset: u64,
}

impl pagewalk::Tables for KernelTables {
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

fn map_write_combining(start: u64, end: u64, prefetchable: bool) -> Result<pagewalk::Report, &'static str> {
    use crate::arch_impl::x86_64::{pat, smp};
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

// ---------------------------------------------------------------------------
// The render thread.
// ---------------------------------------------------------------------------

/// Start the render thread, if `attach` found the display.
pub fn start() {
    let Some(attached) = ATTACHED.lock().take() else {
        return;
    };
    let Some(display) = DISPLAY.get() else {
        return;
    };
    let Some(layout) = display.layout else {
        return;
    };
    match crate::task::kthread::kthread_run(move || render_thread(attached, layout), "fbcon") {
        Ok(_) => {
            // Keep the normal time-sharing policy: an idle-policy thread can
            // starve indefinitely behind a runnable graphics thread or users.
            // The render loop blocks between passes to bound its CPU demand.
            RUNNING.store(true, Ordering::Release);
        }
        Err(error) => log::error!("[fbcon] no render thread: {:?}", error),
    }
}

/// A text row as last drawn.
#[derive(Clone, PartialEq, Eq)]
struct Shown {
    text: Vec<u8>,
    cursor: Option<usize>,
}

struct Screen {
    layout: Layout,
    shadow: Vec<u8>,
    vram: *mut u8,
    /// What each text row of video memory shows, once it is drawn.
    shown: Vec<Option<Shown>>,
    cleared: bool,
}

/// The grid as the render thread last copied it.
struct Snapshot {
    cols: usize,
    rows: usize,
    cells: Vec<u8>,
    cursor: (usize, usize),
    generation: u64,
}

impl Snapshot {
    fn row(&self, row: usize) -> &[u8] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// Copy the grid if it changed since `generation`. False when it did not
    /// or its lock stayed held.
    fn refresh(&mut self) -> bool {
        let generation = self.generation;
        let cells = &mut self.cells;
        let copied = with_grid(|grid| {
            if grid.generation() == generation {
                return None;
            }
            for row in 0..grid.rows() {
                cells[row * grid.cols()..(row + 1) * grid.cols()].copy_from_slice(grid.row(row));
            }
            Some((grid.generation(), grid.cursor()))
        });
        match copied.flatten() {
            Some((generation, cursor)) => {
                self.generation = generation;
                self.cursor = cursor;
                true
            }
            None => false,
        }
    }
}

fn render_thread(attached: Attached, layout: Layout) {
    let Attached { shadow, write_combining } = attached;
    let (cols, rows) = with_grid(|grid| (grid.cols(), grid.rows())).unwrap_or((0, 0));
    let mut snapshot = Snapshot {
        cols,
        rows,
        cells: alloc::vec![grid::BLANK; cols * rows],
        cursor: (0, 0),
        generation: u64::MAX,
    };
    let vram = DISPLAY.get().map_or(core::ptr::null_mut(), |display| display.buffer);
    let mut screen = Screen { layout, shadow, vram, shown: alloc::vec![None; rows], cleared: false };

    report_write_combining(write_combining);
    let mut announced = false;
    while !crate::task::kthread::kthread_should_stop() {
        if DISPLAY_TAKEN.load(Ordering::Acquire) {
            answer_checks(None);
            sleep_ms(PASS_INTERVAL_MS * 10);
            continue;
        }
        if snapshot.refresh() {
            let started = crate::time::get_monotonic_time_ns();
            let drawn = screen.update(&snapshot);
            if !announced && screen.shown.iter().all(Option::is_some) {
                announced = true;
                let elapsed = elapsed_us(started);
                log::info!(
                    "[fbcon] console tty and kernel log on the screen: {}x{} cells, {} rows drawn in {}.{:03} ms",
                    cols,
                    rows,
                    drawn,
                    elapsed / 1000,
                    elapsed % 1000
                );
            }
        }
        let session = SESSION.load(Ordering::Acquire);
        if session != QUIET.load(Ordering::Acquire) {
            if session {
                log::info!("[fbcon] the console has a session: only kernel warnings and errors are drawn");
                QUIET.store(true, Ordering::Release);
            } else {
                QUIET.store(false, Ordering::Release);
                log::info!("[fbcon] the console's session ended: the kernel log is drawn again");
            }
        }
        if announced {
            answer_checks(Some(&screen));
        }
        sleep_ms(PASS_INTERVAL_MS);
    }
}

fn elapsed_us((seconds, nanos): (u64, u64)) -> u64 {
    let (now_seconds, now_nanos) = crate::time::get_monotonic_time_ns();
    let start = seconds * 1_000_000_000 + nanos;
    let now = now_seconds * 1_000_000_000 + now_nanos;
    now.saturating_sub(start) / 1000
}

fn report_write_combining(result: Result<pagewalk::Report, &'static str>) {
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

/// Block the render thread on the scheduler timer.
fn sleep_ms(ms: u64) {
    use crate::task::{scheduler, thread::ThreadState};
    let Some(tid) = scheduler::current_thread_id() else {
        return;
    };
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let deadline = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos)
        .saturating_add(ms.saturating_mul(1_000_000));
    scheduler::with_current(|sched| sched.block_current_for_timer(deadline));
    scheduler::yield_current();
    while scheduler::thread_state(tid) == Some(ThreadState::BlockedOnTimer) {
        crate::arch_halt_with_interrupts();
    }
}

impl Screen {
    /// Draw the rows that differ from what video memory shows. Returns how many
    /// rows were written.
    fn update(&mut self, snapshot: &Snapshot) -> usize {
        if !self.cleared {
            self.shadow.fill(0);
            self.cleared = self.copy_lines(0..self.layout.height);
            if !self.cleared {
                return 0;
            }
        }
        let mut drawn = 0;
        for row in 0..snapshot.rows {
            let (cursor_row, cursor_col) = snapshot.cursor;
            let wanted = Shown {
                text: snapshot.row(row).to_vec(),
                cursor: (cursor_row == row).then_some(cursor_col),
            };
            if self.shown[row].as_ref() == Some(&wanted) {
                continue;
            }
            raster::draw_row(&mut self.shadow, &self.layout, row, &wanted.text, wanted.cursor);
            if self.copy_lines(self.layout.band(row)) {
                self.shown[row] = Some(wanted);
                drawn += 1;
            } else {
                self.shown[row] = None;
            }
        }
        drawn
    }

    /// Copy pixel rows `lines` of the shadow into video memory, one text row's
    /// band at most at a time with interrupts masked. False when the display
    /// was taken or its owner was writing it.
    fn copy_lines(&self, lines: core::ops::Range<usize>) -> bool {
        let mut y = lines.start;
        while y < lines.end {
            let end = (y + raster::LINE_HEIGHT).min(lines.end);
            let copied = crate::arch_without_interrupts(|| {
                if DISPLAY_TAKEN.load(Ordering::Acquire)
                    || VRAM_BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
                {
                    return false;
                }
                let bytes = self.layout.line(y).start..self.layout.line(end - 1).end;
                // SAFETY: inside the framebuffer mapping, which holds
                // `layout.bytes()` bytes (`init_early`).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.shadow[bytes.clone()].as_ptr(),
                        self.vram.add(bytes.start),
                        bytes.len(),
                    );
                    core::arch::asm!("sfence", options(nostack, preserves_flags));
                }
                VRAM_BUSY.store(false, Ordering::Release);
                true
            });
            if !copied {
                return false;
            }
            y = end;
        }
        true
    }

    /// Read video memory back and compare it with the shadow, and with the
    /// pixels the font gives for the text each row was drawn with.
    fn check(&self) -> Check {
        let layout = self.layout;
        let bpp = layout.bytes_per_pixel;
        let mut line = alloc::vec![0u8; layout.width * bpp];
        let mut differing_lines = 0;
        let mut font_mismatches = 0;
        for y in 0..layout.height {
            let bytes = layout.line(y);
            // Eight bytes per read: each read of video memory is a bus
            // transaction of its own.
            let mut index = 0;
            while index < line.len() {
                // SAFETY: inside the framebuffer mapping; the wide read is aligned.
                unsafe {
                    let source = self.vram.add(bytes.start + index);
                    if index + 8 <= line.len() && source as usize % 8 == 0 {
                        let word = core::ptr::read_volatile(source as *const u64);
                        line[index..index + 8].copy_from_slice(&word.to_le_bytes());
                        index += 8;
                    } else {
                        line[index] = core::ptr::read_volatile(source);
                        index += 1;
                    }
                }
            }
            if line[..] != self.shadow[bytes] {
                differing_lines += 1;
            }
            let row = y / raster::LINE_HEIGHT;
            let shown = self.shown.get(row).and_then(Option::as_ref);
            for (x, pixel) in line.chunks_exact(bpp).enumerate() {
                let expected = match shown {
                    Some(shown) => raster::expected_intensity(&shown.text, shown.cursor, x, y % raster::LINE_HEIGHT),
                    None => 0,
                };
                if !raster::pixel_is(pixel, expected) {
                    font_mismatches += 1;
                }
            }
        }
        let shown: Vec<&Shown> = self.shown.iter().flatten().collect();
        let glyph_cells = shown
            .iter()
            .map(|row| row.text.iter().filter(|&&cell| cell != grid::BLANK).count())
            .sum();
        let rows = shown
            .iter()
            .map(|row| {
                let end = row.text.iter().rposition(|&cell| cell != grid::BLANK).map_or(0, |last| last + 1);
                row.text[..end].to_vec()
            })
            .filter(|text| !text.is_empty())
            .collect();
        let mut sha = crate::block::install::sha256::Sha256::new();
        sha.update(&self.shadow);
        Check {
            pixel_lines: layout.height,
            differing_lines,
            font_mismatches,
            glyph_cells,
            sha256: sha.finalize(),
            rows,
        }
    }
}

// ---------------------------------------------------------------------------
// The read-back check.
// ---------------------------------------------------------------------------

/// One read-back of the screen.
struct Check {
    pixel_lines: usize,
    /// Pixel rows of video memory that differ from the shadow.
    differing_lines: usize,
    /// Pixels of video memory that differ from what the font gives.
    font_mismatches: usize,
    glyph_cells: usize,
    sha256: [u8; 32],
    /// The text rows drawn, without trailing blanks, blank rows left out.
    rows: Vec<Vec<u8>>,
}

impl Check {
    fn identical(&self) -> bool {
        self.differing_lines == 0 && self.font_mismatches == 0
    }
}

struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{:02x}", byte))
    }
}

/// Answer the checks asked for: on the render thread, after a pass, so the
/// screen is what `shown` says it is. Without a screen once a process has the display.
fn answer_checks(screen: Option<&Screen>) {
    let idle = IDLE_CHECK.swap(false, Ordering::AcqRel);
    let token = PROC_CHECK.swap(0, Ordering::AcqRel);
    if !idle && token == 0 {
        return;
    }
    let check = screen.map(Screen::check);
    if idle {
        match &check {
            Some(check) if check.identical() => log::info!(
                "[fbcon] screen read back identical: {} pixel rows, {} glyph cells match the font, sha256 {}",
                check.pixel_lines,
                check.glyph_cells,
                Hex(&check.sha256)
            ),
            Some(check) => log::warn!(
                "[fbcon] screen read back differs: {} of {} pixel rows from the shadow, {} pixels from the font",
                check.differing_lines,
                check.pixel_lines,
                check.font_mismatches
            ),
            None => log::info!("[fbcon] screen not read back: a process has the display"),
        }
    }
    if token != 0 {
        *PROC_RESULT.lock() = check;
        PROC_DONE.complete(token);
    }
}

/// Ask the render thread to read the screen back and report it in the log.
pub fn request_idle_check() {
    IDLE_CHECK.store(true, Ordering::Release);
}

/// `/proc/fbcon/check`: the render thread reads the screen back; this reports
/// whether it matches the shadow and the font, and the text rows drawn.
pub fn proc_check() -> Result<String, i32> {
    const EACCES: i32 = 13;
    const EBUSY: i32 = 16;
    const EIO: i32 = 5;
    // Retained console text can contain privileged output. Authorize before
    // requesting or consuming a snapshot; procfs generates it at open time.
    let privileged = crate::task::scheduler::current_thread_id().is_some_and(|tid| {
        let guard = crate::process::manager();
        guard.as_ref().is_some_and(|manager| {
            manager.find_process_by_thread(tid).is_some_and(|(_, process)| process.cred.euid == 0)
        })
    });
    if !privileged {
        return Err(EACCES);
    }
    if !RUNNING.load(Ordering::Acquire) {
        return Ok(String::from("fbcon check: no screen console\n"));
    }
    if PROC_BUSY.swap(true, Ordering::AcqRel) {
        return Err(EBUSY);
    }
    let token = PROC_TOKEN.fetch_add(1, Ordering::AcqRel).wrapping_add(1).max(1);
    PROC_DONE.reset();
    *PROC_RESULT.lock() = None;
    PROC_CHECK.store(token, Ordering::Release);
    let waited = PROC_DONE.wait_timeout(token, 5_000_000_000);
    PROC_CHECK.store(0, Ordering::Release);
    let check = PROC_RESULT.lock().take();
    PROC_BUSY.store(false, Ordering::Release);
    match waited {
        Ok(true) => {}
        Ok(false) => return Err(EIO),
        Err(error) => return Err(error),
    }
    let mut text = String::new();
    match check {
        None => text.push_str("fbcon check: a process has the display\n"),
        Some(check) => {
            if check.identical() {
                let _ = writeln!(
                    text,
                    "fbcon check: shadow and screen identical, {} glyph cells match the font",
                    check.glyph_cells
                );
            } else {
                let _ = writeln!(
                    text,
                    "fbcon check: shadow and screen differ: {} of {} pixel rows, {} pixels differ from the font",
                    check.differing_lines, check.pixel_lines, check.font_mismatches
                );
            }
            let _ = writeln!(text, "fbcon sha256: {}", Hex(&check.sha256));
            for row in &check.rows {
                text.push_str("fbcon row: ");
                text.extend(row.iter().map(|&byte| char::from(byte)));
                text.push('\n');
            }
        }
    }
    let dropped = DROPPED.load(Ordering::Relaxed);
    if dropped != 0 {
        let _ = writeln!(text, "fbcon dropped: {} writes", dropped);
    }
    Ok(text)
}
