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
//! (`raster::expected_colour`). The check ties what it read to the scanout:
//! scanout memory, or the backing of the resource the device last accepted for
//! scanout 0 after a completed flush of the whole screen
//! (`display::Display::settle`).
//!
//! Until the console takes the screen, it shows the boot splash (`splash`): the
//! logo and boot progress, drawn as soon as the display exists, which gives way
//! to the text on the first of the contract's triggers (a reader on tty1,
//! `KDSETMODE(KD_TEXT)`, a key, a display owner, a panic, or the backstop). The
//! grid keeps the kernel log meanwhile.
//!
//! **x86-64** draws the splash straight into the framebuffer at `init_early`,
//! and into the shadow from `attach` on; its log lines and `/proc/fbcon/check`
//! keep the `[fbcon]` and `fbcon` text the x86-metal boot path measures.
//! **ARM64** draws it when the display exists (`attach`). ARM64 carries
//! structured log severity through `serial_line`; unlevelled serial records
//! (`log_bytes`) are drawn as INFO.
//!
//! **Six virtual terminals** (`vt`, contract sections 1 to 4) share the screen:
//! tty1 is the console TTY and carries the kernel log, and each terminal has
//! its own grid and scrollback. The render thread draws the foreground
//! terminal's view, and completes a switch once the new terminal's frame is on
//! the display; it then reads that frame back and logs the terminal number and
//! a digest, not its text.
//!
//! The console starts in init's session. While it is there, or in no session,
//! kernel log records of INFO and above are drawn; while another session has
//! it as its controlling terminal (a getty and the login it starts), only
//! warnings and errors are, so the console's user is not shown the per-second
//! census lines. Serial keeps each record. Once a process takes the display
//! (`take_display`) the console stops drawing.

pub mod boxdraw;
pub mod display;
pub mod grid;
pub mod keymap;
pub mod keys;
pub mod raster;
pub mod splash;
pub mod vcs;
pub mod vt;
#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

use alloc::string::String;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use display::{Display, Source};
use raster::Layout;
use spin::Mutex;

/// The tag of the console's log lines: x86-metal's measured stages match
/// `[fbcon]`.
#[cfg(target_arch = "x86_64")]
const TAG: &str = "[fbcon]";
#[cfg(target_arch = "aarch64")]
const TAG: &str = "[console]";

/// A console log line, using each architecture's levelled serial logger.
#[cfg(target_arch = "x86_64")]
macro_rules! say {
    (warn, $($arg:tt)*) => { log::warn!($($arg)*) };
    (error, $($arg:tt)*) => { log::error!($($arg)*) };
    (info, $($arg:tt)*) => { log::info!($($arg)*) };
}
#[cfg(target_arch = "aarch64")]
macro_rules! say {
    (warn, $($arg:tt)*) => { log::warn!($($arg)*) };
    (error, $($arg:tt)*) => { log::error!($($arg)*) };
    (info, $($arg:tt)*) => { log::info!($($arg)*) };
}

static DISPLAY: OnceCell<Display> = OnceCell::uninit();

/// The terminals, which is in the foreground, and who left tty1's last line open.
struct Console {
    vts: [vt::Vt; vt::VT_COUNT],
    sw: vt::Switching,
    /// The last write to tty1 was kernel log output that did not end its line.
    kernel_line_open: bool,
}

static CONSOLE: Mutex<Console> = Mutex::new(Console {
    vts: [const { vt::Vt::new() }; vt::VT_COUNT],
    sw: vt::Switching::new(),
    kernel_line_open: false,
});

/// tty1's screens, in static storage: x86-64 sizes its grid at `init_early`,
/// before the heap, so the boot log is kept from the start. Sized for the
/// largest grid; `set_size` blanks what it uses.
const UNSET: grid::Cell = grid::Cell { glyph: 0, colours: 0, flags: 0 };
static mut TTY1_CELLS: [grid::Cell; grid::MAX_COLS * grid::MAX_ROWS] = [UNSET; grid::MAX_COLS * grid::MAX_ROWS];
static mut TTY1_ALTERNATE: [grid::Cell; grid::MAX_COLS * grid::MAX_ROWS] = [UNSET; grid::MAX_COLS * grid::MAX_ROWS];

/// Boot output can scroll before the heap exists.
static mut TTY1_HISTORY: [grid::Cell; grid::MAX_COLS * grid::HISTORY_LINES] =
    [UNSET; grid::MAX_COLS * grid::HISTORY_LINES];

/// Grid writes given up because its lock stayed held (a holder that faulted).
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// A session other than init's has the console as its controlling terminal.
static SESSION: AtomicBool = AtomicBool::new(false);

/// The console's text has the screen: the splash gave way.
static SCREEN: AtomicBool = AtomicBool::new(false);
/// The splash has been drawn on the screen: the console owns it from here.
static SPLASH_DRAWN: AtomicBool = AtomicBool::new(false);
/// A process has the display.
static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);

/// What `attach` set up: the shadow and the splash drawn in it, which the boot
/// thread keeps up to date until the render thread takes them.
struct Attached {
    shadow: Vec<u8>,
    splash: SplashRun,
    #[cfg(target_arch = "x86_64")]
    write_combining: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>,
}

static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
// Covers the entire render pass, including GPU completion and read-back.
static PRESENTING: AtomicBool = AtomicBool::new(false);
static REDRAW: AtomicBool = AtomicBool::new(false);

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
        // SAFETY: the display is recorded once, so these are the only
        // references ever taken to tty1's static screens.
        let (cells, alternate) =
            unsafe { (&mut *core::ptr::addr_of_mut!(TTY1_CELLS), &mut *core::ptr::addr_of_mut!(TTY1_ALTERNATE)) };
        with_grid(move |console| {
            let tty1 = &mut console.vts[0].grid;
            tty1.set_storage(cells, alternate);
            tty1.set_size(layout.cols(), layout.rows());
            // SAFETY: installed once with the display, like the static screens.
            tty1.set_history(unsafe { &mut *core::ptr::addr_of_mut!(TTY1_HISTORY) });
        });
    }
}

/// Give tty2 to tty6 their screens and scrollback before keyboard input can
/// reach them. tty1 already retains boot output in static storage.
fn size_terminals(layout: &Layout) {
    let size = layout.cols() * layout.rows();
    for index in 1..vt::VT_COUNT {
        let mut cells = Vec::new();
        let mut alternate = Vec::new();
        if cells.try_reserve_exact(size).is_err() || alternate.try_reserve_exact(size).is_err() {
            say!(warn, "[console] no tty{}: no memory for its screen", index + 1);
            continue;
        }
        cells.resize(size, UNSET);
        alternate.resize(size, UNSET);
        let (cells, alternate) = (cells.leak(), alternate.leak());
        with_grid(move |console| {
            let grid = &mut console.vts[index].grid;
            grid.set_storage(cells, alternate);
            grid.set_size(layout.cols(), layout.rows());
        });
        // Reserve before this terminal can receive interrupt-generated echo.
        vt::ensure_history(index);
    }
}

/// Allocate the shadow, say what the console draws on, and draw the splash.
fn prepare(
    bgr: bool,
    #[cfg(target_arch = "x86_64")] write_combining: Result<crate::arch_impl::x86_64::pagewalk::Report, &'static str>,
) {
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
    size_terminals(&layout);
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
        splash: SplashRun {
            painter: splash::Painter::new(layout, bgr),
            shown: false,
            first: None,
            progress_checked: false,
            failed: false,
            wait_until: None,
        },
        #[cfg(target_arch = "x86_64")]
        write_combining,
    });
    splash_boot_pass();
}

/// Bring the splash on the screen up to date from the boot thread, until the
/// render thread takes it over (`start`).
pub(crate) fn splash_boot_pass() {
    if !splash::is_up() {
        return;
    }
    let Some(display) = DISPLAY.get() else {
        return;
    };
    let Some(mut attached) = ATTACHED.try_lock() else {
        return;
    };
    if let Some(attached) = attached.as_mut() {
        let run = &mut attached.splash;
        run.shown = present_splash(display, &mut attached.shadow, &mut run.painter);
        // CPU0 may stay pinned through the final boot stage. Capture real
        // early pixels here and carry the sample into the render thread.
        if run.shown && !run.failed {
            run.read_back(display, &attached.shadow);
        }
    }
}

/// Draw what changed on the splash into the shadow and copy those rows to the
/// screen; true when the screen then shows the whole shadow. The first time it
/// is shown, say what was drawn.
fn present_splash(display: &Display, shadow: &mut [u8], painter: &mut splash::Painter) -> bool {
    let bands = painter.paint(shadow, splash::now_ms());
    let shown = bands
        .iter()
        .all(|rows| display.write(shadow, rows.clone(), may_draw) && may_draw() && display.flush(rows).is_ok());
    if !shown {
        painter.forget();
        return false;
    }
    if !SPLASH_DRAWN.swap(true, Ordering::AcqRel) {
        painter.announce();
        #[cfg(target_arch = "aarch64")]
        crate::graphics::boot_screen::console_drew();
    }
    true
}

/// A text cell's size in pixels, once the console has a display.
pub fn cell_size() -> Option<(usize, usize)> {
    let font = DISPLAY.get()?.layout().font;
    Some((font.glyph_width(), font.line_height()))
}

/// Whether the console has drawn on the screen (the splash, then its text).
pub fn owns_screen() -> bool {
    SPLASH_DRAWN.load(Ordering::Acquire)
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
    if SESSION.load(Ordering::Acquire) && level > log::Level::Warn {
        return;
    }
    let mut line = Line { bytes: [0; 256], len: 0 };
    let _ = write!(line, "{:>5}: {}", level, args);
    with_grid(|console| {
        let tty1 = &mut console.vts[0].grid;
        tty1.write_kernel(&line.bytes[..line.len], true);
        tty1.write_kernel(b"\n", false);
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
    log_bytes_at(log::Level::Info, bytes);
}

/// A serial record with a known severity, retained even during a quiet session
/// when it is a warning or error. Untyped serial output is INFO.
#[cfg(target_arch = "aarch64")]
pub fn log_bytes_at(level: log::Level, bytes: &[u8]) {
    if bytes.is_empty() || (SESSION.load(Ordering::Acquire) && level > log::Level::Warn) {
        return;
    }
    with_grid(|console| {
        console.vts[0].grid.write_kernel(bytes, !console.kernel_line_open);
        console.kernel_line_open = !bytes.ends_with(b"\n");
    });
}

/// Put console TTY output (tty1's) on the console.
pub fn write_tty(bytes: &[u8]) {
    write_vt(1, bytes);
}

/// Put terminal `number`'s (1-based) output into its grid. A terminal that
/// is not in the foreground keeps it there, shown when it returns.
pub fn write_vt(number: usize, bytes: &[u8]) {
    let index = number - 1;
    vt::ensure_history(index);
    with_grid(|console| {
        if index == 0 && console.kernel_line_open {
            console.vts[0].grid.begin_line();
            console.kernel_line_open = false;
        }
        let vt::Vt { grid, replies, .. } = &mut console.vts[index];
        // Keep whole replies until the input ring accepts them. The writer
        // may be echoing in IRQ context, so it only queues fixed storage.
        grid.write_with_reply(bytes, |reply| replies.push(reply));
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
    splash::request(splash::Reason::DisplayTaken);
    DISPLAY_TAKEN.store(true, Ordering::SeqCst);
    wait_until_not_presenting();
}

/// Woken at the end of each render pass, for the processes waiting in
/// `wait_until_not_presenting`.
static PASS_ENDED: crate::task::waitqueue::WaitQueueHead = crate::task::waitqueue::WaitQueueHead::new();

/// The render thread's pass is over: it has no row on its way to the screen.
fn end_pass() {
    PRESENTING.store(false, Ordering::SeqCst);
    if PASS_ENDED.has_waiters() {
        PASS_ENDED.wake_up();
    }
}

/// Wait until the render thread has no row of the console's on its way to
/// the screen and no fenced GPU command of its own outstanding: a timed-out
/// fenced command still belongs to the device, and a new owner may not draw
/// until that DMA has stopped. The caller is a process in a system call. It
/// blocks as an I/O wait, woken at the end of each pass and
/// after at most a millisecond to look at the GPU again. A signal cannot
/// finish the hand-over while a presentation is outstanding. The render
/// thread's own `sleep_ms` blocks a kernel thread on the timer, which a
/// process's system call must not do.
fn wait_until_not_presenting() {
    use crate::task::waitqueue::PrepareOutcome;
    let busy = || {
        #[cfg(target_arch = "aarch64")]
        if crate::drivers::virtio::gpu_pci::presentation_pending() {
            return true;
        }
        PRESENTING.load(Ordering::SeqCst)
    };
    while busy() {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let recheck = seconds * 1_000_000_000 + nanos + 1_000_000;
        let outcome = PASS_ENDED.prepare_to_wait_checked(
            crate::task::thread::ThreadState::BlockedOnIO,
            Some(recheck),
            busy,
        );
        if outcome == PrepareOutcome::Mismatch {
            PASS_ENDED.finish_wait();
            continue;
        }
        if crate::syscall::blocking_io::wait_prepared(&PASS_ENDED, outcome).is_err() {
            // The wait cleaned up its publication. Yield before rechecking;
            // ownership must not escape this barrier on EINTR or a wait error.
            crate::task::scheduler::yield_current();
        }
    }
    display::wait_until_idle();
}

/// Return the display to the retained terminal, repainting even unchanged rows.
pub fn give_back_display() {
    vt::give_back_display();
    REDRAW.store(true, Ordering::Release);
    DISPLAY_TAKEN.store(false, Ordering::SeqCst);
}

/// Whether the console has the screen and no process has taken it.
pub fn has_screen() -> bool {
    SCREEN.load(Ordering::Acquire) && !DISPLAY_TAKEN.load(Ordering::Acquire) && !vt::foreground_graphics()
}

/// Asked with the screen held before each band is written.
fn may_draw() -> bool {
    if DISPLAY_TAKEN.load(Ordering::Acquire) || vt::foreground_graphics() {
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
        bgr: matches!(info.pixel_format, PixelFormat::Bgr),
        font: raster::Font::for_display(info.width, info.height),
    };
    // The splash, straight into the framebuffer: the shadow needs the heap,
    // which comes later (`attach`).
    if layout.bytes() <= buffer.len() {
        splash::Look::new(layout, info.pixel_format == PixelFormat::Bgr).draw_all(buffer, &splash::View::now(0));
    }
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
    prepare(info.pixel_format == bootloader_api::info::PixelFormat::Bgr, attached.write_combining);
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

/// Read the active ARM64 scanout memory or its attached guest backing without
/// presenting it (a display owner may have replaced the console's pixels).
#[cfg(target_arch = "aarch64")]
pub fn read_display_pixel(x: usize, y: usize) -> Option<u32> {
    let display = DISPLAY.get()?;
    let layout = display.layout();
    if x >= layout.width || y >= layout.height {
        return None;
    }
    let mut line = alloc::vec![0; layout.width * layout.bytes_per_pixel];
    display.read_line(y, &mut line);
    let offset = x * layout.bytes_per_pixel;
    let [a, g, b] = [line[offset], line[offset + 1], line[offset + 2]];
    let bgr = crate::graphics::arm64_fb::FB_INFO_CACHE.get()?.is_bgr;
    let (r, b) = if bgr { (b, a) } else { (a, b) };
    Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

// ---------------------------------------------------------------------------
// ARM64: the GOP framebuffer, or a virtio-gpu resource.
// ---------------------------------------------------------------------------

/// Pick the display, size the grid for it, put the kernel log so far into the
/// grid, allocate the shadow and draw the splash. Called once the framebuffer
/// exists and the heap is up.
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
    prepare(arm64_fb::FB_INFO_CACHE.get().is_none_or(|info| info.is_bgr));
}

/// Draw directly into GOP scanout before allocating or copying any shadow.
#[cfg(target_arch = "aarch64")]
pub fn init_gop_early() {
    let Some(display) = gop_display() else {
        return;
    };
    let Some(buffer) = crate::graphics::arm64_fb::gop_framebuffer() else {
        return;
    };
    let layout = display.layout();
    splash::Look::new(layout, layout.bgr).draw_all(buffer, &splash::View::now(0));
    display::drain_writes();
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
        bgr: info.is_bgr,
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

// ---------------------------------------------------------------------------
// The render thread.
// ---------------------------------------------------------------------------

/// Start the render thread, if `attach` found the display. The boot thread
/// keeps drawing the splash until the thread first runs and takes it.
pub fn start() {
    if ATTACHED.lock().is_none() {
        return;
    }
    let Some(display) = DISPLAY.get() else {
        return;
    };
    let render = move || {
        if let Some(attached) = ATTACHED.lock().take() {
            render_thread(display, attached);
        }
    };
    match crate::task::kthread::kthread_run(render, "console") {
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
    text: Vec<grid::Cell>,
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

/// The foreground terminal's view as the render thread last copied it.
struct Snapshot {
    cols: usize,
    rows: usize,
    cells: Vec<grid::Cell>,
    cursor_visible: bool,
    cursor: (usize, usize),
    generation: u64,
    /// The foreground terminal (0-based) and its acquisition, when copied.
    fg: usize,
    epoch: u64,
}

impl Snapshot {
    fn row(&self, row: usize) -> &[grid::Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// Copy the foreground terminal's view if it, or which terminal is in
    /// the foreground, changed since the last copy. False when neither did or
    /// the lock stayed held. A different terminal sets `REDRAW`, so each of
    /// its rows is drawn.
    fn refresh(&mut self) -> bool {
        let (generation, fg, epoch) = (self.generation, self.fg, self.epoch);
        let cells = &mut self.cells;
        let copied = with_grid(|console| {
            let frame = vt::frame(&vt::Terminals { vts: &mut console.vts, sw: &mut console.sw });
            let grid = &console.vts[frame.fg].grid;
            if grid.generation() == generation && frame.fg == fg && frame.epoch == epoch {
                return None;
            }
            if grid.cols() * grid.rows() != cells.len() {
                return None;
            }
            for row in 0..grid.rows() {
                cells[row * grid.cols()..(row + 1) * grid.cols()].copy_from_slice(grid.view_row(row));
            }
            // The cursor is not drawn while the view is scrolled back.
            let visible = grid.cursor_visible() && grid.view() == 0;
            Some((grid.generation(), grid.cursor(), visible, frame))
        });
        match copied.flatten() {
            Some((generation, cursor, visible, frame)) => {
                if frame.fg != self.fg || frame.epoch != self.epoch {
                    REDRAW.store(true, Ordering::Release);
                }
                self.generation = generation;
                self.cursor = cursor;
                self.cursor_visible = visible;
                self.fg = frame.fg;
                self.epoch = frame.epoch;
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
    let (cols, rows) = with_grid(|console| (console.vts[0].grid.cols(), console.vts[0].grid.rows())).unwrap_or((0, 0));
    let mut snapshot = Snapshot {
        cols,
        rows,
        cells: alloc::vec![grid::BLANK; cols * rows],
        cursor: (0, 0),
        cursor_visible: true,
        generation: u64::MAX,
        fg: usize::MAX,
        epoch: u64::MAX,
    };
    let mut splash_run = attached.splash;
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
    let mut last_session = false;
    while !crate::task::kthread::kthread_should_stop() {
        // Replies are input, independent of which owner currently draws. Retry
        // on a busy/full input ring instead of losing a CPR byte to contention.
        // Each terminal's replies go to its own input queue.
        with_grid(|console| {
            for (index, terminal) in console.vts.iter_mut().enumerate() {
                terminal.replies.drain(|byte| crate::tty::driver::push_reply(index + 1, byte));
            }
        });
        crate::tty::driver::drain_keyboard_input();
        // A switch a key asked for, then switch deadlines, whichever owner draws.
        vt::take_key_switch();
        vt::tick();
        if splash::is_up() {
            PRESENTING.store(true, Ordering::SeqCst);
            splash_run.pass(display, &mut screen.shadow);
            answer_checks(Err(Absent::NotYet), false);
            end_pass();
            sleep_ms(PASS_INTERVAL_MS);
            continue;
        }
        if !SCREEN.load(Ordering::Acquire) {
            answer_checks(Err(Absent::NotYet), false);
            sleep_ms(PASS_INTERVAL_MS);
            continue;
        }
        // Publish before checking ownership. A taker either observes this
        // pass and waits, or this pass observes the take and submits nothing.
        PRESENTING.store(true, Ordering::SeqCst);
        if !may_draw() {
            // A completed text acquisition can change KD mode before this pass.
            // Record an unavailable read-back verdict rather than strand the next switch.
            if let Some(shown) = vt::readback_pending() {
                report_switch(shown + 1, &screen.check(shown));
                vt::checked(shown);
            }
            end_pass();
            answer_checks(Err(Absent::Taken), true);
            // A switch waits on these passes: look again soon while one does.
            sleep_ms(if vt::switching() { PASS_INTERVAL_MS } else { PASS_INTERVAL_MS * 10 });
            continue;
        }
        // A display owner whose terminal just left the foreground may still be
        // finishing a presentation: draw over the screen once it has.
        if vt::wait_for_presentations() {
            end_pass();
            sleep_ms(1);
            continue;
        }
        let changed = snapshot.refresh();
        if REDRAW.swap(false, Ordering::AcqRel) {
            screen.cleared = false;
            screen.complete = false;
            screen.shown.fill(None);
        }
        if changed || !screen.complete {
            let started = crate::time::get_monotonic_time_ns();
            let drawn = screen.update(&snapshot);
            if screen.complete {
                vt::drawn(snapshot.epoch);
            }
            if !announced && screen.complete {
                announced = true;
                let elapsed = elapsed_us(started);
                say!(
                    info,
                    "{} console tty and kernel log on the screen: {}x{} cells, {} rows drawn in {}.{:03} ms (diag write {} us)",
                    TAG,
                    cols,
                    rows,
                    drawn,
                    elapsed / 1000,
                    elapsed % 1000,
                    DIAG_WRITE_US.load(Ordering::Relaxed)
                );
                // x86 reads the screen back at kernel idle; ARM64 once the
                // console first fills it.
                #[cfg(target_arch = "aarch64")]
                request_idle_check();
            }
        }
        let session = SESSION.load(Ordering::Acquire);
        if session != last_session {
            if session {
                say!(info, "{} the console has a session: only kernel warnings and errors are drawn", TAG);
            } else {
                say!(info, "{} the console's session ended: the kernel log is drawn again", TAG);
            }
            last_session = session;
        }
        if let Some(shown) = vt::readback_pending() {
            if shown == snapshot.fg && screen.complete {
                report_switch(shown + 1, &screen.check(snapshot.fg));
                vt::checked(shown);
            }
        }
        if announced {
            answer_checks(Ok((&screen, snapshot.fg)), true);
        }
        end_pass();
        sleep_ms(PASS_INTERVAL_MS);
    }
}

/// The splash state shared in turn by the boot and render threads: its two
/// read-backs and the hand-over.
struct SplashRun {
    painter: splash::Painter,
    /// The screen shows the whole shadow.
    shown: bool,
    /// What the first read-back saw, once it matched.
    first: Option<splash::View>,
    progress_checked: bool,
    /// A read-back differed: no more are made.
    failed: bool,
    /// The hand-over waits until then for the progress read-back.
    wait_until: Option<u64>,
}

/// How long a hand-over waits for the cursor's other phase, so the progress
/// read-back can still be made: half a blink and a pass.
const PROGRESS_WAIT_MS: u64 = 520;

impl SplashRun {
    fn pass(&mut self, display: &Display, shadow: &mut [u8]) {
        if !may_draw() {
            #[cfg(target_arch = "aarch64")]
            if crate::graphics::boot_screen::diagnostics_up() {
                self.hand_over(splash::Reason::Panic);
                return;
            }
            self.hand_over(splash::requested().unwrap_or(splash::Reason::DisplayTaken));
            return;
        }
        splash::check_backstop();
        self.shown = present_splash(display, shadow, &mut self.painter);
        if self.shown && !self.failed {
            self.read_back(display, shadow);
        }
        let Some(reason) = splash::requested() else {
            return;
        };
        if self.progress_pending() {
            let now = splash::now_ms();
            let until = *self.wait_until.get_or_insert(now + PROGRESS_WAIT_MS);
            if now < until {
                return;
            }
        }
        self.hand_over(reason);
    }

    /// The progress read-back can still be made: the bar has moved since the
    /// first, and the cursor's other phase is at most half a blink away.
    fn progress_pending(&self) -> bool {
        match (self.first, self.painter.drawn()) {
            (Some(first), Some(now)) => {
                !self.failed && !self.progress_checked && now.reached != first.reached && now.bar_units() != first.bar_units()
            }
            _ => false,
        }
    }

    /// The first read-back once the splash is shown, and the progress
    /// read-back once the stages reached, the bar's width and the cursor's
    /// phase each differ from the first's.
    fn read_back(&mut self, display: &Display, shadow: &[u8]) {
        let Some(view) = self.painter.drawn() else {
            return;
        };
        let due = match self.first {
            None => true,
            Some(first) => {
                !self.progress_checked
                    && view.reached != first.reached
                    && view.bar_units() != first.bar_units()
                    && view.lit != first.lit
            }
        };
        if !due {
            return;
        }
        let Some(readback) = self.painter.check(display, shadow, may_draw) else {
            return;
        };
        if readback.source.is_err() && !may_draw() {
            // The display changed hands during the read, so it compared no pixels.
            return;
        }
        if !readback.identical() {
            readback.report_difference();
            self.failed = true;
            return;
        }
        let source = readback.source.as_ref().ok();
        match self.first {
            None => {
                log::info!(
                    "[splash] logo read back: shadow and screen identical, {} of {} logo probes match the oracle, stages {} of {}, bar {} units, cursor {}, sha256 {}; {}",
                    readback.probes - readback.mismatches,
                    readback.probes,
                    view.completed(),
                    view.total(),
                    view.bar_units(),
                    view.cursor(),
                    Hex(&readback.sha256),
                    ReadFrom(source)
                );
                self.first = Some(readback.view);
            }
            Some(first) => {
                log::info!(
                    "[splash] progress read back: shadow and screen identical, {} of {} logo probes match the oracle, stages {} -> {} of {}, bar {} -> {} units, cursor {} -> {}, sha256 {}; {}",
                    readback.probes - readback.mismatches,
                    readback.probes,
                    first.completed(),
                    view.completed(),
                    view.total(),
                    first.bar_units(),
                    view.bar_units(),
                    first.cursor(),
                    view.cursor(),
                    Hex(&readback.sha256),
                    ReadFrom(source)
                );
                self.progress_checked = true;
            }
        }
    }

    /// Give tty1's text the screen.
    fn hand_over(&mut self, reason: splash::Reason) {
        splash::end();
        REDRAW.store(true, Ordering::Release);
        SCREEN.store(true, Ordering::Release);
        log::info!("[splash] console took the screen: {}", reason.name());
    }
}

/// What a read-back read, for its log line.
struct ReadFrom<'a>(Option<&'a Source>);

impl fmt::Display for ReadFrom<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(source) => write!(f, "{}", source),
            None => Ok(()),
        }
    }
}

static DIAG_WRITE_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static DIAG_READ_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

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
                && may_draw()
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
                cursor: (snapshot.cursor_visible && cursor_row == row).then_some(cursor_col),
            };
            if self.shown[row].as_ref() == Some(&wanted) {
                continue;
            }
            raster::draw_row(&mut self.shadow, &self.layout, row, &wanted.text, wanted.cursor);
            let band = self.layout.band(row);
            let t0 = crate::time::get_monotonic_time_ns();
            let wrote = self.display.write(&self.shadow, band.clone(), may_draw);
            DIAG_WRITE_US.fetch_add(elapsed_us(t0), Ordering::Relaxed);
            if wrote {
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
            if !may_draw() || self.display.flush(lines.clone()).is_err() {
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
    fn check(&self, fg: usize) -> Check {
        let layout = self.layout;
        let bpp = layout.bytes_per_pixel;
        let font = layout.font;
        let source = if may_draw() { self.display.settle() } else { Err("display ownership changed") };
        let mut line = alloc::vec![0u8; layout.width * bpp];
        let mut differing_lines = 0;
        let mut first_difference = None;
        let mut font_mismatches = 0;
        if source.is_ok() {
            for y in 0..layout.height {
                let bytes = layout.line(y);
                let t0 = crate::time::get_monotonic_time_ns();
                self.display.read_line(y, &mut line);
                DIAG_READ_US.fetch_add(elapsed_us(t0), Ordering::Relaxed);
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
                            raster::expected_colour(font, &shown.text, shown.cursor, x, y % font.line_height())
                        }
                        None => raster::PALETTE[0],
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
            .map(|row| row.text.iter().filter(|&&cell| cell.glyph != b' ' as u16).count())
            .sum();
        let rows = shown
            .iter()
            .map(|row| {
                let end = row.text.iter().rposition(|&cell| cell.glyph != b' ' as u16).map_or(0, |last| last + 1);
                row.text[..end].to_vec()
            })
            .filter(|text| !text.is_empty())
            .collect();
        Check {
            fg,
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
    /// The foreground terminal (0-based) it read.
    fg: usize,
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
    rows: Vec<Vec<grid::Cell>>,
}

impl Check {
    fn identical(&self) -> bool {
        self.source.is_ok() && self.complete && self.differing_lines == 0 && self.font_mismatches == 0
    }
}

/// Why a check had no screen to read.
#[derive(Clone, Copy)]
enum Absent {
    /// The boot splash still has the display.
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
/// screen is what `shown` says it is. The idle read-back waits for a pass that
/// may answer it (`idle_allowed`): it reads the console's text, not the splash.
fn answer_checks(screen: Result<(&Screen, usize), Absent>, idle_allowed: bool) {
    let idle = idle_allowed && IDLE_CHECK.swap(false, Ordering::AcqRel);
    let token = PROC_CHECK.swap(0, Ordering::AcqRel);
    if !idle && token == 0 {
        return;
    }
    DIAG_READ_US.store(0, Ordering::Relaxed);
    let t0 = crate::time::get_monotonic_time_ns();
    let answer = screen.map(|(screen, fg)| screen.check(fg));
    if idle {
        log::info!("[console] diag check {} us, read {} us", elapsed_us(t0), DIAG_READ_US.load(Ordering::Relaxed));
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
            Err(Absent::NotYet) => say!(info, "{} screen not read back: the boot splash has the display", TAG),
            Err(Absent::Taken) => say!(info, "{} screen not read back: a process has the display", TAG),
        }
    }
    if token != 0 {
        *PROC_RESULT.lock() = Some(answer);
        PROC_DONE.complete(token);
    }
}

/// After a switch, the read-back of the new terminal's frame, logged with
/// the terminal number and a digest of the shadow, not the screen's text
/// (contract section 8).
fn report_switch(number: usize, check: &Check) {
    if check.identical() {
        log::info!("[console] tty{} shown: read back identical; digest {}", number, Hex(&check.sha256[..8]));
        return;
    }
    match (&check.source, check.first_difference) {
        (Err(reason), _) => log::warn!("[console] tty{} shown: read back differs: not read from the scanout: {}", number, reason),
        (Ok(_), Some((x, y))) => log::warn!("[console] tty{} shown: read back differs: x={} y={}", number, x, y),
        (Ok(_), None) if !check.complete => log::warn!("[console] tty{} shown: read back differs: frame incomplete", number),
        (Ok(_), None) => log::warn!(
            "[console] tty{} shown: read back differs: {} pixels from the font",
            number,
            check.font_mismatches
        ),
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

fn push_rows(text: &mut String, prefix: &str, rows: &[Vec<grid::Cell>]) {
    for row in rows {
        text.push_str(prefix);
        text.extend(row.iter().map(|cell| cell.character()));
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
        Some(Err(Absent::NotYet)) => text.push_str("console check: the boot splash has the display\n"),
        Some(Err(Absent::Taken)) => text.push_str("console check: a process has the display\n"),
        Some(Ok(check)) => {
            let layout = check.layout;
            let font = layout.font;
            let _ = writeln!(text, "console foreground: tty{}", check.fg + 1);
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
