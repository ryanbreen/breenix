//! The boot splash: the Breenix logo and boot progress, drawn on the screen as
//! soon as the target has one (`docs/console/contract.md`, sections 6 and 7).
//!
//! The splash is tty1 in a kernel-owned graphics state. `stage()` records a
//! kernel boot stage in a lock-free mask from the first instruction, so stages
//! reached before the screen exists show as reached when it is first drawn.
//! The logo, the progress bar and the six milestone dots come from
//! `breenix_logo`; the stage name, the boot mode and the status line are drawn
//! here in the console font.
//!
//! Until the console's render thread runs, the boot thread draws the splash
//! into the console's shadow and copies the rows that changed to the screen
//! (`console::splash_boot_pass`). From then on the render thread owns it: it
//! blinks the cursor, follows the progress, reads the screen back twice
//! (`Painter::check`) and hands the screen to tty1's text on the first give-way
//! trigger (`request`).

use super::display::{Display, Source};
use super::raster::Layout;
use breenix_logo::{self as logo, Frame, Milestone, PixelFormat, Surface};
use core::fmt::Write;
use core::ops::Range;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Stages. Names match `docs/boot-path.json`; `MILESTONES` gives the first
// stage of each of the six milestones the dots show.
// ---------------------------------------------------------------------------

/// ARM64's kernel boot stages, in boot order.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    KernelStarting,
    RunningAtEl1,
    MmuEnabled,
    MemoryReady,
    TimerCalibrated,
    RtcRead,
    GicInitialized,
    UartInterrupts,
    InterruptsEnabled,
    DriversInitialized,
    DriverSelfTests,
    Ext2Mounted,
    Devfs,
    Devpts,
    Procfs,
    Tty,
    PerCpu,
    ProcessManager,
    Scheduler,
    TimerInterrupt,
    SmpOnline,
    AllCpusOnline,
    BootComplete,
    StartingPid1,
}

#[cfg(target_arch = "aarch64")]
const STAGE_NAMES: [&str; 24] = [
    "ARM64 kernel starting",
    "Running at EL1",
    "MMU enabled",
    "Memory management ready",
    "Timer calibrated",
    "RTC read",
    "GIC initialized",
    "UART interrupts enabled",
    "Interrupts enabled",
    "Device drivers initialized",
    "Driver self-tests ran",
    "ext2 root filesystem mounted",
    "devfs initialized",
    "devptsfs initialized",
    "procfs initialized",
    "TTY subsystem initialized",
    "Per-CPU data initialized",
    "Process manager initialized",
    "Scheduler initialized",
    "Timer interrupt initialized",
    "SMP CPUs online",
    "All reported CPUs online",
    "ARM64 boot complete",
    "Starting PID 1",
];

#[cfg(target_arch = "aarch64")]
const MILESTONES: [usize; 7] = [0, 4, 9, 11, 16, 23, 24];

/// x86-64's splash stages: catalog stages `main.rs` reaches at each milestone's
/// boundaries.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    KernelEntry,
    SerialPort,
    GdtIdt,
    Heap,
    InterruptController,
    Timer,
    PciEnumerated,
    NetworkStack,
    Procfs,
    Threading,
    ProcessManagement,
    KernelInitComplete,
    StartingPid1,
}

#[cfg(target_arch = "x86_64")]
const STAGE_NAMES: [&str; 13] = [
    "Kernel entry point",
    "Serial port initialized",
    "GDT and IDT initialized",
    "Kernel heap initialized",
    "Interrupt controller initialized",
    "Timer initialized",
    "PCI bus enumerated",
    "Network stack initialized",
    "procfs initialized",
    "Threading subsystem ready",
    "Process management ready",
    "Kernel initialization complete",
    "Starting PID 1",
];

#[cfg(target_arch = "x86_64")]
const MILESTONES: [usize; 7] = [0, 4, 6, 8, 9, 12, 13];

const STAGE_COUNT: usize = STAGE_NAMES.len();
const ALL_STAGES: u32 = (1 << STAGE_COUNT) - 1;

/// Bit i set: stage i reached.
static REACHED: AtomicU32 = AtomicU32::new(0);
/// The stage reached last, or `u8::MAX` before the first.
static LATEST: AtomicU8 = AtomicU8::new(u8::MAX);

/// Record that the boot reached `stage`, and bring the splash up to date if
/// the boot thread still draws it. Call from thread context.
pub fn stage(stage: Stage) {
    REACHED.fetch_or(1 << stage as u32, Ordering::AcqRel);
    LATEST.store(stage as u8, Ordering::Release);
    super::splash_boot_pass();
}

/// The name of the stage the boot was in: the first stage not yet reached.
pub fn current_stage_name() -> &'static str {
    let reached = REACHED.load(Ordering::Acquire);
    match (0..STAGE_COUNT).find(|&i| reached & (1 << i) == 0) {
        Some(i) => STAGE_NAMES[i],
        None => "boot complete, userspace running",
    }
}

// ---------------------------------------------------------------------------
// Text lines: the boot mode, the status line and the last stage's label.
// ---------------------------------------------------------------------------

const TEXT_CAP: usize = 112;

/// Small fixed-capacity text: formatting into it does not allocate.
#[derive(Clone, Copy)]
pub(crate) struct Text {
    buf: [u8; TEXT_CAP],
    len: usize,
}

impl Text {
    pub(crate) const fn new() -> Self {
        Self { buf: [0; TEXT_CAP], len: 0 }
    }

    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, b: u8) {
        if self.len < TEXT_CAP {
            self.buf[self.len] = b;
            self.len += 1;
        }
    }
}

impl Write for Text {
    /// Printable ASCII is kept; dashes and arrows become `-` and `->`, and any
    /// other character becomes a single `?`.
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            let replacement = match c {
                ' '..='~' => {
                    self.push(c as u8);
                    continue;
                }
                '\u{2010}'..='\u{2015}' | '\u{2212}' => "-",
                '\u{2192}' | '\u{27f6}' => "->",
                _ => "?",
            };
            for &b in replacement.as_bytes() {
                self.push(b);
            }
        }
        Ok(())
    }
}

struct Texts {
    mode: Text,
    status: Text,
    last_label: Text,
}

static TEXTS: Mutex<Texts> = Mutex::new(Texts { mode: Text::new(), status: Text::new(), last_label: Text::new() });
/// Bumped whenever a text line changes.
static TEXT_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Attempts at the text lock before a text update is dropped. A painter holds
/// it while it draws one text row; the lock is not waited on unboundedly,
/// because its holder may be preempted on the CPU that waits.
const TEXT_SPINS: usize = 100_000;

fn set_text(update: impl FnOnce(&mut Texts)) {
    for _ in 0..TEXT_SPINS {
        if let Some(mut texts) = TEXTS.try_lock() {
            update(&mut texts);
            TEXT_GENERATION.fetch_add(1, Ordering::AcqRel);
            break;
        }
        core::hint::spin_loop();
    }
    super::splash_boot_pass();
}

/// Set the boot mode, shown as `Boot mode: <mode>`.
pub fn set_mode(args: core::fmt::Arguments) {
    set_text(|texts| {
        texts.mode = Text::new();
        let _ = texts.mode.write_fmt(args);
    });
}

/// Set the status line: what the boot does next.
pub fn set_status(args: core::fmt::Arguments) {
    set_text(|texts| {
        texts.status = Text::new();
        let _ = texts.status.write_fmt(args);
    });
}

/// Change the label of the last stage (a testing kernel starts test programs, not PID 1).
pub fn set_last_stage_label(label: &str) {
    set_text(|texts| {
        texts.last_label = Text::new();
        let _ = texts.last_label.write_str(label);
    });
}

// ---------------------------------------------------------------------------
// Giving way to the console.
// ---------------------------------------------------------------------------

/// Why the splash gave way (contract section 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Reason {
    Reader = 1,
    KdText,
    Key,
    DisplayTaken,
    Panic,
    Backstop,
}

impl Reason {
    fn from_u8(value: u8) -> Option<Reason> {
        Some(match value {
            1 => Reason::Reader,
            2 => Reason::KdText,
            3 => Reason::Key,
            4 => Reason::DisplayTaken,
            5 => Reason::Panic,
            6 => Reason::Backstop,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Reason::Reader => "reader",
            Reason::KdText => "kd-text",
            Reason::Key => "key",
            Reason::DisplayTaken => "display-taken",
            Reason::Panic => "panic",
            Reason::Backstop => "backstop",
        }
    }
}

const UP: u8 = 0;
const ENDED: u8 = 1;

/// Whether the splash still has tty1: `UP` until the render thread hands over.
static STATE: AtomicU8 = AtomicU8::new(UP);
/// The first give-way trigger, or 0.
static REQUESTED: AtomicU8 = AtomicU8::new(0);
/// Monotonic milliseconds plus one when the first process started, or 0.
static FIRST_PROCESS_MS: AtomicU64 = AtomicU64::new(0);

/// The backstop: the console takes the screen this long after the first
/// process starts, unless another trigger came first.
const BACKSTOP_MS: u64 = 10_000;

/// A testing kernel keeps the splash until the backstop or the end of the run:
/// only the backstop, a display owner and a panic end it there.
const TESTING: bool = cfg!(any(feature = "testing", feature = "boot_tests"));

pub fn is_up() -> bool {
    STATE.load(Ordering::Acquire) == UP
}

/// Ask for the hand-over. The first trigger wins; later ones have no effect.
/// Lock-free, so it may be called from interrupt context; the render thread
/// makes the hand-over and logs it.
pub fn request(reason: Reason) {
    if !is_up() {
        return;
    }
    if TESTING && matches!(reason, Reason::Reader | Reason::KdText | Reason::Key) {
        return;
    }
    let _ = REQUESTED.compare_exchange(0, reason as u8, Ordering::AcqRel, Ordering::Acquire);
}

/// The trigger the render thread is to act on.
pub(super) fn requested() -> Option<Reason> {
    Reason::from_u8(REQUESTED.load(Ordering::Acquire))
}

/// A process is reading from tty1.
pub fn reader() {
    if is_up() {
        request(Reason::Reader);
    }
}

/// A key was pressed on the keyboard. True when the splash consumes it: the
/// byte that requests the hand-over is not delivered; subsequent bytes are.
pub fn key() -> bool {
    if TESTING || !is_up() {
        return false;
    }
    REQUESTED.compare_exchange(0, Reason::Key as u8, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// The first process started: the backstop runs from here.
pub fn first_process_started() {
    let now = now_ms().saturating_add(1);
    let _ = FIRST_PROCESS_MS.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire);
}

/// Ask for the backstop hand-over once it is due.
pub(super) fn check_backstop() {
    let started = FIRST_PROCESS_MS.load(Ordering::Acquire);
    if started != 0 && now_ms().saturating_add(1) >= started.saturating_add(BACKSTOP_MS) {
        request(Reason::Backstop);
    }
}

/// The render thread handed over: the splash is gone for good.
pub(super) fn end() {
    STATE.store(ENDED, Ordering::Release);
}

pub(super) fn now_ms() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1000).saturating_add(nanos / 1_000_000)
}

/// Monotonic milliseconds plus one when the splash was first drawn, or 0: the
/// cursor blinks against it.
static EPOCH_MS: AtomicU64 = AtomicU64::new(0);

/// Whether the cursor block is lit now.
pub(super) fn cursor_lit(now: u64) -> bool {
    let epoch = EPOCH_MS.load(Ordering::Acquire);
    epoch == 0 || logo::cursor_visible(now.saturating_add(1).saturating_sub(epoch), is_up())
}

// ---------------------------------------------------------------------------
// Drawing.
// ---------------------------------------------------------------------------

const FIELD: [u8; 3] = [logo::BACKGROUND.0, logo::BACKGROUND.1, logo::BACKGROUND.2];
const DIM: [u8; 3] = [110, 120, 145];

/// What the splash shows: the stages reached and the cursor's phase.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct View {
    pub reached: u32,
    pub lit: bool,
}

impl View {
    pub fn now(now: u64) -> View {
        View { reached: REACHED.load(Ordering::Acquire) & ALL_STAGES, lit: cursor_lit(now) }
    }

    pub fn completed(&self) -> u16 {
        self.reached.count_ones() as u16
    }

    pub fn total(&self) -> u16 {
        STAGE_COUNT as u16
    }

    /// The bar's fill in design units (contract section 6.2).
    pub fn bar_units(&self) -> u32 {
        176 * u32::from(self.completed()) / u32::from(self.total())
    }

    pub fn cursor(&self) -> &'static str {
        if self.lit { "lit" } else { "dark" }
    }

    fn frame(&self) -> Frame {
        let mut milestones = [Milestone::Pending; 6];
        for (index, state) in milestones.iter_mut().enumerate() {
            let (first, end) = (MILESTONES[index], MILESTONES[index + 1]);
            let mask = ((1u32 << end) - 1) & !((1u32 << first) - 1);
            *state = match self.reached & mask {
                0 => Milestone::Pending,
                reached if reached == mask => Milestone::Done,
                _ => Milestone::Running,
            };
        }
        Frame { cursor_visible: self.lit, completed: self.completed(), total: self.total(), milestones }
    }
}

/// Up to four bands of pixel rows a pass changed.
pub(super) struct Bands {
    bands: [Range<usize>; 4],
    len: usize,
}

impl Bands {
    fn new() -> Self {
        Self { bands: [0..0, 0..0, 0..0, 0..0], len: 0 }
    }

    fn add(&mut self, rows: Range<usize>) {
        if rows.is_empty() {
            return;
        }
        for band in &mut self.bands[..self.len] {
            if rows.start <= band.end && band.start <= rows.end {
                *band = band.start.min(rows.start)..band.end.max(rows.end);
                return;
            }
        }
        if self.len < self.bands.len() {
            self.bands[self.len] = rows;
            self.len += 1;
        } else {
            let last = &mut self.bands[self.len - 1];
            *last = last.start.min(rows.start)..last.end.max(rows.end);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.bands[..self.len].iter().cloned()
    }
}

/// Where the splash sits on a framebuffer (contract section 6.2).
#[derive(Clone, Copy)]
pub(super) struct Look {
    layout: Layout,
    format: PixelFormat,
    /// Pixels per design unit.
    k: u16,
    /// The composition's top-left pixel; either may be negative.
    left: i32,
    top: i32,
}

impl Look {
    pub fn new(layout: Layout, bgr: bool) -> Self {
        let k = match layout.font.size() {
            32 => 4,
            24 => 3,
            _ => 2,
        };
        let format = match (bgr, layout.bytes_per_pixel) {
            (true, 3) => PixelFormat::Bgr,
            (true, _) => PixelFormat::Bgra,
            (false, 3) => PixelFormat::Rgb,
            (false, _) => PixelFormat::Rgba,
        };
        let (width, height) = (layout.width as i32, layout.height as i32);
        let unit = i32::from(k);
        Self { layout, format, k, left: width / 2 - 88 * unit, top: 2 * height / 5 - 32 * unit }
    }

    fn unit(&self) -> i32 {
        i32::from(self.k)
    }

    /// Pixel rows `from..to` design units below the composition's top, clipped to the screen.
    fn rows(&self, from: i32, to: i32) -> Range<usize> {
        let height = self.layout.height as i32;
        let start = (self.top + from * self.unit()).clamp(0, height) as usize;
        let end = (self.top + to * self.unit()).clamp(0, height) as usize;
        start..end
    }

    /// The badge, as the screen rectangle (x, y, width, height).
    fn badge(&self) -> (i32, i32, i32, i32) {
        (self.left + 56 * self.unit(), self.top, 64 * self.unit(), 64 * self.unit())
    }

    fn text_height(&self) -> usize {
        self.layout.font.line_height()
    }

    fn stage_row(&self) -> i32 {
        self.top + 141 * self.unit()
    }

    fn mode_row(&self) -> i32 {
        self.layout.height as i32 - 3 * self.text_height() as i32
    }

    fn status_row(&self) -> i32 {
        self.layout.height as i32 - 2 * self.text_height() as i32
    }

    fn text_rows(&self, top: i32) -> Range<usize> {
        let height = self.layout.height as i32;
        top.clamp(0, height) as usize..(top + self.text_height() as i32).clamp(0, height) as usize
    }

    /// Write `rgb` into the pixel at byte `offset`.
    fn put(&self, frame: &mut [u8], offset: usize, rgb: [u8; 3]) {
        let pixel = &mut frame[offset..offset + self.layout.bytes_per_pixel];
        let [r, g, b] = rgb;
        let ordered = match self.format {
            PixelFormat::Bgr | PixelFormat::Bgra => [b, g, r],
            PixelFormat::Rgb | PixelFormat::Rgba => [r, g, b],
        };
        pixel[..3].copy_from_slice(&ordered);
        pixel[3..].fill(self.layout.reserved);
    }

    /// The RGB colour of a pixel as written by `put` or the logo renderer.
    fn rgb(&self, pixel: &[u8]) -> [u8; 3] {
        match self.format {
            PixelFormat::Bgr | PixelFormat::Bgra => [pixel[2], pixel[1], pixel[0]],
            PixelFormat::Rgb | PixelFormat::Rgba => [pixel[0], pixel[1], pixel[2]],
        }
    }

    fn fill_rows(&self, frame: &mut [u8], rows: Range<usize>) {
        let bpp = self.layout.bytes_per_pixel;
        for y in rows {
            let line = self.layout.line(y);
            for offset in line.step_by(bpp) {
                self.put(frame, offset, FIELD);
            }
        }
    }

    /// Render the logo composition's pixel rows `rows` for `view`.
    fn logo(&self, frame: &mut [u8], rows: Range<usize>, view: &View) {
        if rows.is_empty() {
            return;
        }
        let stride = self.layout.stride_bytes;
        let end = (rows.end * stride).min(frame.len());
        let Ok(mut surface) = Surface::new(
            &mut frame[rows.start * stride..end],
            self.layout.width,
            rows.len(),
            stride,
            self.format,
        ) else {
            return;
        };
        let _ = logo::render(&mut surface, (self.left, self.top - rows.start as i32), self.k, &view.frame());
    }

    /// Clear the text row at `top` and draw `text` centred on it in the dim colour.
    fn text(&self, frame: &mut [u8], top: i32, text: &str) {
        let rows = self.text_rows(top);
        self.fill_rows(frame, rows.clone());
        let font = self.layout.font;
        let size = match font.size() {
            32 => RasterHeight::Size32,
            24 => RasterHeight::Size24,
            20 => RasterHeight::Size20,
            _ => RasterHeight::Size16,
        };
        let glyph_width = font.glyph_width() as i32;
        let width = self.layout.width as i32;
        let mut x = width / 2 - text.len() as i32 * glyph_width / 2;
        let bpp = self.layout.bytes_per_pixel;
        for c in text.chars() {
            let Some(glyph) = get_raster(c, FontWeight::Regular, size).or_else(|| get_raster('?', FontWeight::Regular, size))
            else {
                continue;
            };
            for (dy, glyph_row) in glyph.raster().iter().enumerate() {
                let y = top + dy as i32;
                if y < rows.start as i32 || y >= rows.end as i32 {
                    continue;
                }
                for (dx, &intensity) in glyph_row.iter().enumerate() {
                    let px = x + dx as i32;
                    if px < 0 || px >= width || intensity == 0 {
                        continue;
                    }
                    let blend = |src: u8, under: u8| {
                        ((u32::from(src) * u32::from(intensity) + u32::from(under) * (255 - u32::from(intensity)) + 127)
                            / 255) as u8
                    };
                    let rgb = [blend(DIM[0], FIELD[0]), blend(DIM[1], FIELD[1]), blend(DIM[2], FIELD[2])];
                    self.put(frame, self.layout.line(y as usize).start + px as usize * bpp, rgb);
                }
            }
            x += glyph_width;
        }
    }

    /// The stage reached last, under the label a testing kernel gives the last stage.
    fn stage_text(&self, frame: &mut [u8], view: &View, texts: &Texts) {
        let name = match LATEST.load(Ordering::Acquire) as usize {
            index if index == STAGE_COUNT - 1 && !texts.last_label.is_empty() => texts.last_label.as_str(),
            index if index < STAGE_COUNT && view.reached & (1 << index) != 0 => STAGE_NAMES[index],
            _ => "",
        };
        self.text(frame, self.stage_row(), name);
    }

    fn mode_text(&self, frame: &mut [u8], texts: &Texts) {
        let mut line = Text::new();
        if !texts.mode.is_empty() {
            let _ = write!(line, "Boot mode: {}", texts.mode.as_str());
        }
        self.text(frame, self.mode_row(), line.as_str());
    }

    /// Draw the whole splash for `view` into `frame`, a framebuffer laid out as
    /// the look's layout. Does not allocate.
    pub fn draw_all(&self, frame: &mut [u8], view: &View) {
        self.fill_rows(frame, 0..self.layout.height);
        self.logo(frame, self.rows(0, logo::HEIGHT as i32), view);
        let texts = TEXTS.lock();
        self.stage_text(frame, view, &texts);
        self.mode_text(frame, &texts);
        self.text(frame, self.status_row(), texts.status.as_str());
    }
}

/// The splash as drawn in the console's shadow.
pub(super) struct Painter {
    look: Look,
    /// What the shadow shows, once drawn.
    drawn: Option<(View, u32)>,
}

impl Painter {
    pub fn new(layout: Layout, bgr: bool) -> Self {
        Self { look: Look::new(layout, bgr), drawn: None }
    }

    /// The view the shadow shows.
    pub fn drawn(&self) -> Option<View> {
        self.drawn.map(|(view, _)| view)
    }

    /// The next pass draws everything again (a write or flush failed).
    pub fn forget(&mut self) {
        self.drawn = None;
    }

    /// Bring the shadow up to date with the boot's progress, the cursor's
    /// phase and the text lines; returns the pixel rows it changed.
    pub fn paint(&mut self, shadow: &mut [u8], now: u64) -> Bands {
        let _ = EPOCH_MS.compare_exchange(0, now.saturating_add(1), Ordering::AcqRel, Ordering::Acquire);
        let view = View::now(now);
        let generation = TEXT_GENERATION.load(Ordering::Acquire);
        let mut bands = Bands::new();
        let look = self.look;
        match self.drawn {
            None => {
                look.draw_all(shadow, &view);
                bands.add(0..look.layout.height);
            }
            Some((drawn, drawn_generation)) => {
                if view.lit != drawn.lit {
                    let rows = look.rows(44, 51);
                    look.logo(shadow, rows.clone(), &view);
                    bands.add(rows);
                }
                if view.reached != drawn.reached {
                    let rows = look.rows(113, 134);
                    look.logo(shadow, rows.clone(), &view);
                    bands.add(rows);
                }
                let texts_changed = generation != drawn_generation;
                if view.reached == drawn.reached && !texts_changed {
                    self.drawn = Some((view, generation));
                    return bands;
                }
                let texts = TEXTS.lock();
                look.stage_text(shadow, &view, &texts);
                bands.add(look.text_rows(look.stage_row()));
                if texts_changed {
                    look.mode_text(shadow, &texts);
                    look.text(shadow, look.status_row(), texts.status.as_str());
                    bands.add(look.text_rows(look.mode_row()).start..look.text_rows(look.status_row()).end);
                }
            }
        }
        self.drawn = Some((view, generation));
        bands
    }

    /// Say what was first drawn: the logo's place and the progress shown.
    pub fn announce(&self) {
        let Some(view) = self.drawn() else {
            return;
        };
        let layout = self.look.layout;
        let (x, y, width, height) = self.look.badge();
        log::info!(
            "[splash] logo drawn: {}x{}, font {}, k={}, badge {}x{} at ({}, {})",
            layout.width,
            layout.height,
            layout.font.size(),
            self.look.k,
            width,
            height,
            x,
            y
        );
        let frame = view.frame();
        let mut dots = Text::new();
        for (index, milestone) in frame.milestones.iter().enumerate() {
            let _ = write!(
                dots,
                "{}{}",
                if index == 0 { "" } else { "," },
                match milestone {
                    Milestone::Done => "done",
                    Milestone::Running => "running",
                    Milestone::Pending => "pending",
                }
            );
        }
        log::info!(
            "[splash] progress shown: {} of {} stages, bar {} of 176 units, dots {}",
            view.completed(),
            view.total(),
            view.bar_units(),
            dots.as_str()
        );
    }

    /// Read the screen back: compare it with the shadow, and the logo block's
    /// probe pixels with the logo's oracle for the view drawn (contract section 8).
    pub fn check(&self, display: &Display, shadow: &[u8], allowed: fn() -> bool) -> Option<Readback> {
        let view = self.drawn()?;
        let look = &self.look;
        let layout = look.layout;
        let bpp = layout.bytes_per_pixel;
        let frame = view.frame();
        let source = if allowed() { display.settle() } else { Err("display ownership changed") };
        let mut readback = Readback {
            view,
            source,
            differing_lines: 0,
            first_difference: None,
            probes: 0,
            mismatches: 0,
            first_mismatch: None,
            sha256: super::sha256::sha256(shadow),
        };
        if readback.source.is_err() {
            return Some(readback);
        }
        let k = i32::from(look.k);
        // One row through the B's upper bowl, one through the wordmark.
        let b_row = 23 * k + k / 2;
        let word_row = 92 * k + k / 2;
        let mut line = alloc::vec![0u8; layout.width * bpp];
        for y in 0..layout.height {
            display.read_line(y, &mut line);
            let bytes = layout.line(y);
            if line[..] != shadow[bytes.clone()] {
                readback.differing_lines += 1;
                if readback.first_difference.is_none() {
                    let x = line.chunks_exact(bpp).zip(shadow[bytes].chunks_exact(bpp)).position(|(a, b)| a != b);
                    readback.first_difference = Some((x.unwrap_or(0), y));
                }
            }
            let local_y = y as i32 - look.top;
            if local_y < 0 || local_y >= logo::HEIGHT as i32 * k {
                continue;
            }
            let whole_row = local_y == b_row || local_y == word_row;
            if !whole_row && local_y % 8 != 0 {
                continue;
            }
            for (x, pixel) in line.chunks_exact(bpp).enumerate() {
                let local_x = x as i32 - look.left;
                if local_x < 0 || local_x >= logo::WIDTH as i32 * k || (!whole_row && local_x % 8 != 0) {
                    continue;
                }
                let Some(expected) = logo::oracle::expected_at(local_x, local_y, look.k, &frame) else {
                    continue;
                };
                readback.probes += 1;
                let logo::Color(r, g, b) = expected.color;
                if look.rgb(pixel) != [r, g, b] {
                    readback.mismatches += 1;
                    if readback.first_mismatch.is_none() {
                        readback.first_mismatch = Some((x, y));
                    }
                }
            }
        }
        Some(readback)
    }
}

/// One read-back of the splash.
pub(super) struct Readback {
    pub view: View,
    /// What was read, or why the read could not be tied to the scanout.
    pub source: Result<Source, &'static str>,
    pub differing_lines: usize,
    pub first_difference: Option<(usize, usize)>,
    /// Logo pixels compared with the oracle, those that differ, and the first.
    pub probes: usize,
    pub mismatches: usize,
    pub first_mismatch: Option<(usize, usize)>,
    pub sha256: [u8; 32],
}

impl Readback {
    pub fn identical(&self) -> bool {
        self.source.is_ok() && self.differing_lines == 0 && self.mismatches == 0
    }

    /// Why it is not identical, for the `differs` line.
    pub fn report_difference(&self) {
        match (&self.source, self.first_difference, self.first_mismatch) {
            (Err(reason), _, _) => log::warn!("[splash] logo read back differs: not read from the scanout: {}", reason),
            (Ok(_), Some((x, y)), _) => log::warn!(
                "[splash] logo read back differs: {} pixel rows differ from the shadow, the first at x={} y={}; {} of {} probes differ from the oracle",
                self.differing_lines,
                x,
                y,
                self.mismatches,
                self.probes
            ),
            (Ok(_), None, Some((x, y))) => log::warn!(
                "[splash] logo read back differs: the shadow and the screen match, but {} of {} probes differ from the oracle, the first at x={} y={}",
                self.mismatches,
                self.probes,
                x,
                y
            ),
            (Ok(_), None, None) => {}
        }
    }
}
