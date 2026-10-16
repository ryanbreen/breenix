//! The breengel suite: BreenGL's 2D library (shapes, blits, clipping and alpha; TrueType and
//! bitmap text; font, inflate, BMP and PNG parsing; widgets, text entry and animated icons)
//! and the kernel window interface, compositor and display observation path, measured
//! against the written contract, `docs/gui/breengel.md` (docs/suites/breengel.json).
//!
//! Every case cites the contract section it checks, in its failure message. Expected values
//! come from the contract's literal tables and formulas (typed here by hand), from fixtures
//! built in this file from those tables, or from a lower layer whose own cases certify it
//! first; the code under test never computes its own expectation.
//!
//! Surface cases draw into memory with a `PROT_NONE` guard page after the pixels and a
//! sentinel pattern in every row's padding and before the first row, and check both. A call
//! that can crash or run away (extreme coordinates, malformed input) runs in a forked child
//! with a deadline, so the case reports which call failed and how.
//!
//! A case whose layer does not exist fails with a message naming the missing piece and its
//! section. The kernel window interface is probed through its `VERSION` op: without it the
//! case says so; with it, a case whose checks are not written here fails saying that, so no
//! case passes for want of a layer.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use libbreenix::crypto::sha256::sha256;
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::suite::{case, category, fail, suite, value, CaseResult, Suite};
use libbreenix::syscall::raw;
use libbreenix::{io, memory, signal, time};

use libbui::input::{InputState, WidgetEvent};
use libbui::rect::Rect;
use libbui::shortcut::Shortcut;
use libbui::theme::Theme;
use libbui::widget::button::Button;
use libbui::widget::checkbox::Checkbox;
use libbui::widget::file_picker::{FileEntry, FilePicker};
use libbui::widget::menu_bar::{Menu, MenuBar, MenuEvent, MenuItem};
use libbui::widget::scroll_bar::ScrollBar;
use libbui::widget::slider::Slider;
use libbui::widget::tab_bar::TabBar;
use libfont::{CachedFont, Font};
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::{bitmap_font, diagnostics, font as font5x7, shapes, ttf_font};
use libicon::icon::{Icon, IconBase, IconMouse, IconState};
use libicon::icons::{arrow::ArrowIcon, home::HomeIcon, save::SaveIcon, search::SearchIcon};
use libicon::particles::{Particle, ParticlePool};
use libicon::physics::Spring;

/// Counts every allocation the suite's process makes, and the bytes it frees, so "before any
/// allocation", the parsers' allocation bounds (contract section 7) and an owned surface's
/// release can be measured in the guest.
struct Counting;
static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static FREE_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; only counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREE_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Relaxed);
        FREE_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

mod images;
mod raster;
mod show;
mod widgets;
mod window_cases;

use libbreenix::suite::Case;

// Each lane's module exports its own case tables. A category whose cases come from more
// than one module is joined here in manifest order.

/// The number of cases in `parts`.
const fn total(parts: &[&[Case]]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < parts.len() {
        n += parts[i].len();
        i += 1;
    }
    n
}

/// The cases of `parts`, in order, as one table.
const fn join<const N: usize>(parts: &[&[Case]]) -> [Case; N] {
    let mut out = [const { Case { id: "", title: "", run: unjoined } }; N];
    let (mut k, mut i) = (0, 0);
    while i < parts.len() {
        let mut j = 0;
        while j < parts[i].len() {
            let c = &parts[i][j];
            out[k] = Case { id: c.id, title: c.title, run: c.run };
            k += 1;
            j += 1;
        }
        i += 1;
    }
    assert!(k == N, "a joined case table has the wrong length");
    out
}

/// Never run: `join` fills every slot, and asserts so at compile time.
fn unjoined() -> CaseResult {
    fail("an empty case-table slot")
}

const TEXT_PARTS: &[&[Case]] = &[raster::TEXT, widgets::FONTWATCH];
static TEXT: [Case; total(TEXT_PARTS)] = join(TEXT_PARTS);

const COMPOSITION_PARTS: &[&[Case]] = &[window_cases::COMPOSITION, show::COMPOSITION, window_cases::COMPOSITION_FLUSH];
static COMPOSITION: [Case; total(COMPOSITION_PARTS)] = join(COMPOSITION_PARTS);

const PANEL_PARTS: &[&[Case]] = &[show::PANEL, window_cases::PANEL];
static PANEL: [Case; total(PANEL_PARTS)] = join(PANEL_PARTS);

static SUITE: Suite = suite("breengel", "BreenGL graphics library", &[
    category("raster", "shapes, blits, clipping & alpha", raster::RASTER),
    category("text", "TrueType & bitmap text", &TEXT),
    category("font-parser", "font parsing safety", raster::FONT_PARSER),
    category("inflate", "clean-room inflate", images::INFLATE),
    category("bmp", "BMP images", images::BMP),
    category("png-decode", "PNG decoding", images::PNG_DECODE),
    category("png-encode", "PNG encoding", images::PNG_ENCODE),
    category("widgets", "widgets", widgets::WIDGETS),
    category("text-entry", "text fields, focus & file picker", widgets::TEXT_ENTRY),
    category("icons", "animated icons", widgets::ICONS),
    category("windows", "window lifecycle on every target", window_cases::WINDOWS),
    category("isolation", "window authority & EFAULT safety", window_cases::ISOLATION),
    category("composition", "commit, composition & present", &COMPOSITION),
    category("events", "window input events", window_cases::EVENTS),
    category("scanout", "display observation on every target", window_cases::SCANOUT),
    category("panel", "suite & probe panels", &PANEL),
]);

fn main() {
    SUITE.run()
}

// ---------------------------------------------------------------------------------------
// Shared harness (owned by the raster lane; every module reaches it through `use crate::*`)
// ---------------------------------------------------------------------------------------

/// The contract document every message cites.
const CONTRACT: &str = "docs/gui/breengel.md";

/// Fail naming a piece of the stack this case needs that does not exist.
fn missing(what: &str, section: &str) -> CaseResult {
    fail(format!("missing {what} ({CONTRACT} section {section})"))
}

/// Fail with a check's own message, citing its section.
fn at(section: &str, msg: impl AsRef<str>) -> CaseResult {
    fail(format!("{} ({CONTRACT} section {section})", msg.as_ref()))
}

fn ensure(cond: bool, section: &str, msg: impl AsRef<str>) -> CaseResult {
    if cond { Ok(()) } else { at(section, msg) }
}

fn errno_name(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 5 => "EIO", 9 => "EBADF",
        11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 16 => "EBUSY",
        22 => "EINVAL", 24 => "EMFILE", 38 => "ENOSYS", 110 => "ETIMEDOUT",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw system call's return described for a message.
fn outcome(ret: i64) -> String {
    if (-4095..0).contains(&ret) { format!("failed with {}", errno_name(-ret)) } else { format!("returned {ret}") }
}

const SYS_FB_INFO: u64 = 410;
const SYS_FB_DRAW: u64 = 411;
const SYS_FB_MMAP: u64 = 412;
const SYS_WINDOW: u64 = 414;
const SYS_TEST_CONTROL: u64 = 415;
const SYS_TAKE_OVER_DISPLAY: u64 = 431;
const SYS_GIVE_BACK_DISPLAY: u64 = 432;

/// `window(VERSION)` (contract section 17.1): 2 when the interface exists.
fn window_version() -> i64 {
    // SAFETY: op 0 takes no argument; the kernel reads no memory.
    unsafe { raw::syscall3(SYS_WINDOW, 0, 0, 0) as i64 }
}

/// `test_control(STATE)` (contract section 21): 1 when suite-mode test controls are latched on.
fn test_control_state() -> i64 {
    // SAFETY: op 0 takes no argument.
    unsafe { raw::syscall3(SYS_TEST_CONTROL, 0, 0, 0) as i64 }
}

/// Run `f` in a forked child, which reports its own result; a crash or a child that runs
/// past `limit_ms` is reported as such. Output a case needs goes back as the message.
fn isolated(what: &str, limit_ms: u64, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let (rd, wr) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    match process::fork().map_err(|e| format!("fork: {e}"))? {
        ForkResult::Child => {
            let _ = io::close(rd);
            let code = match f() {
                Ok(()) => 0,
                Err(msg) => {
                    let _ = io::write(wr, msg.as_bytes());
                    1
                }
            };
            process::exit(code);
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(wr);
            let pid = pid.raw() as i32;
            let start = now_ms();
            let mut status = 0;
            let exited = loop {
                match process::waitpid(pid, &mut status, WNOHANG) {
                    Ok(p) if p.raw() != 0 => break true,
                    Ok(_) => {}
                    Err(e) => return Err(format!("waitpid: {e}")),
                }
                if now_ms() - start > limit_ms {
                    let _ = signal::kill(pid, signal::SIGKILL);
                    let _ = process::waitpid(pid, &mut status, 0);
                    break false;
                }
                let _ = time::sleep_ms(1);
            };
            let mut msg = Vec::new();
            let mut buf = [0u8; 512];
            while let Ok(n) = io::read(rd, &mut buf) {
                if n == 0 { break; }
                msg.extend_from_slice(&buf[..n]);
            }
            let _ = io::close(rd);
            let msg = String::from_utf8_lossy(&msg).into_owned();
            if !exited {
                Err(format!("{what} did not finish within {limit_ms} ms"))
            } else if process::wifsignaled(status) {
                Err(format!("{what} crashed with signal {}", process::wtermsig(status)))
            } else if process::wifexited(status) && process::wexitstatus(status) == 0 {
                Ok(())
            } else {
                Err(if msg.is_empty() { format!("{what} failed without a message") } else { msg })
            }
        }
    }
}

fn now_ms() -> u64 {
    time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

const PAGE: usize = 4096;

/// A surface in its own mapping: a sentinel run before the first row, a sentinel in each
/// row's padding, and a `PROT_NONE` guard page right after the last row's last byte.
struct Guarded {
    map: *mut u8,
    map_len: usize,
    base: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    bgr: bool,
    lead: usize,
}

const LEAD_BYTES: usize = 64;

fn sentinel(row: usize, i: usize) -> u8 {
    (0xA5 ^ (row as u8).wrapping_mul(29) ^ (i as u8).wrapping_mul(7)) | 1
}

impl Guarded {
    fn new(width: usize, height: usize, stride: usize, bpp: usize, bgr: bool) -> Guarded {
        let bytes = stride * height;
        let body = (bytes + LEAD_BYTES).div_ceil(PAGE) * PAGE;
        let map_len = body + PAGE;
        let map = memory::mmap(core::ptr::null_mut(), map_len, memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).expect("mmap for a guarded surface");
        // SAFETY: the mapping is `map_len` bytes; the guard is its last page.
        unsafe {
            memory::mprotect(map.add(body), PAGE, memory::PROT_NONE).expect("mprotect guard page");
        }
        // The pixels end at the guard; with stride a multiple of 4 the base stays aligned.
        let base = unsafe { map.add(body - bytes) };
        let lead = body - bytes;
        let mut g = Guarded { map, map_len, base, width, height, stride, bpp, bgr, lead };
        g.reset(Color::rgb(0, 0, 0));
        g
    }

    /// A 4-byte canonical BGRX surface with no padding.
    fn bgrx(width: usize, height: usize) -> Guarded {
        Guarded::new(width, height, width * 4, 4, true)
    }

    /// Fill every pixel with `c` (X byte 0xFF) and rewrite every sentinel.
    fn reset(&mut self, c: Color) {
        // SAFETY: all offsets lie in the mapping's writable part.
        unsafe {
            for i in 0..self.lead {
                *self.map.add(i) = sentinel(usize::MAX, i);
            }
            for y in 0..self.height {
                let row = self.base.add(y * self.stride);
                for x in 0..self.width {
                    let p = row.add(x * self.bpp);
                    let (b0, b1, b2) = if self.bgr { (c.b, c.g, c.r) } else { (c.r, c.g, c.b) };
                    *p = b0;
                    *p.add(1) = b1;
                    *p.add(2) = b2;
                    if self.bpp == 4 {
                        *p.add(3) = 0xFF;
                    }
                }
                for i in self.width * self.bpp..self.stride {
                    *row.add(i) = sentinel(y, i);
                }
            }
        }
    }

    fn fb(&self) -> FrameBuf {
        // SAFETY: base addresses stride * height writable bytes that outlive the FrameBuf,
        // which is used only while this Guarded is alive.
        unsafe { FrameBuf::from_raw(self.base, self.width, self.height, self.stride, self.bpp, self.bgr).expect("a guarded surface is valid") }
    }

    /// The pixel's colour channels, decoded by the surface's channel order.
    fn rgb(&self, x: usize, y: usize) -> [u8; 3] {
        // SAFETY: x < width, y < height by the callers.
        unsafe {
            let p = self.base.add(y * self.stride + x * self.bpp);
            if self.bgr { [*p.add(2), *p.add(1), *p] } else { [*p, *p.add(1), *p.add(2)] }
        }
    }

    fn color(&self, x: usize, y: usize) -> Color {
        let [r, g, b] = self.rgb(x, y);
        Color::rgb(r, g, b)
    }

    fn x_byte(&self, x: usize, y: usize) -> u8 {
        // SAFETY: as for rgb, on a 4-byte surface.
        unsafe { *self.base.add(y * self.stride + x * self.bpp + 3) }
    }

    /// Every sentinel is intact: the run before the first row and each row's padding.
    fn sentinels(&self) -> Result<(), String> {
        // SAFETY: the offsets are inside the mapping.
        unsafe {
            for i in 0..self.lead {
                if *self.map.add(i) != sentinel(usize::MAX, i) {
                    return Err(format!("the sentinel {} bytes before the first row was overwritten", self.lead - i));
                }
            }
            for y in 0..self.height {
                let row = self.base.add(y * self.stride);
                for i in self.width * self.bpp..self.stride {
                    if *row.add(i) != sentinel(y, i) {
                        return Err(format!("row {y}'s padding byte {} was overwritten", i - self.width * self.bpp));
                    }
                }
            }
        }
        Ok(())
    }

    /// The first pixel whose colour differs from `want(x, y)`, as a message.
    fn compare(&self, what: &str, want: impl Fn(usize, usize) -> Color) -> Result<(), String> {
        for y in 0..self.height {
            for x in 0..self.width {
                let (got, w) = (self.color(x, y), want(x, y));
                if got != w {
                    return Err(format!("{what}: pixel ({x},{y}) is {} where the contract gives {}", hex(got), hex(w)));
                }
            }
        }
        self.sentinels()
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = memory::munmap(self.map, self.map_len);
    }
}

/// How many pixels differ from `bg`: the positive control that a drawing drew something.
fn drawn_pixels(g: &Guarded, bg: Color) -> usize {
    (0..g.height).flat_map(|y| (0..g.width).map(move |x| (x, y))).filter(|&(x, y)| g.color(x, y) != bg).count()
}

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

const INK: Color = Color::rgb(200, 30, 90);
const PAPER: Color = Color::rgb(10, 20, 30);

fn in_rect(x: i64, y: i64, rx: i64, ry: i64, rw: i64, rh: i64) -> bool {
    rw > 0 && rh > 0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
}

/// A step result lifted into a case result, citing `section`.
fn step(section: &str, r: Result<(), String>) -> CaseResult {
    match r {
        Ok(()) => Ok(()),
        Err(msg) => at(section, msg),
    }
}

/// Cite a contract section on a step's error.
trait Cite<T> {
    fn cite(self, section: &str) -> Result<T, libbreenix::suite::CaseError>;
}

impl<T, E: std::fmt::Display> Cite<T> for Result<T, E> {
    fn cite(self, section: &str) -> Result<T, libbreenix::suite::CaseError> {
        self.map_err(|e| libbreenix::suite::CaseError::Fail(format!("{e} ({CONTRACT} section {section})")))
    }
}

const WHITE: Color = Color::rgb(255, 255, 255);

/// A seeded xorshift generator, so a random scene is the same on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

/// Where the guest reads the reviewed references (section 8.6).
const FRAMES: &str = "/usr/local/test/share/breengel/frames";

/// Require `g` to equal the reviewed reference `name` pixel for pixel. The reference is a
/// PNG decoded by libimg (section 13, certified by the png-decode cases); it must show
/// something, so a blank drawing cannot match a blank reference.
fn matches_reference(g: &Guarded, name: &str, section: &str) -> CaseResult {
    let path = format!("{FRAMES}/{name}");
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the reviewed reference {path}: {e}")).cite(section)?;
    let img = libimg::png::decode(&bytes).map_err(|e| format!("the reviewed reference {path} does not decode: {e:?}")).cite(section)?;
    ensure((img.width as usize, img.height as usize) == (g.width, g.height), section, format!(
        "the reviewed reference {name} is {}x{}; the scene is {}x{}", img.width, img.height, g.width, g.height))?;
    let at_px = |x: usize, y: usize| {
        let p = &img.pixels[(y * g.width + x) * 4..][..4];
        Color::rgb(p[2], p[1], p[0])
    };
    let shown = (0..g.height).flat_map(|y| (0..g.width).map(move |x| (x, y))).filter(|&(x, y)| at_px(x, y) != Color::rgb(0, 0, 0)).count();
    ensure(shown > 0, section, format!("the reviewed reference {name} is blank"))?;
    for y in 0..g.height {
        for x in 0..g.width {
            if g.color(x, y) != at_px(x, y) {
                return at(section, format!("{name}: pixel ({x},{y}) is {} where the reviewed reference has {}", hex(g.color(x, y)), hex(at_px(x, y))));
            }
        }
    }
    step(section, g.sentinels())
}
