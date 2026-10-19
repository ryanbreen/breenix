//! Diagnostics screen and display hand-over (ARM64).
//!
//! The boot splash is the screen console's (`console::splash`). This module
//! keeps the kernel's screen for failures and the hand-over to a display owner.
//!
//! On a panic or fatal EL1 fault `show_panic`/`show_fault` draw a red diagnostics
//! screen with the failure, the boot stage and the last log lines. They do not
//! block on a lock: if the framebuffer lock is held the screen is skipped and the
//! report is on serial alone; if the GPU lock stays held for about 100 ms
//! (possibly by the failing CPU) the screen is left marked dirty for the render
//! thread, or on VirGL the presenter thread (`spawn_presenter`), to present.
//!
//! Once userspace takes the display (`stop_drawing`) the console stops drawing;
//! when userspace actually draws (`hand_over`) the kernel's screen is cleared to
//! a plain background for it. If userspace does not draw, the kernel's screen
//! stays up.

#![cfg(target_arch = "aarch64")]

use super::arm64_fb;
use super::cursor;
use super::primitives::{draw_char, fill_rect, Canvas, Color, Rect, TextStyle};
use crate::console::splash::Text;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;

/// The console has drawn on the screen (the splash, then its text), and its
/// pixels are still on screen.
static CONSOLE_UP: AtomicBool = AtomicBool::new(false);
/// Userspace has the display; the kernel no longer draws.
static HANDED_OVER: AtomicBool = AtomicBool::new(false);
/// Userspace has drawn and the kernel's screen has been replaced by the split layout.
static LAYOUT_SETTLED: AtomicBool = AtomicBool::new(false);
/// The diagnostics screen has been claimed (first failure wins).
static DIAG_CLAIMED: AtomicBool = AtomicBool::new(false);

const BG: Color = Color::rgb(15, 20, 35);

const LINE: i32 = 20;

/// The console drew its first frame: a display owner's first draw clears it.
pub fn console_drew() {
    CONSOLE_UP.store(true, Ordering::Release);
}

/// Whether the diagnostics screen has the screen.
pub fn diagnostics_up() -> bool {
    DIAG_CLAIMED.load(Ordering::Acquire)
}

/// Space below the splash stage label and above its mode/status lines.
/// The console presents this region only while its splash owns the display.
pub fn runner_area(canvas: &impl Canvas) -> Option<Rect> {
    if !crate::console::splash::is_up()
        || HANDED_OVER.load(Ordering::Acquire)
        || DIAG_CLAIMED.load(Ordering::Acquire)
    {
        return None;
    }
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;
    let font = crate::console::raster::Font::for_display(width as usize, height as usize);
    let scale = match font.size() {
        32 => 4,
        24 => 3,
        _ => 2,
    };
    let line = font.line_height() as i32;
    let top = 2 * height / 5 + (141 - 32) * scale + line + 8;
    let bottom = height - 3 * line - 8;
    if bottom <= top || width <= 80 {
        return None;
    }
    Some(Rect {
        x: 40,
        y: top,
        width: (width - 80) as u32,
        height: (bottom - top) as u32,
    })
}

/// Userspace has taken the display: stop drawing the boot screen and the
/// screen console, leaving their pixels up until userspace draws (`hand_over`).
/// If the new owner fails before it draws, the kernel's screen stays up.
///
/// Returns false once the diagnostics screen is up: it keeps the screen.
pub fn stop_drawing() -> bool {
    if DIAG_CLAIMED.load(Ordering::Acquire) {
        return false;
    }
    HANDED_OVER.store(true, Ordering::Release);
    crate::console::take_display();
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
    if CONSOLE_UP.swap(false, Ordering::AcqRel) {
        let (width, height) = (fb.width(), fb.height());
        cursor::hide(fb);
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
        cursor::show(fb);
        if let Some(db) = fb.double_buffer_mut() {
            db.flush_if_dirty();
        }
    }
    LAYOUT_SETTLED.store(true, Ordering::Release);
    true
}

/// Start the thread that presents the kernel's screen when a synchronous present
/// could not, for displays with no render thread (VirGL: the render thread's
/// cursor and dirty-rect flushes would overwrite the frames bwm composites).
///
/// A diagnostics screen whose flush found the GPU lock busy is left marked
/// dirty; this thread presents it once the GPU is free. It presents only while
/// the kernel's screen is up (until userspace takes the display, or the
/// diagnostics screen) and otherwise just idles.
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
    HANDED_OVER.store(true, Ordering::Release);
    let canvas = &mut *guard;
    let (width, height) = (canvas.width() as i32, canvas.height() as i32);
    cursor::hide(canvas);
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
    let stage = [("Boot stage", crate::console::splash::current_stage_name())];
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

    cursor::show(canvas);

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
