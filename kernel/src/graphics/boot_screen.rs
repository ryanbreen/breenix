//! Boot screen and diagnostics screen (ARM64).
//!
//! As soon as the framebuffer exists the kernel draws a boot screen: a title, the
//! boot mode, a progress bar and the kernel boot stages of docs/boot-path.json
//! (milestones 1-5 plus "Starting PID 1"). `stage()` records a stage in a lock-free
//! mask from anywhere on the boot path; stages reached before the framebuffer
//! existed are simply already in the mask when the screen is first drawn.
//!
//! Drawing happens only on the boot thread (not from interrupt context), redraws
//! only rows whose state changed, and takes the framebuffer with `try_lock`: a
//! contended update is skipped and caught up by the next one. Once userspace takes
//! the display (`stop_drawing`) the kernel stops drawing; when userspace actually
//! draws (`hand_over`) the kernel's screen is cleared to a plain background for it.
//! If userspace does not draw, the boot screen stays up.
//!
//! On a panic or fatal EL1 fault `show_panic`/`show_fault` draw a red diagnostics
//! screen with the failure, the boot stage and the last log lines. They do not
//! block on a lock: if the framebuffer lock is held the screen is skipped and the
//! report is on serial alone; if the GPU lock stays held for about 100 ms
//! (possibly by the failing CPU) the screen is left marked dirty for the render
//! thread, or on VirGL the presenter thread (`spawn_presenter`), to present.

#![cfg(target_arch = "aarch64")]

use super::arm64_fb;
use super::primitives::{draw_char, draw_rect, fill_rect, Canvas, Color, Rect, TextStyle};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;

/// Kernel boot stages, in boot order. Names match docs/boot-path.json.
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
    BootComplete,
    StartingPid1,
}

const STAGE_COUNT: usize = 23;

const STAGE_NAMES: [&str; STAGE_COUNT] = [
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
    "ARM64 boot complete",
    "Starting PID 1",
];

/// Milestone groups: (title, first stage, stage count).
const GROUPS: [(&str, usize, usize); 6] = [
    ("Arch bring-up", 0, 4),
    ("Interrupts & time", 4, 5),
    ("Devices", 9, 2),
    ("Filesystems & console", 11, 5),
    ("Scheduler, timer tick & SMP", 16, 6),
    ("First userspace process", 22, 1),
];

/// Bit i set = stage i reached.
static REACHED: AtomicU32 = AtomicU32::new(0);
/// The boot screen has been drawn and its pixels are still on screen.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The fbconsole=log console was drawing and its pixels are still on screen.
static LOG_CONSOLE_UP: AtomicBool = AtomicBool::new(false);
/// Userspace has taken the display; the kernel no longer draws the boot screen.
static HANDED_OVER: AtomicBool = AtomicBool::new(false);
/// Userspace has drawn and the kernel's screen has been replaced by the split layout.
static LAYOUT_SETTLED: AtomicBool = AtomicBool::new(false);
/// A boot-screen GPU flush failed (a stalled or broken GPU). Later updates are
/// still drawn, but their flush is left to the render thread, so a stalled GPU
/// costs the boot path one command timeout rather than one per update.
static GPU_FLUSH_FAILED: AtomicBool = AtomicBool::new(false);
/// The diagnostics screen has been claimed (first failure wins).
static DIAG_CLAIMED: AtomicBool = AtomicBool::new(false);

const BG: Color = Color::rgb(15, 20, 35);
const TITLE: Color = Color::rgb(235, 240, 250);
const DIM: Color = Color::rgb(130, 145, 170);
const HEADING: Color = Color::rgb(110, 168, 212);
const DONE: Color = Color::rgb(70, 200, 120);
const RUNNING: Color = Color::rgb(240, 190, 60);
const PENDING: Color = Color::rgb(75, 85, 105);
const BAR_BG: Color = Color::rgb(40, 50, 75);

const LINE: i32 = 20;
const TEXT_CAP: usize = 112;

/// Small fixed-capacity text buffer: formatting into it never allocates.
#[derive(Clone, Copy)]
struct Text {
    buf: [u8; TEXT_CAP],
    len: usize,
}

impl Text {
    const fn new() -> Self {
        Self {
            buf: [0; TEXT_CAP],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
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

struct Screen {
    /// Stage mask as last drawn.
    drawn: u32,
    /// Stage index drawn as "running", or STAGE_COUNT for none.
    drawn_running: usize,
    mode: Text,
    mode_dirty: bool,
    status: Text,
    status_dirty: bool,
    last_label: Text,
}

static SCREEN: Mutex<Screen> = Mutex::new(Screen {
    drawn: 0,
    drawn_running: usize::MAX,
    mode: Text::new(),
    mode_dirty: false,
    status: Text::new(),
    status_dirty: false,
    last_label: Text::new(),
});

/// Geometry derived from the framebuffer size.
struct Layout {
    width: i32,
    height: i32,
    left: i32,
    col_width: i32,
}

impl Layout {
    fn of(canvas: &impl Canvas) -> Self {
        let width = canvas.width() as i32;
        let height = canvas.height() as i32;
        let content = (width - 80).clamp(320, 840);
        Self {
            width,
            height,
            left: (width - content) / 2,
            col_width: content / 2,
        }
    }

    fn title_y(&self) -> i32 {
        48
    }
    fn mode_y(&self) -> i32 {
        128
    }
    fn bar_y(&self) -> i32 {
        160
    }
    fn bar_width(&self) -> i32 {
        self.col_width * 2 - 16
    }
    fn stages_y(&self) -> i32 {
        200
    }
    fn status_y(&self) -> i32 {
        self.height - 72
    }

    /// Top-left of stage `index`'s row. Groups 0-2 fill the left column, 3-5 the right.
    fn stage_pos(&self, index: usize) -> (i32, i32) {
        let (col, first_group) = if index < GROUPS[3].1 { (0, 0) } else { (1, 3) };
        let mut y = self.stages_y();
        for group in &GROUPS[first_group..first_group + 3] {
            y += LINE + 4;
            if index < group.1 + group.2 {
                return (
                    self.left + col * self.col_width,
                    y + (index - group.1) as i32 * LINE,
                );
            }
            y += group.2 as i32 * LINE + 8;
        }
        (self.left, y)
    }
}

/// Record that the boot reached `stage`, and redraw the boot screen if it is up.
///
/// Call only from the boot path (never from interrupt context).
pub fn stage(stage: Stage) {
    REACHED.fetch_or(1 << stage as u32, Ordering::AcqRel);
    refresh();
}

/// Set the boot mode shown under the title.
pub fn set_mode(args: core::fmt::Arguments) {
    if let Some(mut screen) = SCREEN.try_lock() {
        screen.mode = Text::new();
        let _ = screen.mode.write_fmt(args);
        screen.mode_dirty = true;
    }
    refresh();
}

/// Set the status line near the bottom of the boot screen.
pub fn set_status(args: core::fmt::Arguments) {
    if let Some(mut screen) = SCREEN.try_lock() {
        screen.status = Text::new();
        let _ = screen.status.write_fmt(args);
        screen.status_dirty = true;
    }
    refresh();
}

/// Change the label of the last stage (a testing kernel starts test programs, not PID 1).
pub fn set_last_stage_label(label: &str) {
    if let Some(mut screen) = SCREEN.try_lock() {
        screen.last_label = Text::new();
        let _ = screen.last_label.write_str(label);
    }
}

/// Draw the boot screen for the first time. Called once the framebuffer exists.
pub fn show() {
    if HANDED_OVER.load(Ordering::Acquire) {
        return;
    }
    let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return;
    };
    let Some(mut screen) = SCREEN.try_lock() else {
        return;
    };
    let Some(mut guard) = fb.try_lock() else {
        return;
    };
    let canvas = &mut *guard;
    let layout = Layout::of(canvas);
    fill_rect(
        canvas,
        Rect {
            x: 0,
            y: 0,
            width: layout.width as u32,
            height: layout.height as u32,
        },
        BG,
    );
    draw_scaled(canvas, layout.left, layout.title_y(), "Breenix", 4, TITLE);
    let style = TextStyle::new().with_color(HEADING).with_background(BG);
    let mut y = layout.stages_y();
    for (g, group) in GROUPS.iter().enumerate() {
        if g == 3 {
            y = layout.stages_y();
        }
        let x = layout.left + if g < 3 { 0 } else { layout.col_width };
        draw_str(canvas, x, y, group.0, &style);
        y += LINE + 4 + group.2 as i32 * LINE + 8;
    }
    screen.drawn = 0;
    screen.drawn_running = usize::MAX;
    screen.mode_dirty = true;
    screen.status_dirty = true;
    draw_changes(canvas, &layout, &mut screen, true);
    present(guard);
    ACTIVE.store(true, Ordering::Release);
}

fn refresh() {
    if !ACTIVE.load(Ordering::Acquire) || HANDED_OVER.load(Ordering::Acquire) {
        return;
    }
    let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return;
    };
    let Some(mut screen) = SCREEN.try_lock() else {
        return;
    };
    let Some(mut guard) = fb.try_lock() else {
        return;
    };
    if HANDED_OVER.load(Ordering::Acquire) {
        return;
    }
    let layout = Layout::of(&*guard);
    draw_changes(&mut *guard, &layout, &mut screen, false);
    present(guard);
}

/// Draw every row whose state differs from what is on screen.
fn draw_changes(canvas: &mut impl Canvas, layout: &Layout, screen: &mut Screen, all: bool) {
    let reached = REACHED.load(Ordering::Acquire);
    let running = (0..STAGE_COUNT)
        .find(|&i| reached & (1 << i) == 0)
        .unwrap_or(STAGE_COUNT);
    for i in 0..STAGE_COUNT {
        let bit = 1u32 << i;
        let changed = (screen.drawn ^ reached) & bit != 0
            || i == running && screen.drawn_running != i
            || i == screen.drawn_running && running != i;
        if !all && !changed {
            continue;
        }
        let (x, y) = layout.stage_pos(i);
        let name = if i == STAGE_COUNT - 1 && screen.last_label.len > 0 {
            screen.last_label.as_str()
        } else {
            STAGE_NAMES[i]
        };
        let mark = Rect {
            x: x + 6,
            y: y + 4,
            width: 9,
            height: 9,
        };
        fill_rect(canvas, mark, BG);
        if reached & bit != 0 {
            fill_rect(canvas, mark, DONE);
        } else if i == running {
            fill_rect(canvas, mark, RUNNING);
        } else {
            draw_rect(canvas, mark, PENDING);
        }
        let text = if reached & bit != 0 {
            TITLE
        } else if i == running {
            RUNNING
        } else {
            DIM
        };
        draw_str(
            canvas,
            x + 24,
            y,
            name,
            &TextStyle::new().with_color(text).with_background(BG),
        );
    }
    if all || screen.drawn != reached {
        let done = reached.count_ones() as i32;
        let bar = Rect {
            x: layout.left,
            y: layout.bar_y(),
            width: layout.bar_width() as u32,
            height: 12,
        };
        fill_rect(canvas, bar, BAR_BG);
        fill_rect(
            canvas,
            Rect {
                width: (layout.bar_width() * done / STAGE_COUNT as i32) as u32,
                ..bar
            },
            DONE,
        );
    }
    screen.drawn = reached;
    screen.drawn_running = running;
    if screen.mode_dirty {
        clear_line(canvas, layout, layout.mode_y());
        if screen.mode.len > 0 {
            let mut line = Text::new();
            let _ = write!(line, "Boot mode: {}", screen.mode.as_str());
            draw_str(
                canvas,
                layout.left,
                layout.mode_y(),
                line.as_str(),
                &TextStyle::new().with_color(DIM).with_background(BG),
            );
        }
        screen.mode_dirty = false;
    }
    if screen.status_dirty {
        clear_line(canvas, layout, layout.status_y());
        draw_str(
            canvas,
            layout.left,
            layout.status_y(),
            screen.status.as_str(),
            &TextStyle::new().with_color(RUNNING).with_background(BG),
        );
        screen.status_dirty = false;
    }
}

fn clear_line(canvas: &mut impl Canvas, layout: &Layout, y: i32) {
    fill_rect(
        canvas,
        Rect {
            x: layout.left,
            y,
            width: (layout.col_width * 2) as u32,
            height: LINE as u32,
        },
        BG,
    );
}

/// Userspace has taken the display: stop drawing the boot screen (or log
/// console), leaving its pixels up until userspace draws (`hand_over`). If the
/// new owner fails before it draws, the boot screen stays on screen.
///
/// Returns false once the diagnostics screen is up: it keeps the screen.
pub fn stop_drawing() -> bool {
    if DIAG_CLAIMED.load(Ordering::Acquire) {
        return false;
    }
    if !HANDED_OVER.swap(true, Ordering::AcqRel) && super::log_console::stop() {
        LOG_CONSOLE_UP.store(true, Ordering::Release);
    }
    true
}

/// Userspace is about to draw into `fb`: replace the kernel's screen with a
/// plain dark background for the userspace pane (the whole screen for the
/// display owner, the left half for other programs).
///
/// Called from the fb syscalls with the framebuffer lock held, after they have
/// checked that the command draws, so the layout is settled in the same critical
/// section as the first userspace draw. The GPU flush is left to the caller's
/// flush and the render thread's dirty-rect flush.
///
/// Returns false once the diagnostics screen is up: it keeps the screen, and the
/// caller refuses the draw.
pub fn hand_over(fb: &mut arm64_fb::ShellFrameBuffer) -> bool {
    if !stop_drawing() {
        return false;
    }
    if LAYOUT_SETTLED.load(Ordering::Acquire) {
        return true;
    }
    let boot_screen_up = ACTIVE.swap(false, Ordering::AcqRel);
    let log_console_up = LOG_CONSOLE_UP.swap(false, Ordering::AcqRel);
    if boot_screen_up || log_console_up {
        let (width, height) = (fb.width(), fb.height());
        fill_rect(
            fb,
            Rect {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            BG,
        );
        if let Some(db) = fb.double_buffer_mut() {
            db.flush_if_dirty();
        }
    }
    LAYOUT_SETTLED.store(true, Ordering::Release);
    true
}

/// Copy the shadow buffer out (if any) and flush the dirty region to the display.
///
/// The flush is synchronous so that a boot that hangs right after an update still
/// shows it. After one failed flush the dirty region is left marked for the render
/// thread, or on VirGL the presenter thread, instead (see `GPU_FLUSH_FAILED`).
fn present(mut guard: spin::MutexGuard<'_, arm64_fb::ShellFrameBuffer>) {
    if let Some(db) = guard.double_buffer_mut() {
        db.flush_if_dirty();
    }
    drop(guard);
    if GPU_FLUSH_FAILED.load(Ordering::Acquire) {
        return;
    }
    if let Some((x, y, w, h)) = arm64_fb::take_dirty_rect() {
        if arm64_fb::flush_dirty_rect(x, y, w, h).is_err() {
            GPU_FLUSH_FAILED.store(true, Ordering::Release);
            arm64_fb::mark_dirty(x, y, w, h);
        }
    }
}

/// Start the thread that presents the kernel's screen when a synchronous present
/// could not, for displays with no render thread (VirGL: the render thread's
/// cursor and dirty-rect flushes would overwrite the frames bwm composites).
///
/// A boot-screen update whose flush failed, or a diagnostics screen whose flush
/// found the GPU lock busy, is left marked dirty; this thread presents it once
/// the GPU is free. It presents only while the kernel's screen is up (the boot
/// screen until userspace takes the display, or the diagnostics screen) and
/// otherwise just idles.
pub fn spawn_presenter() -> Result<u64, &'static str> {
    let handle = crate::task::kthread::kthread_run(presenter_main, "kscreen")
        .map_err(|_| "failed to spawn the screen presenter kthread")?;
    let tid = handle.tid();
    *PRESENTER.lock() = Some(handle);
    Ok(tid)
}

static PRESENTER: Mutex<Option<crate::task::kthread::KthreadHandle>> = Mutex::new(None);

/// Runs with interrupts enabled: no logging (see render_task).
fn presenter_main() {
    use crate::arch_impl::aarch64::timer;
    let mut retry_at = 0u64;
    while !crate::task::kthread::kthread_should_stop() {
        let kernel_screen_up =
            DIAG_CLAIMED.load(Ordering::Acquire) || !HANDED_OVER.load(Ordering::Acquire);
        if kernel_screen_up && timer::rdtsc() >= retry_at {
            if let Some((x, y, w, h)) = arm64_fb::take_dirty_rect() {
                // Non-blocking: the GPU lock may be held for good by a CPU that
                // failed inside a GPU command. Back off a second after a miss.
                if !arm64_fb::try_flush_rect_nonblocking(x, y, w, h) {
                    arm64_fb::mark_dirty(x, y, w, h);
                    retry_at = timer::rdtsc() + timer::frequency_hz();
                }
            }
        }
        crate::task::scheduler::yield_current();
        crate::arch_halt();
    }
}

fn draw_str(canvas: &mut impl Canvas, x: i32, y: i32, s: &str, style: &TextStyle) -> i32 {
    let mut cx = x;
    for c in s.chars() {
        if cx >= canvas.width() as i32 {
            break;
        }
        cx += draw_char(canvas, cx, y, c, style);
    }
    cx
}

/// Draw text with each font pixel scaled to `scale` x `scale`.
fn draw_scaled(canvas: &mut impl Canvas, x: i32, y: i32, s: &str, scale: i32, color: Color) {
    let font = TextStyle::new().font;
    let advance = font.metrics().char_advance() as i32;
    let mut cx = x;
    for c in s.chars() {
        let glyph = font.glyph_or_replacement(c);
        for (gx, gy, intensity) in glyph.pixels() {
            if intensity >= 128 {
                fill_rect(
                    canvas,
                    Rect {
                        x: cx + gx as i32 * scale,
                        y: y + gy as i32 * scale,
                        width: scale as u32,
                        height: scale as u32,
                    },
                    color,
                );
            }
        }
        cx += advance * scale;
    }
}

/// Name of the stage the boot was in: the first stage not yet reached.
fn current_stage_name() -> &'static str {
    let reached = REACHED.load(Ordering::Acquire);
    match (0..STAGE_COUNT).find(|&i| reached & (1 << i) == 0) {
        Some(i) => STAGE_NAMES[i],
        None => "boot complete, userspace running",
    }
}

// =============================================================================
// Diagnostics screen
// =============================================================================

const DIAG_BG: Color = Color::rgb(110, 18, 22);
const DIAG_TEXT: Color = Color::rgb(255, 235, 235);
const DIAG_DIM: Color = Color::rgb(235, 170, 170);
const DIAG_LOG_LINES: usize = 20;

/// Bytes of recent kernel log the diagnostics screen reads.
const DIAG_LOG_BYTES: usize = 4096;

/// The kernel log as it stood when a panic began (`capture_panic_log`), before the
/// serial dumps the panic handler prints ahead of the screen. `PANIC_LOG_LEN` is
/// `usize::MAX` until it is captured; the first panicking CPU captures it.
static PANIC_LOG: [AtomicU8; DIAG_LOG_BYTES] = [const { AtomicU8::new(0) }; DIAG_LOG_BYTES];
static PANIC_LOG_LEN: AtomicUsize = AtomicUsize::new(usize::MAX);
static PANIC_LOG_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Keep the recent kernel log as it is now, for the panic's diagnostics screen to
/// show the lines that led to the panic rather than the dumps printed after it.
/// Lock-free and allocation-free.
pub fn capture_panic_log() {
    if PANIC_LOG_CLAIMED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut tail = [0u8; DIAG_LOG_BYTES];
    let n = crate::log_buffer::copy_tail(&mut tail);
    for (slot, &byte) in PANIC_LOG.iter().zip(&tail[..n]) {
        slot.store(byte, Ordering::Relaxed);
    }
    PANIC_LOG_LEN.store(n, Ordering::Release);
}

/// Which kernel log the diagnostics screen shows.
#[derive(Clone, Copy)]
enum RecentLog {
    /// The log as it is now.
    Live,
    /// The log captured when the panic began, if it was.
    AtPanic,
}

impl RecentLog {
    fn copy(self, buf: &mut [u8; DIAG_LOG_BYTES]) -> usize {
        let captured = PANIC_LOG_LEN.load(Ordering::Acquire);
        match self {
            RecentLog::AtPanic if captured != usize::MAX => {
                for (byte, slot) in buf.iter_mut().zip(&PANIC_LOG[..captured]) {
                    *byte = slot.load(Ordering::Relaxed);
                }
                captured
            }
            _ => crate::log_buffer::copy_tail(buf),
        }
    }
}

/// Draw the diagnostics screen for a kernel panic. Never blocks.
pub fn show_panic(info: &core::panic::PanicInfo) {
    let mut what = Text::new();
    let _ = write!(what, "{}", info.message());
    let mut place = Text::new();
    if let Some(location) = info.location() {
        let _ = write!(
            place,
            "{}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }
    show_diagnostics(
        "KERNEL PANIC",
        what.as_str(),
        &[("Location", place.as_str())],
        RecentLog::AtPanic,
    );
}

/// Draw the diagnostics screen for a fatal EL1 exception, described in words
/// from its syndrome (`esr`), fault address (`far`) and PC (`elr`). Does not block.
pub fn show_fault(esr: u64, far: u64, elr: u64) {
    let ec = ((esr >> 26) & 0x3f) as u8;
    let iss = esr & 0x01ff_ffff;
    let mut what = Text::new();
    let mut address = Text::new();
    let mut exception = Text::new();
    let mut pc = Text::new();
    let _ = write!(pc, "{:#018x}", elr);
    let (headline, class) = exception_class(ec);
    let _ = write!(exception, "class {:#04x} ({})", ec, class);
    // Instruction aborts (0x20/0x21) and data aborts (0x24/0x25) carry a fault
    // status code and, unless ISS.FnV says otherwise, a valid FAR.
    if matches!(ec, 0x20 | 0x21 | 0x24 | 0x25) {
        let status = (iss & 0x3f) as u8;
        let access = if ec < 0x24 {
            "instruction fetch from"
        } else if iss & (1 << 6) != 0 {
            "write to"
        } else {
            "read of"
        };
        let _ = write!(what, "{} in the kernel: {} ", headline, access);
        write_fault_target(&mut what, status);
        let _ = write!(exception, ", ");
        write_fault_status(&mut exception, status);
        if iss & (1 << 10) != 0 {
            let _ = write!(address, "not reported (FAR not valid)");
        } else {
            let _ = write!(address, "{:#018x}", far);
        }
    } else {
        let _ = write!(what, "{} in the kernel", headline);
        let _ = write!(address, "none (not a memory access fault)");
    }
    let _ = write!(exception, ", ESR {:#010x}", esr);
    show_diagnostics(
        "FATAL KERNEL FAULT",
        what.as_str(),
        &[
            ("Fault address", address.as_str()),
            ("PC", pc.as_str()),
            ("Exception", exception.as_str()),
        ],
        RecentLog::Live,
    );
}

/// A plain headline and the architectural name for an ESR_EL1 exception class.
fn exception_class(ec: u8) -> (&'static str, &'static str) {
    match ec {
        0x00 => ("Undefined instruction", "unknown reason"),
        0x01 => ("Trapped WFI/WFE", "WFI/WFE trap"),
        0x07 => ("Trapped FP/SIMD instruction", "FP/SIMD access trap"),
        0x0e => ("Illegal execution state", "illegal execution state"),
        0x15 => ("Unexpected system call", "SVC from AArch64"),
        0x18 => ("Trapped system register access", "MSR/MRS trap"),
        0x20 => ("Instruction abort", "instruction abort from EL0"),
        0x21 => ("Instruction abort", "instruction abort from EL1"),
        0x22 => ("Misaligned PC", "PC alignment fault"),
        0x24 => ("Data abort", "data abort from EL0"),
        0x25 => ("Data abort", "data abort from EL1"),
        0x26 => ("Misaligned stack pointer", "SP alignment fault"),
        0x2f => ("SError (asynchronous abort)", "SError"),
        0x30 | 0x31 => ("Hardware breakpoint", "breakpoint"),
        0x32 | 0x33 => ("Software step", "software step"),
        0x34 | 0x35 => ("Watchpoint hit", "watchpoint"),
        0x3c => ("BRK instruction", "BRK from AArch64"),
        _ => ("Unexpected exception", "unrecognised class"),
    }
}

/// What the faulting access touched, in plain words, from a DFSC/IFSC code.
fn write_fault_target(out: &mut Text, status: u8) {
    let _ = match status {
        0x00..=0x03 => write!(out, "an address beyond the translation range"),
        0x04..=0x07 => write!(out, "an unmapped address"),
        0x08..=0x0b => write!(out, "a page with its access flag clear"),
        0x0c..=0x0f => write!(out, "a page it has no permission for"),
        0x10 => write!(out, "memory that raised an external abort"),
        0x21 => write!(out, "a misaligned address"),
        _ => write!(out, "an address (fault status {:#04x})", status),
    };
}

/// The architectural name of a DFSC/IFSC fault status code.
fn write_fault_status(out: &mut Text, status: u8) {
    let level = status & 0x3;
    let _ = match status {
        0x00..=0x03 => write!(out, "address size fault at level {}", level),
        0x04..=0x07 => write!(out, "translation fault at level {}", level),
        0x08..=0x0b => write!(out, "access flag fault at level {}", level),
        0x0c..=0x0f => write!(out, "permission fault at level {}", level),
        0x10 => write!(out, "synchronous external abort"),
        0x21 => write!(out, "alignment fault"),
        0x30 => write!(out, "TLB conflict abort"),
        _ => write!(out, "fault status {:#04x}", status),
    };
}

/// Draw the red diagnostics screen: `title`, a large `headline` saying what
/// failed, labelled `details`, the boot stage and the recent kernel log.
fn show_diagnostics(title: &str, headline: &str, details: &[(&str, &str)], recent: RecentLog) {
    if DIAG_CLAIMED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return;
    };
    let Some(mut guard) = fb.try_lock() else {
        return;
    };
    ACTIVE.store(false, Ordering::Release);
    HANDED_OVER.store(true, Ordering::Release);
    super::log_console::stop();
    let canvas = &mut *guard;
    let (width, height) = (canvas.width() as i32, canvas.height() as i32);
    fill_rect(
        canvas,
        Rect {
            x: 0,
            y: 0,
            width: width as u32,
            height: height as u32,
        },
        DIAG_BG,
    );
    let left = 40;
    let text = TextStyle::new()
        .with_color(DIAG_TEXT)
        .with_background(DIAG_BG);
    let dim = TextStyle::new()
        .with_color(DIAG_DIM)
        .with_background(DIAG_BG);
    let advance = text.font.metrics().char_advance().max(1) as i32;
    let cols = ((width - 2 * left) / advance).max(20) as usize;
    draw_scaled(canvas, left, 32, title, 3, DIAG_TEXT);

    // The headline wraps at double size, up to three lines.
    let mut y = 100;
    let headline_cols = (cols / 2).max(10);
    for chunk in headline.as_bytes().chunks(headline_cols).take(3) {
        let chunk = core::str::from_utf8(chunk).unwrap_or("");
        draw_scaled(canvas, left, y, chunk, 2, DIAG_TEXT);
        y += LINE * 2;
    }
    y += LINE / 2;

    let value_x = left + 16 * advance;
    let stage = [("Boot stage", current_stage_name())];
    for (label, value) in details.iter().chain(stage.iter()) {
        draw_str(canvas, left, y, label, &dim);
        draw_str(canvas, value_x, y, value, &text);
        y += LINE;
    }
    y += LINE;
    draw_str(canvas, left, y, "Recent kernel log:", &dim);
    y += LINE + 4;

    let mut tail = [0u8; DIAG_LOG_BYTES];
    let n = recent.copy(&mut tail);
    let log = &tail[..n];
    let mut starts = [0usize; DIAG_LOG_LINES + 1];
    let mut found = 0;
    let mut end = log.len();
    while end > 0 && (log[end - 1] == b'\n' || log[end - 1] == b'\r') {
        end -= 1;
    }
    let mut i = end;
    while i > 0 && found < DIAG_LOG_LINES {
        i -= 1;
        if log[i] == b'\n' {
            starts[found] = i + 1;
            found += 1;
        }
    }
    if i == 0 && found < DIAG_LOG_LINES {
        starts[found] = 0;
        found += 1;
    }
    for k in (0..found).rev() {
        if y + LINE > height - 8 {
            break;
        }
        let start = starts[k];
        let stop = log[start..end]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(end, |p| start + p);
        // Decode as UTF-8 so a multi-byte character is one replacement, not one
        // per byte; a sequence cut by the start of the copied tail is one `?`.
        let mut line = Text::new();
        for chunk in log[start..stop].utf8_chunks() {
            // CRLF line ends leave a '\r' that would draw as '?'.
            for part in chunk.valid().split('\r') {
                let _ = line.write_str(part);
            }
            if !chunk.invalid().is_empty() {
                let _ = line.write_str("?");
            }
        }
        let shown = line.as_str();
        let shown = &shown[..shown.len().min(cols)];
        draw_str(canvas, left, y, shown, &text);
        y += LINE;
    }

    if let Some(db) = guard.double_buffer_mut() {
        db.flush_if_dirty();
    }
    drop(guard);
    present_diagnostics(width as u32, height as u32);
}

/// Put the diagnostics screen on the display. The render thread (or on VirGL the
/// presenter thread) may be presenting on another CPU and hold the GPU lock, so
/// retry for about 100 ms; if the flush still cannot be issued, leave the screen
/// marked dirty for that thread to present once the GPU is free.
fn present_diagnostics(width: u32, height: u32) {
    use crate::arch_impl::aarch64::timer;
    let _ = arm64_fb::take_dirty_rect();
    let deadline = timer::rdtsc() + timer::frequency_hz() / 10;
    while !arm64_fb::try_flush_rect_nonblocking(0, 0, width, height) {
        if timer::rdtsc() >= deadline {
            arm64_fb::mark_dirty(0, 0, width, height);
            return;
        }
        core::hint::spin_loop();
    }
}
