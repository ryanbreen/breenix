//! Breenix Self-Check — windowed post-boot test runner.
//!
//! Forks and execs a curated suite of test binaries, displaying live
//! GPU-rendered progress bars and per-test pass/fail results in a
//! compositor window.

use breengel::{CachedFont, Window, Event, Handled};
#[path = "bcheck/layout.rs"] mod layout;
use layout::Layout;
use std::time::Duration;

#[path = "bcheck/catalog.rs"] mod catalog;
#[path = "bcheck/node.rs"] mod node;
#[path = "bcheck/runner.rs"] mod runner;
#[path = "bcheck/verdict.rs"] mod verdict;
use runner::Runner;
use verdict::Verdict;
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::shapes;
use libgfx::surface::Surface;
use libgfx::ttf_font;

use libbui::Rect;
use libbui::widget::scroll_bar::ScrollBar;

// ---------------------------------------------------------------------------
// Test definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum TestStatus { Pending, Running, Pass, Fail }

struct TestDef {
    name: String,
    category: String,
    argv: Vec<String>,
    deadline_ms: u64,
    status: TestStatus,
    verdict: String,
    pid: Option<u32>,
    elapsed_ms: u32,
}

fn make_tests(path: &str) -> Result<Vec<TestDef>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(catalog::parse(&text).into_iter().map(|entry| match entry {
        Ok(entry) => TestDef { name: entry.name, category: entry.category, argv: entry.argv,
            deadline_ms: entry.deadline_ms, status: TestStatus::Pending, verdict: String::new(), pid: None, elapsed_ms: 0 },
        Err(error) => TestDef { name: format!("line {}", error.line), category: "catalog".into(), argv: vec![],
            deadline_ms: 0, status: TestStatus::Fail, verdict: format!("invalid catalog: {}", error.reason), pid: None, elapsed_ms: 0 },
    }).collect())
}

fn verdict_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Pass => "pass".into(), Verdict::Exit(code) => format!("exit {code}"),
        Verdict::Signal(_, name) => (*name).into(),
        Verdict::ExecFailed(error) => format!("exec failed: {error}"),
        Verdict::TimedOut => "timed out".into(), Verdict::Cancelled => "cancelled".into(),
    }
}

fn finish_test(test: &mut TestDef, verdict: Verdict, start: u64) {
    test.status = if verdict.failed() { TestStatus::Fail } else { TestStatus::Pass };
    test.verdict = verdict_text(&verdict);
    test.pid = None;
    test.elapsed_ms = clock_ms().saturating_sub(start) as u32;
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
const COLOR_CAT: Color = Color::rgb(100, 110, 140);
const COLOR_HEADER_LINE: Color = Color::rgb(50, 60, 90);

/// Compute the total content height of the test list (for scroll clamping).
fn content_height(tests: &[TestDef], layout: &Layout) -> i32 {
    let mut y = 0i32;
    let mut last_cat = "";
    for test in tests.iter() {
        if test.category != last_cat {
            last_cat = &test.category;
            if y > 0 { y += layout.text.px(4); }
            y += layout.category_h;
        }
        y += layout.row_h;
    }
    y
}

/// A frame's text, drawn after its fills, which it does not overlap: each string as
/// `ttf_font::draw_text` draws it (or the 5x7 font at its scale without TrueType), with the
/// strings' glyphs sharing draws.
#[derive(Default)]
struct Text {
    strings: Vec<(String, i32, i32, f32, Color, usize)>,
}

impl Text {
    fn finish(self, fb: &mut Surface, ttf: Option<&mut CachedFont>) {
        if let Some(f) = ttf {
            let clip = libgfx::surface::Rect::new(0, 0, fb.width() as i32, fb.height() as i32);
            let runs: Vec<ttf_font::TextRun<'_>> = self.strings.iter().map(|(text, x, y, size, fg, _)| ttf_font::TextRun {
                bytes: text.as_bytes(), x: *x, y: *y, size: *size, fg: *fg, clip,
            }).collect();
            ttf_font::draw_runs(fb, f, &runs);
        } else {
            for (text, x, y, _, fg, scale) in &self.strings {
                font::draw_text(fb, text.as_bytes(), *x as usize, *y as usize, *fg, *scale);
            }
        }
    }
}

/// Queue text in the TrueType font if available, else the bitmap font at `bitmap_scale`.
fn draw_text_scaled(
    text_out: &mut Text,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Color,
    bitmap_scale: usize,
) {
    text_out.strings.push((String::from(text), x, y, size, color, bitmap_scale));
}

/// Measure text width using TTF font if available, falling back to bitmap.
fn measure_text(ttf: Option<&mut CachedFont>, text: &str, size: f32, bitmap_scale: usize) -> i32 {
    if let Some(f) = ttf {
        ttf_font::text_width(f, text, size)
    } else {
        font::text_width(text.as_bytes(), bitmap_scale) as i32
    }
}

fn render(fb: &mut Surface, tests: &[TestDef], scroll_offset: i32, ttf: &mut Option<CachedFont>, font_size: f32) {
    let w = fb.width() as i32;
    let h = fb.height() as i32;
    let layout = Layout::new(ttf.as_ref(), font_size, fb.ui_scale(), w, h);
    let px = |n| layout.text.px(n);
    let font_size = layout.text.size;
    let bitmap_scale = layout.text.scale as usize;
    let margin = px(12);
    let mut text = Text::default();

    // Background
    fb.clear(BG);

    // Title
    let title = "BREENIX SELF-CHECK";
    let title_size = font_size * 2.0;
    let title_w = measure_text(ttf.as_mut(), title, title_size, 2 * bitmap_scale);
    draw_text_scaled(&mut text, title, (w - title_w) / 2, px(10), title_size, TITLE_COLOR, 2 * bitmap_scale);

    // Header line
    shapes::fill_rect(fb, margin, layout.bar_y - px(4), w - margin * 2, px(1), COLOR_HEADER_LINE);

    // Overall progress bar
    let total = tests.len() as u32;
    let passed = tests.iter().filter(|t| t.status == TestStatus::Pass).count() as u32;
    let failed = tests.iter().filter(|t| t.status == TestStatus::Fail).count() as u32;
    let running = tests.iter().filter(|t| t.status == TestStatus::Running).count() as u32;
    let completed = passed + failed;

    let bar_w = (w - margin * 2) as u32;
    shapes::fill_rect(fb, margin, layout.bar_y, bar_w as i32, layout.bar_h, BAR_BG);

    if total > 0 {
        // Pass segment
        let pass_w = (passed as u32 * bar_w / total) as i32;
        if pass_w > 0 {
            shapes::fill_rect(fb, margin, layout.bar_y, pass_w, layout.bar_h, BAR_PASS);
        }
        // Fail segment
        let fail_w = (failed as u32 * bar_w / total) as i32;
        if fail_w > 0 {
            shapes::fill_rect(fb, margin + pass_w, layout.bar_y, fail_w, layout.bar_h, BAR_FAIL);
        }
        // Running segment (pulsing indicator)
        let run_w = (running as u32 * bar_w / total).max(if running > 0 { px(4) as u32 } else { 0 }) as i32;
        if run_w > 0 {
            shapes::fill_rect(fb, margin + pass_w + fail_w, layout.bar_y, run_w, layout.bar_h, BAR_RUN);
        }
    }

    // Progress text on the bar
    let pct = if total > 0 { completed * 100 / total } else { 0 };
    let mut pct_buf = [0u8; 16];
    let pct_len = format_progress(&mut pct_buf, completed, total, pct);
    let pct_str = core::str::from_utf8(&pct_buf[..pct_len]).unwrap_or("");
    let pct_tw = measure_text(ttf.as_mut(), pct_str, font_size, bitmap_scale);
    draw_text_scaled(&mut text, pct_str, (w - pct_tw) / 2, layout.bar_y + px(4), font_size, FG, bitmap_scale);

    // Visible area for test list
    let visible_top = layout.list_y;
    let visible_bottom = h - layout.footer_h;

    // Test list (offset by scroll)
    let mut y = layout.list_y - scroll_offset;
    let mut last_cat = "";

    for test in tests.iter() {
        // Category header
        if test.category != last_cat {
            last_cat = &test.category;
            let cat_label = match test.category.as_str() {
                "core" => "CORE",
                "fs"   => "FILESYSTEM",
                "ipc"  => "IPC",
                "proc" => "PROCESS",
                "sig"  => "SIGNALS",
                "net"  => "NETWORK",
                _      => "OTHER",
            };
            if y > layout.list_y - scroll_offset {
                y += px(4); // Extra spacing between categories
            }
            // Only draw if visible
            if y >= visible_top && y + layout.category_h <= visible_bottom {
                let cat_tw = measure_text(ttf.as_mut(), cat_label, font_size, bitmap_scale);
                draw_text_scaled(&mut text, cat_label, margin, y, font_size, COLOR_CAT, bitmap_scale);
                shapes::fill_rect(fb, margin + cat_tw + px(4), y + px(3),
                                  w - margin * 2 - cat_tw - px(4), px(1), ACCENT);
            }
            y += layout.category_h;
        }

        if y + layout.row_h > visible_bottom {
            break; // Past the bottom — stop iterating
        }

        // Only draw row if it's in the visible region
        if y >= visible_top {
            // Status icon
            let (icon, icon_color): (&str, Color) = match test.status {
                TestStatus::Pass    => ("OK", COLOR_PASS),
                TestStatus::Fail    => ("!!", COLOR_FAIL),
                TestStatus::Running => (">>", COLOR_RUN),
                TestStatus::Pending => ("..", COLOR_PEND),
            };
            draw_text_scaled(&mut text, icon, margin + px(2), y, font_size, icon_color, bitmap_scale);

            // Test name
            draw_text_scaled(&mut text, &test.name, margin + 3 * layout.text.cell_w + px(4), y, font_size, FG, bitmap_scale);

            // Status label
            let (label, label_color): (&str, Color) = match test.status {
                TestStatus::Pass    => (test.verdict.as_str(), COLOR_PASS),
                TestStatus::Fail    => (test.verdict.as_str(), COLOR_FAIL),
                TestStatus::Running => ("RUN ", COLOR_RUN),
                TestStatus::Pending => ("    ", COLOR_PEND),
            };
            let label_x = w / 2;
            draw_text_scaled(&mut text, label, label_x, y, font_size, label_color, bitmap_scale);


        }

        y += layout.row_h;
    }

    // Footer
    let footer_y = h - layout.footer_h + px(4);
    shapes::fill_rect(fb, margin, footer_y - px(6), w - margin * 2, px(1), COLOR_HEADER_LINE);

    let mut footer_buf = [0u8; 48];
    let footer_len = format_footer(&mut footer_buf, passed, failed, total - completed);
    let footer_str = core::str::from_utf8(&footer_buf[..footer_len]).unwrap_or("");
    draw_text_scaled(&mut text, footer_str, margin, footer_y, font_size, FG, bitmap_scale);
    text.finish(fb, ttf.as_mut());
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

fn publish_tree(win: &mut Window, tests: &[TestDef], offset: i32) {
    use breengel::tree::{Flag, Node, Role, Tree};
    let rows: Vec<_> = tests.iter().map(|test| node::Row { name: &test.name, state: match test.status {
        TestStatus::Pending => node::State::Pending,
        TestStatus::Running => node::State::Running { pid: test.pid.expect("running child") },
        TestStatus::Pass | TestStatus::Fail => node::State::Done { verdict: &test.verdict, failed: test.status == TestStatus::Fail },
    }}).collect();
    let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
    let flags = vec![Flag::Enabled, Flag::Visible];
    win.set_tree(Tree { root: Node { key: "rows".into(), parent: None, role: Role::List, name: "Rows".into(),
        value: node::rows(&rows, offset), bounds, clip: bounds, flags: flags.clone() }, children: vec![Node {
        key: "totals".into(), parent: Some("rows".into()), role: Role::Status, name: "Totals".into(),
        value: node::totals(&rows), bounds, clip: bounds, flags,
    }] });
}

fn frame(win: &mut Window, tests: &[TestDef], offset: i32, ttf: &mut Option<CachedFont>, font_size: f32,
    scroll: Option<(&ScrollBar, &libbui::Theme)>) -> Result<(), libbreenix::error::Error> {
    publish_tree(win, tests, offset);
    let fb = win.surface()?;
    render(fb, tests, offset, ttf, font_size);
    if let Some((bar, theme)) = scroll { bar.draw(fb, theme); }
    win.present()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.as_slice() {
        [_] => "/usr/share/bcheck/catalog",
        [_, flag, path] if flag == "--catalog" => path,
        _ => { eprintln!("usage: bcheck [--catalog FILE]"); std::process::exit(1); }
    };
    let mut tests = make_tests(path).unwrap_or_else(|error| { eprintln!("[bcheck] {error}"); std::process::exit(1) });
    let mut win = match Window::new(b"Self-Check", WIN_W, WIN_H) {
        Ok(win) => win,
        Err(_) => { run_headless(&mut tests); std::process::exit(exit_code(&tests)); }
    };
    let mut ttf_font = win.take_mono_font();
    let font_size = if win.mono_size() >= 6.0 { win.mono_size() } else { 10.0 };
    let ui_scale = win.surface().expect("self-check surface").ui_scale() as i32;
    if ui_scale != 1 { win.apply_resize(WIN_W * ui_scale as u32, WIN_H * ui_scale as u32).expect("scaled self-check"); }
    let layout = Layout::new(ttf_font.as_ref(), font_size, ui_scale as u32, win.width() as i32, win.height() as i32);
    let visible_h = layout.visible_h() / ui_scale;
    let total_h = content_height(&tests, &layout) / ui_scale;
    let r = layout.scroll_rect();
    let bar_rect = Rect::new(r[0] / ui_scale, r[1] / ui_scale, r[2] / ui_scale, r[3] / ui_scale);
    let mut scroll_bar = ScrollBar::new(bar_rect, total_h, visible_h);
    let theme = libbui::Theme::dark();
    // Present the scrollbar as soon as results become scrollable.
    if scroll_bar.can_scroll() && frame(&mut win, &tests, 0, &mut ttf_font, font_size, Some((&scroll_bar, &theme))).is_ok() {
        let mut pixels = vec![0u8; win.width() as usize * win.height() as usize * 4];
        match libbreenix::window::capture(win.id(), &mut pixels, None) {
            Ok(_) => {
                let x = bar_rect.x * ui_scale;
                let y = bar_rect.y * ui_scale;
                let samples = [(x, y, [48, 42, 40]), (x + 3 * ui_scale, y + 2 * ui_scale, [100, 85, 80])];
                let exact = samples.iter().all(|(x, y, want)| {
                    let at = (*y as usize * win.width() as usize + *x as usize) * 4;
                    pixels[at..at + 3] == *want
                });
                if exact {
                    println!("[bcheck] scrollbar read back: bounds={},{},{},{} window={}x{} scale={} 2 pixels exact",
                        x, y, bar_rect.w * ui_scale, bar_rect.h * ui_scale, win.width(), win.height(), ui_scale);
                } else {
                    println!("[bcheck] scrollbar layout FAILED: track or thumb differs");
                }
            }
            Err(e) => println!("[bcheck] scrollbar layout FAILED: capture {}", e),
        }
    }

    let mut active: Option<(usize, Runner, u64)> = None;
    let mut closing = false;
    let sleep = libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 50_000_000 };
    loop {
        let mut resize = None;
        let count = win.dispatch(|delivered| {
            match delivered.event.clone() {
                Event::KeyPress { keycode, ascii, .. } => match keycode {
                    breengel::keys::UP => scroll_bar.scroll(1),
                    breengel::keys::DOWN => scroll_bar.scroll(-1),
                    breengel::keys::ESC => closing = true,
                    _ if ascii == b'q' || ascii == b'Q' => closing = true,
                    _ => {},
                },
                Event::Scroll { delta_y, .. } => scroll_bar.scroll(delta_y),
                Event::CloseRequested => closing = true,
                Event::ResizeRequested { width, height } => resize = Some((width, height)),
                Event::Resized { width, height } => {
                    let layout = Layout::new(ttf_font.as_ref(), font_size, ui_scale as u32, width as i32, height as i32);
                    let r = layout.scroll_rect();
                    scroll_bar.set_rect(Rect::new(r[0] / ui_scale, r[1] / ui_scale, r[2] / ui_scale, r[3] / ui_scale));
                    scroll_bar.set_dimensions(total_h, layout.visible_h() / ui_scale);
                }
                _ => {}
            }
            Handled::Redraw
        }).unwrap_or_else(|error| { eprintln!("[bcheck] dispatch: {error}"); closing = true; 0 });
        if let Some((width, height)) = resize {
            if win.apply_resize(width, height).is_ok() {
                let layout = Layout::new(ttf_font.as_ref(), font_size, ui_scale as u32, win.width() as i32, win.height() as i32);
                let r = layout.scroll_rect();
                scroll_bar.set_rect(Rect::new(r[0] / ui_scale, r[1] / ui_scale, r[2] / ui_scale, r[3] / ui_scale));
                scroll_bar.set_dimensions(total_h, layout.visible_h() / ui_scale);
            }
        }
        let mut changed = count != 0;
        if closing { if let Some((_, runner, _)) = active.as_mut() { runner.cancel(); } }
        if let Some((index, runner, start)) = active.as_mut() {
            if let Some(verdict) = runner.poll() {
                finish_test(&mut tests[*index], verdict, *start);
                active = None;
                changed = true;
            }
        }
        if closing && active.is_none() { std::process::exit(exit_code(&tests)); }
        if !closing && active.is_none() {
            if let Some(index) = tests.iter().position(|test| test.status == TestStatus::Pending) {
                let start = clock_ms();
                match Runner::start(&tests[index].argv, Duration::from_millis(tests[index].deadline_ms)) {
                    Ok(runner) => { tests[index].status = TestStatus::Running; tests[index].pid = Some(runner.pid()); active = Some((index, runner, start)); }
                    Err(verdict) => finish_test(&mut tests[index], verdict, start),
                }
                changed = true;
            }
        }
        if changed { let _ = frame(&mut win, &tests, scroll_bar.offset() * ui_scale, &mut ttf_font, font_size, Some((&scroll_bar, &theme))); }
        else { let _ = time::nanosleep(&sleep); }
    }
}

fn exit_code(tests: &[TestDef]) -> i32 { i32::from(tests.iter().any(|test| test.status == TestStatus::Fail)) }

fn run_headless(tests: &mut [TestDef]) {
    for test in tests {
        if test.status != TestStatus::Pending { continue; }
        let start = clock_ms();
        let verdict = match Runner::start(&test.argv, Duration::from_millis(test.deadline_ms)) {
            Err(verdict) => verdict,
            Ok(mut runner) => loop {
                if let Some(verdict) = runner.poll() { break verdict; }
                std::thread::sleep(Duration::from_millis(50));
            },
        };
        finish_test(test, verdict, start);
        println!("[bcheck] {}: {}", test.name, test.verdict);
    }
}
