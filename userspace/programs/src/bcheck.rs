//! Breenix Self-Check — windowed post-boot test runner.
//!
//! Forks and execs a curated suite of test binaries, displaying live
//! GPU-rendered progress bars and per-test pass/fail results in a
//! compositor window.

use breengel::{CachedFont, Window, Event};
use libbreenix::process::{fork, exec, waitpid, ForkResult};
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;
use libgfx::ttf_font;

use libbui::Rect;
use libbui::widget::scroll_bar::ScrollBar;

// ---------------------------------------------------------------------------
// Test definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum TestStatus {
    Pending,
    Running,
    Pass,
    Fail,
    Skip,
}

struct TestDef {
    /// Binary path on ext2 (null-terminated)
    path: &'static [u8],
    /// Display name
    name: &'static str,
    /// Category for grouping
    category: &'static str,
    /// Result
    status: TestStatus,
    /// Elapsed time in milliseconds
    elapsed_ms: u32,
}

/// The curated test suite — exercises core subsystems without requiring
/// network, stdin, or interactive TTY.
fn make_tests() -> Vec<TestDef> {
    // Test binaries matching *_test or test_* are installed to /usr/local/test/bin/
    // by create_ext2_disk.sh.  Everything else goes to /bin/.
    let tests: &[(&[u8], &str, &str)] = &[
        // Core / syscall
        (b"/usr/local/test/bin/clock_gettime_test\0", "clock_gettime",   "core"),
        (b"/usr/local/test/bin/brk_test\0",           "brk",             "core"),
        (b"/usr/local/test/bin/test_mmap\0",          "mmap",            "core"),
        (b"/bin/syscall_enosys\0",                    "syscall_enosys",  "core"),
        // Filesystem
        (b"/usr/local/test/bin/file_read_test\0",     "file_read",       "fs"),
        (b"/usr/local/test/bin/lseek_test\0",         "lseek",           "fs"),
        (b"/usr/local/test/bin/getdents_test\0",      "getdents",        "fs"),
        (b"/usr/local/test/bin/access_test\0",        "access",          "fs"),
        (b"/usr/local/test/bin/cwd_test\0",           "cwd",             "fs"),
        (b"/usr/local/test/bin/devfs_test\0",         "devfs",           "fs"),
        (b"/usr/local/test/bin/fs_write_test\0",      "fs_write",        "fs"),
        (b"/usr/local/test/bin/fs_directory_test\0",  "fs_directory",    "fs"),
        // IPC
        (b"/usr/local/test/bin/pipe_test\0",         "pipe",             "ipc"),
        (b"/usr/local/test/bin/pipe2_test\0",        "pipe2",            "ipc"),
        (b"/usr/local/test/bin/dup_test\0",          "dup",              "ipc"),
        (b"/usr/local/test/bin/fcntl_test\0",        "fcntl",            "ipc"),
        // Process
        (b"/usr/local/test/bin/fork_test\0",         "fork",             "proc"),
        (b"/usr/local/test/bin/waitpid_test\0",      "waitpid",          "proc"),
        (b"/usr/local/test/bin/fork_memory_test\0",  "fork_memory",      "proc"),
        // Signals
        (b"/usr/local/test/bin/signal_handler_test\0", "signal_handler",  "sig"),
        (b"/usr/local/test/bin/signal_return_test\0",  "signal_return",   "sig"),
        // Network
        (b"/usr/local/test/bin/net_test\0",            "dns_resolve",     "net"),
        (b"/usr/local/test/bin/http_fetch_test\0",     "http_fetch",      "net"),
    ];

    tests.iter().map(|&(path, name, cat)| TestDef {
        path, name, category: cat,
        status: TestStatus::Pending,
        elapsed_ms: 0,
    }).collect()
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

fn clock_ms() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const WIN_W: u32 = 500;
const WIN_H: u32 = 480;

// Colors
const BG: Color = Color::rgb(18, 20, 32);
const FG: Color = Color::rgb(210, 215, 225);
const TITLE_COLOR: Color = Color::rgb(80, 180, 255);
const ACCENT: Color = Color::rgb(50, 60, 85);
const BAR_BG: Color = Color::rgb(40, 44, 60);
const BAR_PASS: Color = Color::rgb(40, 200, 80);
const BAR_FAIL: Color = Color::rgb(220, 50, 50);
const BAR_RUN: Color = Color::rgb(60, 140, 255);
const COLOR_PASS: Color = Color::rgb(80, 220, 100);
const COLOR_FAIL: Color = Color::rgb(240, 60, 60);
const COLOR_RUN: Color = Color::rgb(80, 160, 255);
const COLOR_PEND: Color = Color::rgb(90, 95, 110);
const COLOR_SKIP: Color = Color::rgb(180, 160, 60);
const COLOR_TIME: Color = Color::rgb(120, 125, 140);
const COLOR_CAT: Color = Color::rgb(100, 110, 140);
const COLOR_HEADER_LINE: Color = Color::rgb(50, 60, 90);

const MARGIN: i32 = 12;
const TITLE_Y: i32 = 10;
const BAR_Y: i32 = 38;
const BAR_H: i32 = 16;
const LIST_START_Y: i32 = 64;
const ROW_H: i32 = 18;
const FOOTER_H: i32 = 24;

/// Compute the total content height of the test list (for scroll clamping).
fn content_height(tests: &[TestDef]) -> i32 {
    let mut y = 0i32;
    let mut last_cat = "";
    for test in tests.iter() {
        if test.category != last_cat {
            last_cat = test.category;
            if y > 0 { y += 4; }
            y += 12;
        }
        y += ROW_H;
    }
    y
}

/// Draw text using TTF font if available, falling back to bitmap.
fn draw_text_scaled(
    fb: &mut FrameBuf,
    ttf: Option<&mut CachedFont>,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Color,
    bitmap_scale: usize,
) {
    if let Some(f) = ttf {
        ttf_font::draw_text(fb, f, text, x, y, size, color);
    } else {
        font::draw_text(fb, text.as_bytes(), x as usize, y as usize, color, bitmap_scale);
    }
}

/// Measure text width using TTF font if available, falling back to bitmap.
fn measure_text(ttf: Option<&mut CachedFont>, text: &str, size: f32, bitmap_scale: usize) -> i32 {
    if let Some(f) = ttf {
        ttf_font::text_width(f, text, size)
    } else {
        font::text_width(text.as_bytes(), bitmap_scale) as i32
    }
}

fn render(fb: &mut FrameBuf, tests: &[TestDef], scroll_offset: i32, ttf: &mut Option<CachedFont>, font_size: f32) {
    let w = fb.width as i32;
    let h = fb.height as i32;

    // Background
    fb.clear(BG);

    // Title
    let title = "BREENIX SELF-CHECK";
    let title_size = font_size * 2.0;
    let title_w = measure_text(ttf.as_mut(), title, title_size, 2);
    draw_text_scaled(fb, ttf.as_mut(), title, (w - title_w) / 2, TITLE_Y, title_size, TITLE_COLOR, 2);

    // Header line
    shapes::fill_rect(fb, MARGIN, BAR_Y - 4, w - MARGIN * 2, 1, COLOR_HEADER_LINE);

    // Overall progress bar
    let total = tests.len() as u32;
    let passed = tests.iter().filter(|t| t.status == TestStatus::Pass).count() as u32;
    let failed = tests.iter().filter(|t| t.status == TestStatus::Fail).count() as u32;
    let running = tests.iter().filter(|t| t.status == TestStatus::Running).count() as u32;
    let completed = passed + failed;

    let bar_w = (w - MARGIN * 2) as u32;
    shapes::fill_rect(fb, MARGIN, BAR_Y, bar_w as i32, BAR_H, BAR_BG);

    if total > 0 {
        // Pass segment
        let pass_w = (passed as u32 * bar_w / total) as i32;
        if pass_w > 0 {
            shapes::fill_rect(fb, MARGIN, BAR_Y, pass_w, BAR_H, BAR_PASS);
        }
        // Fail segment
        let fail_w = (failed as u32 * bar_w / total) as i32;
        if fail_w > 0 {
            shapes::fill_rect(fb, MARGIN + pass_w, BAR_Y, fail_w, BAR_H, BAR_FAIL);
        }
        // Running segment (pulsing indicator)
        let run_w = (running as u32 * bar_w / total).max(if running > 0 { 4 } else { 0 }) as i32;
        if run_w > 0 {
            shapes::fill_rect(fb, MARGIN + pass_w + fail_w, BAR_Y, run_w, BAR_H, BAR_RUN);
        }
    }

    // Progress text on the bar
    let pct = if total > 0 { completed * 100 / total } else { 0 };
    let mut pct_buf = [0u8; 16];
    let pct_len = format_progress(&mut pct_buf, completed, total, pct);
    let pct_str = core::str::from_utf8(&pct_buf[..pct_len]).unwrap_or("");
    let pct_tw = measure_text(ttf.as_mut(), pct_str, font_size, 1);
    draw_text_scaled(fb, ttf.as_mut(), pct_str, (w - pct_tw) / 2, BAR_Y + 4, font_size, FG, 1);

    // Visible area for test list
    let visible_top = LIST_START_Y;
    let visible_bottom = h - FOOTER_H;

    // Test list (offset by scroll)
    let mut y = LIST_START_Y - scroll_offset;
    let mut last_cat = "";

    for test in tests.iter() {
        // Category header
        if test.category != last_cat {
            last_cat = test.category;
            let cat_label = match test.category {
                "core" => "CORE",
                "fs"   => "FILESYSTEM",
                "ipc"  => "IPC",
                "proc" => "PROCESS",
                "sig"  => "SIGNALS",
                "net"  => "NETWORK",
                _      => "OTHER",
            };
            if y > LIST_START_Y - scroll_offset {
                y += 4; // Extra spacing between categories
            }
            // Only draw if visible
            if y >= visible_top && y + 12 <= visible_bottom {
                let cat_tw = measure_text(ttf.as_mut(), cat_label, font_size, 1);
                draw_text_scaled(fb, ttf.as_mut(), cat_label, MARGIN, y, font_size, COLOR_CAT, 1);
                shapes::fill_rect(fb, MARGIN + cat_tw + 4, y + 3,
                                  w - MARGIN * 2 - cat_tw - 4, 1, ACCENT);
            }
            y += 12;
        }

        if y + ROW_H > visible_bottom {
            break; // Past the bottom — stop iterating
        }

        // Only draw row if it's in the visible region
        if y >= visible_top {
            // Status icon
            let (icon, icon_color): (&str, Color) = match test.status {
                TestStatus::Pass    => ("OK", COLOR_PASS),
                TestStatus::Fail    => ("!!", COLOR_FAIL),
                TestStatus::Running => (">>", COLOR_RUN),
                TestStatus::Skip    => ("--", COLOR_SKIP),
                TestStatus::Pending => ("..", COLOR_PEND),
            };
            draw_text_scaled(fb, ttf.as_mut(), icon, MARGIN + 2, y, font_size, icon_color, 1);

            // Test name
            draw_text_scaled(fb, ttf.as_mut(), test.name, MARGIN + 22, y, font_size, FG, 1);

            // Status label
            let (label, label_color): (&str, Color) = match test.status {
                TestStatus::Pass    => ("PASS", COLOR_PASS),
                TestStatus::Fail    => ("FAIL", COLOR_FAIL),
                TestStatus::Running => ("RUN ", COLOR_RUN),
                TestStatus::Skip    => ("SKIP", COLOR_SKIP),
                TestStatus::Pending => ("    ", COLOR_PEND),
            };
            let label_x = w - MARGIN - 80;
            draw_text_scaled(fb, ttf.as_mut(), label, label_x, y, font_size, label_color, 1);

            // Elapsed time (for completed tests)
            if test.status == TestStatus::Pass || test.status == TestStatus::Fail {
                let mut time_buf = [0u8; 8];
                let time_len = format_ms(&mut time_buf, test.elapsed_ms);
                let time_str = core::str::from_utf8(&time_buf[..time_len]).unwrap_or("");
                let time_x = w - MARGIN - 36;
                draw_text_scaled(fb, ttf.as_mut(), time_str, time_x, y, font_size, COLOR_TIME, 1);
            }
        }

        y += ROW_H;
    }

    // Footer
    let footer_y = h - FOOTER_H + 4;
    shapes::fill_rect(fb, MARGIN, footer_y - 6, w - MARGIN * 2, 1, COLOR_HEADER_LINE);

    let mut footer_buf = [0u8; 48];
    let footer_len = format_footer(&mut footer_buf, passed, failed, total - completed);
    let footer_str = core::str::from_utf8(&footer_buf[..footer_len]).unwrap_or("");
    draw_text_scaled(fb, ttf.as_mut(), footer_str, MARGIN, footer_y, font_size, FG, 1);
}

// ---------------------------------------------------------------------------
// Formatting helpers (no alloc)
// ---------------------------------------------------------------------------

fn format_u32(buf: &mut [u8], mut n: u32) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut len = 0;
    while n > 0 {
        tmp[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    for i in 0..len {
        buf[i] = tmp[len - 1 - i];
    }
    len
}

fn format_progress(buf: &mut [u8], done: u32, total: u32, pct: u32) -> usize {
    let mut i = 0;
    i += format_u32(&mut buf[i..], done);
    buf[i] = b'/'; i += 1;
    i += format_u32(&mut buf[i..], total);
    buf[i] = b' '; i += 1;
    buf[i] = b'('; i += 1;
    i += format_u32(&mut buf[i..], pct);
    buf[i] = b'%'; i += 1;
    buf[i] = b')'; i += 1;
    i
}

fn format_ms(buf: &mut [u8], ms: u32) -> usize {
    let mut i = 0;
    // Right-align in 4 chars
    if ms < 10 { buf[i] = b' '; i += 1; buf[i] = b' '; i += 1; buf[i] = b' '; i += 1; }
    else if ms < 100 { buf[i] = b' '; i += 1; buf[i] = b' '; i += 1; }
    else if ms < 1000 { buf[i] = b' '; i += 1; }
    i += format_u32(&mut buf[i..], ms);
    buf[i] = b'm'; i += 1;
    buf[i] = b's'; i += 1;
    i
}

fn format_footer(buf: &mut [u8], passed: u32, failed: u32, pending: u32) -> usize {
    let mut i = 0;
    // "PASS: N  FAIL: N  PEND: N"
    for &b in b"PASS: " { buf[i] = b; i += 1; }
    i += format_u32(&mut buf[i..], passed);
    for &b in b"  FAIL: " { buf[i] = b; i += 1; }
    i += format_u32(&mut buf[i..], failed);
    for &b in b"  PEND: " { buf[i] = b; i += 1; }
    i += format_u32(&mut buf[i..], pending);
    i
}

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

fn run_test(test: &mut TestDef) {
    let start = clock_ms();

    match fork() {
        Ok(ForkResult::Child) => {
            match exec(test.path) {
                Ok(_) => unreachable!(),
                Err(_) => {
                    // exec failed — can't print (might not have stdout)
                    std::process::exit(126);
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child_raw = child_pid.raw() as i32;
            let mut status: i32 = 0;
            match waitpid(child_raw, &mut status as *mut i32, 0) {
                Ok(_) => {
                    let exit_code = (status >> 8) & 0xFF;
                    let elapsed = (clock_ms() - start) as u32;
                    test.elapsed_ms = elapsed;
                    test.status = if exit_code == 0 {
                        TestStatus::Pass
                    } else {
                        TestStatus::Fail
                    };
                }
                Err(_) => {
                    test.elapsed_ms = (clock_ms() - start) as u32;
                    test.status = TestStatus::Fail;
                }
            }
        }
        Err(_) => {
            test.status = TestStatus::Skip;
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("[bcheck] Breenix Self-Check starting");

    let mut tests = make_tests();
    let total = tests.len();

    let mut win = match Window::new(b"Self-Check", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[bcheck] Window::new failed: {} -- running headless", e);
            run_headless(&mut tests);
            return;
        }
    };
    println!("[bcheck] Window {} ({}x{})", win.id(), WIN_W, WIN_H);

    // Load TTF mono font (falls back to bitmap if unavailable)
    let mut ttf_font: Option<CachedFont> = win.take_mono_font();
    let font_size = if win.mono_size() >= 6.0 { win.mono_size() } else { 10.0 };
    if ttf_font.is_some() {
        println!("[bcheck] TTF font loaded, size={}", font_size);
    } else {
        println!("[bcheck] TTF font not available, using bitmap fallback");
    }

    // Initial render — all pending
    render(win.framebuf(), &tests, 0, &mut ttf_font, font_size);
    let _ = win.present();

    // Run each test sequentially
    for i in 0..total {
        tests[i].status = TestStatus::Running;
        render(win.framebuf(), &tests, 0, &mut ttf_font, font_size);
        let _ = win.present();

        run_test(&mut tests[i]);

        let status_str = match tests[i].status {
            TestStatus::Pass => "PASS",
            TestStatus::Fail => "FAIL",
            TestStatus::Skip => "SKIP",
            _ => "????",
        };
        println!("[bcheck] {:2}/{} {} {} {}ms",
                 i + 1, total, tests[i].name, status_str, tests[i].elapsed_ms);

        render(win.framebuf(), &tests, 0, &mut ttf_font, font_size);
        let _ = win.present();
    }

    // Summary
    let passed = tests.iter().filter(|t| t.status == TestStatus::Pass).count();
    let failed = tests.iter().filter(|t| t.status == TestStatus::Fail).count();
    println!("[bcheck] Complete: {}/{} passed, {} failed", passed, total, failed);

    // Keep displaying results — support scrolling with arrow keys and scroll wheel
    let visible_h = WIN_H as i32 - LIST_START_Y - FOOTER_H;
    let total_h = content_height(&tests);
    let sleep_ts = libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 50_000_000 }; // 50ms

    // Scrollbar positioned along the right edge of the content area
    let bar_w = ScrollBar::DEFAULT_WIDTH;
    let bar_rect = Rect::new(WIN_W as i32 - bar_w, LIST_START_Y, bar_w, visible_h);
    let mut scroll_bar = ScrollBar::new(bar_rect, total_h, visible_h);
    let theme = libbui::Theme::dark();

    loop {
        let mut need_redraw = false;
        for event in win.poll_events() {
            match event {
                Event::KeyPress { keycode, .. } => {
                    match keycode {
                        0x52 => { scroll_bar.scroll(1); need_redraw = true; }
                        0x51 => { scroll_bar.scroll(-1); need_redraw = true; }
                        _ => {}
                    }
                }
                Event::Scroll { delta_y } => {
                    scroll_bar.scroll(delta_y);
                    need_redraw = true;
                }
                Event::Resized { width: w, height: h } => {
                    let new_visible_h = h as i32 - LIST_START_Y - FOOTER_H;
                    let new_bar_rect = Rect::new(
                        w as i32 - bar_w, LIST_START_Y, bar_w, new_visible_h,
                    );
                    scroll_bar.set_rect(new_bar_rect);
                    scroll_bar.set_dimensions(total_h, new_visible_h);
                    need_redraw = true;
                }
                Event::CloseRequested => std::process::exit(0),
                _ => {}
            }
        }
        if need_redraw {
            render(win.framebuf(), &tests, scroll_bar.offset(), &mut ttf_font, font_size);
            // Draw the scrollbar on top of the content
            scroll_bar.draw(win.framebuf(), &theme);
            let _ = win.present();
        } else {
            let _ = time::nanosleep(&sleep_ts);
        }
    }
}

/// Headless fallback — just run tests and report to serial
fn run_headless(tests: &mut [TestDef]) {
    let total = tests.len();
    for i in 0..total {
        tests[i].status = TestStatus::Running;
        run_test(&mut tests[i]);
        let status_str = match tests[i].status {
            TestStatus::Pass => "PASS",
            TestStatus::Fail => "FAIL",
            TestStatus::Skip => "SKIP",
            _ => "????",
        };
        println!("[bcheck] {:2}/{} {} {} {}ms",
                 i + 1, total, tests[i].name, status_str, tests[i].elapsed_ms);
    }
    let passed = tests.iter().filter(|t| t.status == TestStatus::Pass).count();
    let failed = tests.iter().filter(|t| t.status == TestStatus::Fail).count();
    println!("[bcheck] Complete: {}/{} passed, {} failed", passed, total, failed);
}
