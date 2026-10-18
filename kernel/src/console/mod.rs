//! The screen console: the console TTY and the kernel log, as text, on the
//! firmware's or the boot framebuffer, on both architectures.
//!
//! Kernel log records and the console TTY's output are written, as text, into
//! one retained character grid (`grid`). Writers take the grid's lock with
//! interrupts masked, for one record or one write, so they can come from
//! interrupt handlers as well as threads, and they do not touch the screen.
//! The grid is sized for the display by the size rule (`raster::Font`).
//!
//! The screen has a single writer, the `console` kernel thread. It draws the
//! rows that changed since its last pass into a RAM shadow of the screen and
//! hands only those rows' pixels to the display (`display`), which writes them
//! to scanout memory or to a virtio-gpu resource's backing and flushes them.
//! Drawing does not read the screen; the read-back check (`/proc/console/check`,
//! and once when the console first fills the screen) reads it and compares it
//! with the shadow and with pixels worked out from the font and the text alone
//! (`raster::expected_intensity`). The check ties what it read to the scanout:
//! scanout memory, or the backing of the resource the device last accepted for
//! scanout 0 after a completed flush of the whole screen
//! (`display::Display::settle`).
//!
//! **x86-64** draws from `init_early` on; its log lines and `/proc/fbcon/check`
//! keep the `[fbcon]` and `fbcon` text the x86-metal boot path measures.
//! **ARM64** keeps the kernel log from the moment the display exists, and takes
//! the screen from the boot screen when the first process starts (`take_screen`).
//! ARM64 has no levelled logger: its kernel log is the kernel's serial output
//! (`log_bytes`, fed from `serial_line`), drawn as INFO.
//!
//! The console starts in init's session. While it is there, or in no session,
//! kernel log records of INFO and above are drawn; while another session has
//! it as its controlling terminal (a getty and the login it starts), only
//! warnings and errors are, so the console's user is not shown the per-second
//! census lines. Serial keeps each record. Once a process takes the display
//! (`take_display`) the console stops drawing.

pub mod display;
pub mod grid;
pub mod raster;
#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

use alloc::string::String;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use display::{Display, Source};
use grid::Grid;
use raster::Layout;
use spin::Mutex;

/// The tag of the console's log lines: x86-metal's measured stages match
/// `[fbcon]`.
#[cfg(target_arch = "x86_64")]
const TAG: &str = "[fbcon]";
#[cfg(target_arch = "aarch64")]
const TAG: &str = "[console]";

/// A console log line. ARM64 kernels have no `log` backend, so their lines go
/// to serial, which feeds the kernel log.
#[cfg(target_arch = "x86_64")]
macro_rules! say {
    (warn, $($arg:tt)*) => { log::warn!($($arg)*) };
    (error, $($arg:tt)*) => { log::error!($($arg)*) };
    (info, $($arg:tt)*) => { log::info!($($arg)*) };
}
#[cfg(target_arch = "aarch64")]
macro_rules! say {
    ($level:ident, $($arg:tt)*) => { crate::serial_println!($($arg)*) };
}

static DISPLAY: OnceCell<Display> = OnceCell::uninit();

/// The grid and who left its last line open.
struct Console {
    grid: Grid,
    /// The last write was kernel log output that did not end its line.
    kernel_line_open: bool,
}

static CONSOLE: Mutex<Console> = Mutex::new(Console { grid: Grid::new(), kernel_line_open: false });

/// Grid writes given up because its lock stayed held (a holder that faulted).
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// A session other than init's has the console as its controlling terminal.
static SESSION: AtomicBool = AtomicBool::new(false);
/// Only warnings and errors are drawn. The render thread follows `SESSION`
/// here, saying so on the screen as it changes.
static QUIET: AtomicBool = AtomicBool::new(false);

/// The console has the screen: on x86 once attached, on ARM64 once the boot
/// screen gave way.
static SCREEN: AtomicBool = AtomicBool::new(false);
/// A process has the display.
static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);

/// What `attach` set up, for the render thread.
struct Attached {
    shadow: Vec<u8>,
    #[cfg(target_arch = "x86_64")]
    write_combining: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>,
}

static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The idle read-back has been asked for.
static IDLE_CHECK: AtomicBool = AtomicBool::new(false);
/// The token of the `/proc` check request waiting, or 0.
static PROC_CHECK: AtomicU32 = AtomicU32::new(0);
static PROC_TOKEN: AtomicU32 = AtomicU32::new(0);
static PROC_BUSY: AtomicBool = AtomicBool::new(false);
static PROC_DONE: crate::task::completion::Completion = crate::task::completion::Completion::new();
static PROC_RESULT: Mutex<Option<Answer>> = Mutex::new(None);

/// How often the render thread looks at the grid.
const PASS_INTERVAL_MS: u64 = 20;

/// Attempts at the grid's lock before a write is dropped. Holders keep it for
/// one record with interrupts masked, a few microseconds; this is milliseconds.
const GRID_SPINS: usize = 100_000;

/// Run `f` on the console with interrupts masked; no result when its lock
/// stayed held.
fn with_grid<R>(f: impl FnOnce(&mut Console) -> R) -> Option<R> {
    crate::arch_without_interrupts(|| {
        for _ in 0..GRID_SPINS {
            if let Some(mut console) = CONSOLE.try_lock() {
                return Some(f(&mut console));
            }
            core::hint::spin_loop();
        }
        DROPPED.fetch_add(1, Ordering::Relaxed);
        None
    })
}

/// Record the display and size the grid to it. Log records are kept in the
/// grid from here; the render thread draws them once it runs.
fn set_display(display: Display) {
    let layout = display.layout();
    if layout.cols() == 0 || layout.rows() == 0 {
        return;
    }
    if DISPLAY.try_init_once(|| display).is_ok() {
        with_grid(|console| console.grid.set_size(layout.cols(), layout.rows()));
    }
}

/// Allocate the shadow for the render thread, and say what the console draws on.
fn prepare(#[cfg(target_arch = "x86_64")] write_combining: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>) {
    let Some(display) = DISPLAY.get() else {
        return;
    };
    let layout = display.layout();
    let mut shadow = Vec::new();
    if shadow.try_reserve_exact(layout.bytes()).is_err() {
        say!(warn, "{} no screen console: no memory for a {} byte shadow", TAG, layout.bytes());
        return;
    }
    shadow.resize(layout.bytes(), 0);
    let font = layout.font;
    say!(
        info,
        "[console] display {}x{} {}: font {} ({}x{} pixel cells), grid {}x{}",
        layout.width,
        layout.height,
        display.kind(),
        font.size(),
        font.glyph_width(),
        font.line_height(),
        layout.cols(),
        layout.rows()
    );
    *ATTACHED.lock() = Some(Attached {
        shadow,
        #[cfg(target_arch = "x86_64")]
        write_combining,
    });
}

/// A log line formatted into fixed storage; longer lines are cut short.
#[cfg(target_arch = "x86_64")]
struct Line {
    bytes: [u8; 256],
    len: usize,
}

#[cfg(target_arch = "x86_64")]
impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Put a kernel log record on the console, as its own line.
#[cfg(target_arch = "x86_64")]
pub fn log_record(level: log::Level, args: &fmt::Arguments) {
    if DISPLAY_TAKEN.load(Ordering::Acquire) || (QUIET.load(Ordering::Acquire) && level > log::Level::Warn) {
        return;
    }
    let mut line = Line { bytes: [0; 256], len: 0 };
    let _ = write!(line, "{:>5}: {}", level, args);
    with_grid(|console| {
        console.grid.begin_line();
        console.grid.write(&line.bytes[..line.len]);
        console.grid.write(b"\r\n");
        console.kernel_line_open = false;
    });
}

/// Put kernel serial output on the console: a record from `serial_line`, a
/// line or part of one. It starts a fresh row unless it continues the kernel's
/// own open line, and each line it ends returns to the first column (kernel
/// records end lines with LF alone). Kernel output carries no level, so it is
/// drawn as INFO.
#[cfg(target_arch = "aarch64")]
pub fn log_bytes(bytes: &[u8]) {
    if bytes.is_empty() || DISPLAY_TAKEN.load(Ordering::Acquire) || QUIET.load(Ordering::Acquire) {
        return;
    }
    with_grid(|console| {
        if !console.kernel_line_open {
            console.grid.begin_line();
        }
        for piece in bytes.split_inclusive(|&byte| byte == b'\n') {
            match piece.strip_suffix(b"\n") {
                Some(line) => {
                    console.grid.write(line);
                    console.grid.write(b"\r\n");
                }
                None => console.grid.write(piece),
            }
        }
        console.kernel_line_open = !bytes.ends_with(b"\n");
    });
}

/// Put console TTY output on the console.
pub fn write_tty(bytes: &[u8]) {
    if DISPLAY_TAKEN.load(Ordering::Acquire) {
        return;
    }
    with_grid(|console| {
        if console.kernel_line_open {
            console.grid.begin_line();
            console.kernel_line_open = false;
        }
        console.grid.write(bytes);
    });
}

/// A session other than init's took the console as its controlling terminal
/// (`true`), or the console is back in init's session or out of sessions (`false`).
pub fn console_session(held: bool) {
    SESSION.store(held, Ordering::Release);
}

/// A process has the display: stop drawing on it. Returns once no row of the
/// console's is being written, so the new owner's first draw is not overwritten.
pub fn take_display() {
    DISPLAY_TAKEN.store(true, Ordering::Release);
    display::wait_until_idle();
}

/// Whether the console has the screen and no process has taken it.
pub fn has_screen() -> bool {
    SCREEN.load(Ordering::Acquire) && !DISPLAY_TAKEN.load(Ordering::Acquire)
}

/// Asked with the screen held before each band is written.
fn may_draw() -> bool {
    if DISPLAY_TAKEN.load(Ordering::Acquire) {
        return false;
    }
    #[cfg(target_arch = "aarch64")]
    if crate::graphics::boot_screen::diagnostics_up() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// x86-64: the firmware framebuffer.
// ---------------------------------------------------------------------------

/// The bootloader's framebuffer description, for `attach` and the fb syscalls.
#[cfg(target_arch = "x86_64")]
static FRAMEBUFFER_INFO: OnceCell<bootloader_api::info::FrameBufferInfo> = OnceCell::uninit();

/// Record the bootloader's framebuffer and size the grid to it.
#[cfg(target_arch = "x86_64")]
pub fn init_early(buffer: &'static mut [u8], info: bootloader_api::info::FrameBufferInfo) {
    use bootloader_api::info::PixelFormat;
    let _ = FRAMEBUFFER_INFO.try_init_once(|| info);
    if !matches!((info.pixel_format, info.bytes_per_pixel), (PixelFormat::Rgb | PixelFormat::Bgr, 3 | 4)) {
        return;
    }
    let layout = Layout {
        width: info.width,
        height: info.height,
        bytes_per_pixel: info.bytes_per_pixel,
        stride_bytes: info.stride * info.bytes_per_pixel,
        reserved: 0,
        font: raster::Font::for_display(info.width, info.height),
    };
    // SAFETY: the bootloader's framebuffer mapping lives for the kernel's lifetime.
    if let Some(linear) = unsafe { display::linear::Linear::new(buffer.as_mut_ptr(), buffer.len(), layout) } {
        set_display(Display::Linear(linear));
    }
}

/// Find the display function's BAR, map it write-combining, and allocate the
/// shadow (`display::linear::attach`).
#[cfg(target_arch = "x86_64")]
pub fn attach() {
    let Some(info) = FRAMEBUFFER_INFO.get() else {
        return;
    };
    let Some(Display::Linear(linear)) = DISPLAY.get() else {
        log::warn!(
            "[fbcon] no screen console: pixel format {:?}, {} bytes per pixel",
            info.pixel_format,
            info.bytes_per_pixel
        );
        return;
    };
    let Some(attached) = display::linear::attach(linear, info.pixel_format, info.stride) else {
        return;
    };
    prepare(attached.write_combining);
    SCREEN.store(true, Ordering::Release);
}

/// The framebuffer's layout: width, height and stride in pixels, bytes per
/// pixel, and whether pixels are BGR. No layout without a framebuffer, or for
/// one whose pixels are not 3- or 4-byte RGB or BGR.
#[cfg(target_arch = "x86_64")]
pub fn display_geometry() -> Option<(usize, usize, usize, usize, bool)> {
    DISPLAY.get()?;
    let info = FRAMEBUFFER_INFO.get()?;
    Some((
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        info.pixel_format == bootloader_api::info::PixelFormat::Bgr,
    ))
}

/// Copy rows `rows` and columns `columns` of a display owner's pane onto the
/// framebuffer (`display::linear::Linear::copy_pane`).
///
/// # Safety
/// As `Linear::copy_pane`.
#[cfg(target_arch = "x86_64")]
pub unsafe fn copy_to_display(
    pane: *const u8,
    pane_stride: usize,
    rows: core::ops::Range<usize>,
    columns: core::ops::Range<usize>,
) -> bool {
    match DISPLAY.get() {
        // SAFETY: the caller's contract.
        Some(Display::Linear(linear)) => unsafe { linear.copy_pane(pane, pane_stride, rows, columns) },
        None => false,
    }
}

/// Read one hardware framebuffer pixel as 0x00RRGGBB directly from device memory.
/// Invalid coordinates and busy/unavailable displays return an empty result.
#[cfg(target_arch = "x86_64")]
pub fn read_display_pixel(x: usize, y: usize) -> Option<u32> {
    let Some(Display::Linear(linear)) = DISPLAY.get() else {
        return None;
    };
    let [c0, c1, c2] = linear.read_pixel(x, y)?;
    let (r, b) = match FRAMEBUFFER_INFO.get()?.pixel_format {
        bootloader_api::info::PixelFormat::Rgb => (c0, c2),
        bootloader_api::info::PixelFormat::Bgr => (c2, c0),
        _ => return None,
    };
    Some((u32::from(r) << 16) | (u32::from(c1) << 8) | u32::from(b))
}

// ---------------------------------------------------------------------------
// ARM64: the GOP framebuffer, or a virtio-gpu resource.
// ---------------------------------------------------------------------------

/// Pick the display the kernel's boot screen drew on, size the grid for it,
/// put the kernel log so far into the grid, and allocate the shadow. Called
/// once the framebuffer exists and the heap is up.
#[cfg(target_arch = "aarch64")]
pub fn attach() {
    use crate::graphics::arm64_fb;
    let display = if arm64_fb::is_gop_active() {
        gop_display()
    } else if crate::drivers::virtio::gpu_pci::is_initialized() {
        display::virgl_backing::VirglBacking::new().map(Display::VirglBacking)
    } else {
        display::virtio_2d::Virtio2d::new().map(Display::Virtio2d)
    };
    let Some(display) = display else {
        say!(warn, "[console] no screen console: no display the console can draw on");
        return;
    };
    set_display(display);
    // The boot so far, from the kernel log ring: only kernel output has been
    // written so far.
    let mut boot_log = Vec::new();
    if boot_log.try_reserve_exact(16 * 1024).is_ok() {
        boot_log.resize(16 * 1024, 0);
        let n = crate::log_buffer::copy_tail(&mut boot_log);
        log_bytes(&boot_log[..n]);
    }
    prepare();
}

#[cfg(target_arch = "aarch64")]
fn gop_display() -> Option<Display> {
    use crate::graphics::arm64_fb;
    let info = arm64_fb::FB_INFO_CACHE.get()?;
    let buffer = arm64_fb::gop_framebuffer()?;
    let layout = Layout {
        width: info.width,
        height: info.height,
        bytes_per_pixel: info.bytes_per_pixel,
        stride_bytes: info.stride * info.bytes_per_pixel,
        reserved: 0xff,
        font: raster::Font::for_display(info.width, info.height),
    };
    if !matches!(layout.bytes_per_pixel, 3 | 4) {
        return None;
    }
    // SAFETY: the GOP framebuffer's direct-map mapping lives for the kernel's lifetime.
    let linear = unsafe { display::linear::Linear::new(buffer.as_mut_ptr(), buffer.len(), layout) }?;
    linear.locate(crate::platform_config::fb_base_phys(), display::linear::Place::Gop);
    Some(Display::Linear(linear))
}

/// The boot screen gives the screen to the console: called when the first
/// process starts. The console does not take it once the diagnostics screen is
/// up or a process has taken the display.
#[cfg(target_arch = "aarch64")]
pub fn take_screen() {
    if DISPLAY.get().is_some() && crate::graphics::boot_screen::give_way() {
        SCREEN.store(true, Ordering::Release);
    }
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
    match crate::task::kthread::kthread_run(move || render_thread(display, attached), "console") {
        Ok(_) => {
            // Keep the normal time-sharing policy: an idle-policy thread can
            // starve indefinitely behind a runnable graphics thread or users.
            // The render loop blocks between passes to bound its CPU demand.
            RUNNING.store(true, Ordering::Release);
        }
        Err(error) => say!(error, "{} no render thread: {:?}", TAG, error),
    }
}

/// A text row as last drawn.
#[derive(Clone, PartialEq, Eq)]
struct Shown {
    text: Vec<u8>,
    cursor: Option<usize>,
}

struct Screen {
    display: &'static Display,
    layout: Layout,
    shadow: Vec<u8>,
    /// What each text row of the screen shows, once it is drawn.
    shown: Vec<Option<Shown>>,
    cleared: bool,
    /// Passes that left each row of the screen written and flushed.
    frames: u64,
    /// The last pass wrote and flushed each row it drew, and left no row undrawn.
    complete: bool,
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
        let copied = with_grid(|console| {
            let grid = &console.grid;
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

fn render_thread(display: &'static Display, attached: Attached) {
    #[cfg(target_arch = "x86_64")]
    display::linear::report_write_combining(attached.write_combining);
    let layout = display.layout();
    let (cols, rows) = with_grid(|console| (console.grid.cols(), console.grid.rows())).unwrap_or((0, 0));
    let mut snapshot = Snapshot {
        cols,
        rows,
        cells: alloc::vec![grid::BLANK; cols * rows],
        cursor: (0, 0),
        generation: u64::MAX,
    };
    let mut screen = Screen {
        display,
        layout,
        shadow: attached.shadow,
        shown: alloc::vec![None; rows],
        cleared: false,
        frames: 0,
        complete: false,
    };

    let mut announced = false;
    while !crate::task::kthread::kthread_should_stop() {
        if !SCREEN.load(Ordering::Acquire) {
            answer_checks(Err(Absent::NotYet));
            sleep_ms(PASS_INTERVAL_MS);
            continue;
        }
        if !may_draw() {
            answer_checks(Err(Absent::Taken));
            sleep_ms(PASS_INTERVAL_MS * 10);
            continue;
        }
        let changed = snapshot.refresh();
        if changed || !screen.complete {
            let started = crate::time::get_monotonic_time_ns();
            let drawn = screen.update(&snapshot);
            if !announced && screen.complete {
                announced = true;
                let elapsed = elapsed_us(started);
                say!(
                    info,
                    "{} console tty and kernel log on the screen: {}x{} cells, {} rows drawn in {}.{:03} ms",
                    TAG,
                    cols,
                    rows,
                    drawn,
                    elapsed / 1000,
                    elapsed % 1000
                );
                // x86 reads the screen back at kernel idle; ARM64 once the
                // console first fills it.
                #[cfg(target_arch = "aarch64")]
                request_idle_check();
            }
        }
        let session = SESSION.load(Ordering::Acquire);
        if session != QUIET.load(Ordering::Acquire) {
            if session {
                say!(info, "{} the console has a session: only kernel warnings and errors are drawn", TAG);
                QUIET.store(true, Ordering::Release);
            } else {
                QUIET.store(false, Ordering::Release);
                say!(info, "{} the console's session ended: the kernel log is drawn again", TAG);
            }
        }
        if announced {
            answer_checks(Ok(&screen));
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

/// Block the render thread on the scheduler timer for `ms`.
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
    #[cfg(target_arch = "x86_64")]
    {
        scheduler::yield_current();
        while scheduler::thread_state(tid) == Some(ThreadState::BlockedOnTimer) {
            crate::arch_halt_with_interrupts();
        }
    }
    // Give up the CPU at once rather than waiting in WFI as its current thread,
    // as the ARM64 render thread does (`graphics::render_task`, #1177).
    #[cfg(target_arch = "aarch64")]
    loop {
        let _ = scheduler::check_and_clear_need_resched();
        scheduler::schedule();
        if scheduler::thread_state(tid) != Some(ThreadState::BlockedOnTimer) {
            break;
        }
    }
}

impl Screen {
    /// Draw the rows that differ from what the screen shows, then flush them.
    /// Returns how many rows were written.
    fn update(&mut self, snapshot: &Snapshot) -> usize {
        if !self.cleared {
            raster::clear(&mut self.shadow, &self.layout);
            self.cleared = self.display.write(&self.shadow, 0..self.layout.height, may_draw)
                && self.display.flush(0..self.layout.height).is_ok();
            if !self.cleared {
                self.complete = false;
                return 0;
            }
        }
        let mut drawn = 0;
        let mut failed = false;
        let mut dirty: Option<core::ops::Range<usize>> = None;
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
            let band = self.layout.band(row);
            if self.display.write(&self.shadow, band.clone(), may_draw) {
                self.shown[row] = Some(wanted);
                drawn += 1;
                dirty = Some(match dirty {
                    Some(lines) => lines.start.min(band.start)..lines.end.max(band.end),
                    None => band,
                });
            } else {
                self.shown[row] = None;
                failed = true;
            }
        }
        if let Some(lines) = dirty {
            if self.display.flush(lines.clone()).is_err() {
                // The rows are drawn and flushed again on the next pass.
                let line_height = self.layout.font.line_height();
                for shown in &mut self.shown[lines.start / line_height..lines.end.div_ceil(line_height)] {
                    *shown = None;
                }
                failed = true;
            }
        }
        self.complete = !failed && self.shown.iter().all(Option::is_some);
        if self.complete {
            self.frames += 1;
        }
        drawn
    }

    /// Read the screen back and compare it with the shadow, and with the pixels
    /// the font gives for the text each row was drawn with.
    fn check(&self) -> Check {
        let layout = self.layout;
        let bpp = layout.bytes_per_pixel;
        let font = layout.font;
        let source = self.display.settle();
        let mut line = alloc::vec![0u8; layout.width * bpp];
        let mut differing_lines = 0;
        let mut first_difference = None;
        let mut font_mismatches = 0;
        if source.is_ok() {
            for y in 0..layout.height {
                let bytes = layout.line(y);
                self.display.read_line(y, &mut line);
                if line[..] != self.shadow[bytes.clone()] {
                    differing_lines += 1;
                    if first_difference.is_none() {
                        let shadow = &self.shadow[bytes];
                        let x = line.chunks_exact(bpp).zip(shadow.chunks_exact(bpp)).position(|(a, b)| a != b);
                        first_difference = Some((x.unwrap_or(0), y));
                    }
                }
                let row = y / font.line_height();
                let shown = self.shown.get(row).and_then(Option::as_ref);
                for (x, pixel) in line.chunks_exact(bpp).enumerate() {
                    let expected = match shown {
                        Some(shown) => {
                            raster::expected_intensity(font, &shown.text, shown.cursor, x, y % font.line_height())
                        }
                        None => 0,
                    };
                    if !layout.pixel_is(pixel, expected) {
                        font_mismatches += 1;
                    }
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
        Check {
            layout,
            kind: self.display.kind(),
            source,
            frames: self.frames,
            complete: self.complete,
            pixel_lines: layout.height,
            differing_lines,
            first_difference,
            font_mismatches,
            glyph_cells,
            sha256: sha256::sha256(&self.shadow),
            rows,
        }
    }
}

// ---------------------------------------------------------------------------
// The read-back check.
// ---------------------------------------------------------------------------

/// One read-back of the screen.
struct Check {
    layout: Layout,
    kind: &'static str,
    /// What was read, or why the read could not be tied to the scanout.
    source: Result<Source, &'static str>,
    /// Passes that left each row of the screen written and flushed, and whether
    /// the last did.
    frames: u64,
    complete: bool,
    pixel_lines: usize,
    /// Pixel rows of the screen that differ from the shadow, and the first
    /// pixel that does.
    differing_lines: usize,
    first_difference: Option<(usize, usize)>,
    /// Pixels of the screen that differ from what the font gives.
    font_mismatches: usize,
    glyph_cells: usize,
    sha256: [u8; 32],
    /// The text rows drawn, without trailing blanks, blank rows left out.
    rows: Vec<Vec<u8>>,
}

impl Check {
    fn identical(&self) -> bool {
        self.source.is_ok() && self.complete && self.differing_lines == 0 && self.font_mismatches == 0
    }
}

/// Why a check had no screen to read.
#[derive(Clone, Copy)]
enum Absent {
    /// ARM64: the boot screen still has the display.
    NotYet,
    /// A process has the display, or the diagnostics screen is up.
    Taken,
}

type Answer = Result<Check, Absent>;

struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{:02x}", byte))
    }
}

/// Answer the checks asked for: on the render thread, after a pass, so the
/// screen is what `shown` says it is.
fn answer_checks(screen: Result<&Screen, Absent>) {
    let idle = IDLE_CHECK.swap(false, Ordering::AcqRel);
    let token = PROC_CHECK.swap(0, Ordering::AcqRel);
    if !idle && token == 0 {
        return;
    }
    let answer = screen.map(Screen::check);
    if idle {
        match &answer {
            Ok(check) if check.identical() => match &check.source {
                #[cfg(target_arch = "aarch64")]
                Ok(source) => say!(
                    info,
                    "{} screen read back identical: {} pixel rows, {} glyph cells match the font, sha256 {}; {}",
                    TAG,
                    check.pixel_lines,
                    check.glyph_cells,
                    Hex(&check.sha256),
                    source
                ),
                _ => say!(
                    info,
                    "{} screen read back identical: {} pixel rows, {} glyph cells match the font, sha256 {}",
                    TAG,
                    check.pixel_lines,
                    check.glyph_cells,
                    Hex(&check.sha256)
                ),
            },
            Ok(check) => match &check.source {
                Err(reason) => say!(warn, "{} screen read back differs: not read from the scanout: {}", TAG, reason),
                Ok(_) if !check.complete => say!(warn, "{} screen read back differs: the last frame was not completed", TAG),
                Ok(_) => say!(
                    warn,
                    "{} screen read back differs: {} of {} pixel rows from the shadow, {} pixels from the font",
                    TAG,
                    check.differing_lines,
                    check.pixel_lines,
                    check.font_mismatches
                ),
            },
            Err(Absent::NotYet) => say!(info, "{} screen not read back: the boot screen has the display", TAG),
            Err(Absent::Taken) => say!(info, "{} screen not read back: a process has the display", TAG),
        }
    }
    if token != 0 {
        *PROC_RESULT.lock() = Some(answer);
        PROC_DONE.complete(token);
    }
}

/// Ask the render thread to read the screen back and report it in the log.
pub fn request_idle_check() {
    IDLE_CHECK.store(true, Ordering::Release);
}

/// Have the render thread read the screen back for a root-only `/proc` file.
/// Retained console text can contain privileged output, so the caller is
/// authorized before a snapshot is requested; procfs generates it at open time.
fn proc_answer() -> Result<Option<Answer>, i32> {
    const EACCES: i32 = 13;
    const EBUSY: i32 = 16;
    const EIO: i32 = 5;
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
        return Ok(None);
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
    let answer = PROC_RESULT.lock().take();
    PROC_BUSY.store(false, Ordering::Release);
    match waited {
        Ok(true) => answer.map(Some).ok_or(EIO),
        Ok(false) => Err(EIO),
        Err(error) => Err(error),
    }
}

fn push_rows(text: &mut String, prefix: &str, rows: &[Vec<u8>]) {
    for row in rows {
        text.push_str(prefix);
        text.extend(row.iter().map(|&byte| char::from(byte)));
        text.push('\n');
    }
}

fn push_dropped(text: &mut String, prefix: &str) {
    let dropped = DROPPED.load(Ordering::Relaxed);
    if dropped != 0 {
        let _ = writeln!(text, "{} dropped: {} writes", prefix, dropped);
    }
}

/// `/proc/console/check`: the render thread reads the screen back; this says
/// what it read and where, whether it matches the shadow and the font, and
/// the text rows on the screen.
pub fn proc_console_check() -> Result<String, i32> {
    let mut text = String::new();
    match proc_answer()? {
        None => text.push_str("console check: no screen console\n"),
        Some(Err(Absent::NotYet)) => text.push_str("console check: the boot screen has the display\n"),
        Some(Err(Absent::Taken)) => text.push_str("console check: a process has the display\n"),
        Some(Ok(check)) => {
            let layout = check.layout;
            let font = layout.font;
            text.push_str("console foreground: tty1\n");
            let _ = writeln!(
                text,
                "console display: {}x{} {}, font {} ({}x{} pixel cells), grid {}x{}",
                layout.width,
                layout.height,
                check.kind,
                font.size(),
                font.glyph_width(),
                font.line_height(),
                layout.cols(),
                layout.rows()
            );
            match &check.source {
                Ok(source) => {
                    let _ = writeln!(text, "console scanout: {}", source);
                }
                Err(reason) => {
                    let _ = writeln!(text, "console scanout: not tied to the screen: {}", reason);
                }
            }
            let _ = writeln!(
                text,
                "console frame: {} {}",
                check.frames,
                if check.complete { "complete" } else { "incomplete" }
            );
            match (&check.source, check.first_difference) {
                (Err(_), _) => text.push_str("console check: shadow and screen not compared\n"),
                (Ok(_), Some((x, y))) => {
                    let _ = writeln!(text, "console check: shadow and screen differ at x={} y={}", x, y);
                }
                (Ok(_), None) if !check.complete => {
                    text.push_str("console check: shadow and screen identical, but the frame is incomplete\n")
                }
                (Ok(_), None) => text.push_str("console check: shadow and screen identical\n"),
            }
            let _ = writeln!(text, "console oracle: cells={} mismatches={}", check.glyph_cells, check.font_mismatches);
            let _ = writeln!(text, "console sha256: {}", Hex(&check.sha256));
            push_rows(&mut text, "console row: ", &check.rows);
        }
    }
    push_dropped(&mut text, "console");
    Ok(text)
}

/// `/proc/fbcon/check`, as Metal 4's x86-metal boot path reads it: whether the
/// screen matches the shadow and the font, the shadow's SHA-256, and the text
/// rows drawn.
#[cfg(target_arch = "x86_64")]
pub fn proc_fbcon_check() -> Result<String, i32> {
    let mut text = String::new();
    match proc_answer()? {
        None => text.push_str("fbcon check: no screen console\n"),
        Some(Err(_)) => text.push_str("fbcon check: a process has the display\n"),
        Some(Ok(check)) => {
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
            push_rows(&mut text, "fbcon row: ", &check.rows);
        }
    }
    push_dropped(&mut text, "fbcon");
    Ok(text)
}
