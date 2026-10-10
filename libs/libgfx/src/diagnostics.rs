//! Reusable, syscall-free diagnostics panel. The caller owns the framebuffer and flushes it.

use core::fmt::Write;

use crate::{color::Color, font, framebuf::FrameBuf, shapes};

/// A check's current state and the detail or failure reason to display.
pub enum CheckState<'a> {
    Pending,
    Running,
    Ok(&'a str),
    Fail(&'a str),
    /// Not run, with the reason.
    Skip(&'a str),
}

pub struct Check<'a> {
    pub name: &'a str,
    pub state: CheckState<'a>,
}

pub struct Group<'a> {
    pub title: &'a str,
    pub checks: &'a [Check<'a>],
}

pub enum Verdict<'a> {
    Running(&'a str),
    Passed(&'a str),
    Failed(&'a str),
}

/// All strings are borrowed; callers may redraw with updated checks and recent output.
pub struct Panel<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub groups: &'a [Group<'a>],
    /// Oldest first. The last lines fitting the available area are shown.
    pub output: &'a [&'a str],
    pub verdict: Verdict<'a>,
    /// Benchmark style: under the banner, the overall score (checks passed of all
    /// checks, as a count, a percentage and a bar), a progress bar under each group's
    /// title, and passing checks labelled PASS rather than OK.
    pub scored: bool,
    /// Live readings, drawn in a strip above the verdict bar when the framebuffer is
    /// tall enough; [`draw`] says where, and [`draw_live`] redraws just the strip.
    pub live: Option<&'a Live<'a>>,
}

/// What the live strip shows: the clocks as it is drawn, and the running check's wait
/// and measured values.
pub struct Live<'a> {
    /// The running check, or empty when none is running.
    pub check: &'a str,
    /// CLOCK_MONOTONIC and CLOCK_REALTIME as the strip is drawn, in nanoseconds.
    pub monotonic_ns: i64,
    pub realtime_ns: i64,
    /// How long the running check has run, in milliseconds.
    pub check_ms: i64,
    /// The wait the check last started.
    pub wait: Option<Wait>,
    /// The check's measured values, oldest first; the latest that fit are shown.
    pub values: &'a [Reading<'a>],
    /// The check `wait` and `values` came from: `check`, or the one before it, kept on
    /// screen until the running check reports something of its own.
    pub values_from: &'a str,
    /// Shown in place of the wait when no check is running, such as the suite's total time.
    pub idle: &'a str,
}

/// A wait a check started: on a sleep or a timer.
#[derive(Clone, Copy)]
pub struct Wait {
    /// When the wait should end, in CLOCK_MONOTONIC milliseconds, and how long it is.
    pub until_ms: i64,
    pub for_ms: i64,
    /// The first value the check reported once the wait was over, as an index into
    /// [`Live::values`]: shown in place of the countdown.
    pub result: Option<usize>,
}

/// A measured value and, when the check states one, the range it accepts.
pub struct Reading<'a> {
    pub name: &'a str,
    pub number: i64,
    pub unit: &'a str,
    pub expect: Option<(i64, i64)>,
}

/// Where [`draw`] put the live strip, for [`draw_live`].
#[derive(Clone, Copy, Debug)]
pub struct LiveArea {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    s: i32,
}

fn line(fb: &mut FrameBuf, text: &str, x: i32, y: i32, width: i32, color: Color, scale: usize) {
    if x < 0 || y < 0 || width <= 0 || y as usize >= fb.height { return; }
    let max = (width as usize / (6 * scale)).min(text.len());
    let bytes = text.as_bytes();
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) { end -= 1; }
    font::draw_text(fb, &bytes[..end], x as usize, y as usize, color, scale);
}

/// Render to a valid framebuffer. Short or missing framebuffers can be skipped by callers.
///
/// The panel fills the framebuffer: a full-width banner and verdict bar, the groups
/// split in order across as many columns of at least 200 pixels as fit (at most four,
/// and no more than there are groups, so a lone group spans the width), then the recent
/// output across the full width. It draws at double size whenever the groups still fit
/// that way. When the groups do not fit above the verdict bar with two-line check cards,
/// the cards drop to one line (a failure keeps its reason, after the name). If they still
/// do not fit, each group longer than the space allows shows a window of its checks --
/// the running check, then failures, then the checks nearest the running one (or the
/// last one finished), so the window follows the run -- and a last row counting the
/// checks it leaves out. Only if even the group headings do not fit are whole groups
/// left out, with a line saying so.
///
/// With [`Panel::live`] and room for it (a framebuffer at least `LIVE_MIN_HEIGHT` tall
/// at the panel's scale), a strip of live readings sits above the verdict bar, and the
/// groups take what is left. Returns where the strip is, for [`draw_live`].
pub fn draw(fb: &mut FrameBuf, panel: &Panel<'_>) -> Option<LiveArea> {
    let width = fb.width as i32;
    let height = fb.height as i32;
    if width < 160 || height < 120 { return None; }
    let body_limit = |s: i32, room_for_output: bool| bottom(panel, height, s, room_for_output);
    let fits_capped = |s: i32, cards: Cards, room_for_output: bool, cap: usize| {
        layout_groups(None, panel, s, cards, width, i32::MAX, cap).0
            <= body_limit(s, room_for_output)
    };
    let fits = |s: i32, cards: Cards, room_for_output: bool| fits_capped(s, cards, room_for_output, usize::MAX);
    let (s, cards) = if width >= 960 && fits(2, Cards::Full, !panel.output.is_empty()) {
        (2, Cards::Full)
    } else if fits(1, Cards::Full, false) {
        (1, Cards::Full)
    } else {
        (1, Cards::Compact)
    };
    let margin = 16 * s;
    let text_width = width - 2 * margin;
    fb.clear(Color::rgb(12, 19, 31));
    shapes::fill_rect(fb, 0, 0, width, 54 * s, Color::rgb(24, 39, 59));
    line(fb, panel.title, margin, 10 * s, text_width, Color::WHITE, 2 * s as usize);
    line(fb, panel.subtitle, margin, 36 * s, text_width, Color::rgb(161, 183, 204), s as usize);
    if panel.scored {
        draw_score(fb, panel, margin, 62 * s, text_width, s);
    }

    let body_bottom = body_limit(s, false);
    let line_height = 11 * s;
    // The most checks a group may show (its window, the last row counting the rest)
    // so that every group fits; at least two, the running check and that row.
    let longest = panel.groups.iter().map(|group| group.checks.len()).max().unwrap_or(0);
    let cap = if fits(s, cards, false) {
        Some(usize::MAX)
    } else if longest >= 2 && fits_capped(s, cards, false, 2) {
        let (mut low, mut high) = (2, longest);
        while low < high {
            let mid = (low + high).div_ceil(2);
            if fits_capped(s, cards, false, mid) { low = mid; } else { high = mid - 1; }
        }
        Some(low)
    } else {
        None
    };
    let mut y = if let Some(cap) = cap {
        layout_groups(Some(&mut *fb), panel, s, cards, width, body_bottom, cap).0
    } else {
        // Keep the last body line for a note about the checks left out.
        let limit = body_bottom - line_height;
        let (groups_bottom, hidden) =
            layout_groups(Some(&mut *fb), panel, s, cards, width, limit, 2);
        if hidden > 0 {
            line(fb, "NOT ALL CHECKS FIT ON SCREEN", margin, limit, text_width, HEADING, s as usize);
            body_bottom
        } else {
            groups_bottom.min(body_bottom)
        }
    };
    if !panel.output.is_empty() && y + 14 * s + line_height <= body_bottom {
        line(fb, "RECENT OUTPUT", margin, y, text_width, HEADING, s as usize);
        y += 14 * s;
        let count = ((body_bottom - y) / line_height).max(0) as usize;
        let first = panel.output.len().saturating_sub(count);
        for output in &panel.output[first..] {
            line(fb, output, margin, y, text_width, Color::WHITE, s as usize);
            y += line_height;
        }
    }

    let (text, fill) = match &panel.verdict {
        Verdict::Running(text) => (*text, Color::rgb(48, 67, 96)),
        Verdict::Passed(text) => (*text, Color::rgb(26, 112, 77)),
        Verdict::Failed(text) => (*text, Color::rgb(145, 50, 58)),
    };
    let footer = 38 * s;
    shapes::fill_rect(fb, 0, height - footer, width, footer, fill);
    line(fb, text, margin, height - 27 * s, text_width, Color::WHITE, s as usize);

    let live = panel.live.filter(|_| live_reserve(panel, height, s) > 0)?;
    // The strip takes the height the groups leave, between its least and its most.
    let floor = height - 42 * s;
    let top = (y + 8 * s).clamp(floor - LIVE_MAX_HEIGHT * s, floor - LIVE_HEIGHT * s);
    let area = LiveArea { x: 0, y: top, w: width, h: floor - top, s };
    draw_live(fb, area, live);
    Some(area)
}

const HEADING: Color = Color::rgb(110, 168, 212);
const PENDING: Color = Color::rgb(57, 67, 80);
const PASSED: Color = Color::rgb(26, 112, 77);
const FAILED: Color = Color::rgb(145, 50, 58);
const SKIPPED: Color = Color::rgb(138, 104, 36);

/// The height of the score strip under the banner, at scale 1.
const SCORE_HEIGHT: i32 = 38;

/// How many of a set of checks passed, failed and were skipped.
#[derive(Clone, Copy, Default)]
struct Tally {
    passed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

impl Tally {
    fn of(checks: &[Check<'_>]) -> Tally {
        let mut tally = Tally { total: checks.len(), ..Tally::default() };
        for check in checks {
            match check.state {
                CheckState::Ok(_) => tally.passed += 1,
                CheckState::Fail(_) => tally.failed += 1,
                CheckState::Skip(_) => tally.skipped += 1,
                CheckState::Pending | CheckState::Running => {}
            }
        }
        tally
    }

    fn add(self, other: Tally) -> Tally {
        Tally {
            passed: self.passed + other.passed,
            failed: self.failed + other.failed,
            skipped: self.skipped + other.skipped,
            total: self.total + other.total,
        }
    }

    fn percent(self) -> usize {
        if self.total == 0 { 0 } else { self.passed * 100 / self.total }
    }
}

/// Text formatted into a fixed buffer, cut short if it does not fit.
struct Text {
    bytes: [u8; 96],
    len: usize,
}

impl Text {
    fn new() -> Text { Text { bytes: [0; 96], len: 0 } }

    fn as_str(&self) -> &str { core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("") }
}

impl Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for &byte in text.as_bytes() {
            if self.len == self.bytes.len() { break; }
            self.bytes[self.len] = byte;
            self.len += 1;
        }
        Ok(())
    }
}

/// A bar `width` wide: passed, failed and skipped checks fill it from the left in
/// proportion to the total, and what has not run yet stays dark.
fn tally_bar(fb: &mut FrameBuf, tally: Tally, x: i32, y: i32, width: i32, height: i32) {
    shapes::fill_rect(fb, x, y, width, height, PENDING);
    if tally.total == 0 || width <= 0 { return; }
    let edge = |count: usize| x + (width as i64 * count as i64 / tally.total as i64) as i32;
    let mut done = 0;
    for (count, color) in [(tally.passed, PASSED), (tally.failed, FAILED), (tally.skipped, SKIPPED)] {
        let (left, right) = (edge(done), edge(done + count));
        shapes::fill_rect(fb, left, y, right - left, height, color);
        done += count;
    }
}

/// The score strip: passed of total and the percentage, the other counts, and a bar.
fn draw_score(fb: &mut FrameBuf, panel: &Panel<'_>, x: i32, y: i32, width: i32, s: i32) {
    let tally = panel.groups.iter().fold(Tally::default(), |sum, group| sum.add(Tally::of(group.checks)));
    let mut score = Text::new();
    let _ = write!(score, "{} / {} PASSED  {}%", tally.passed, tally.total, tally.percent());
    line(fb, score.as_str(), x, y, width, Color::WHITE, 2 * s as usize);
    let mut counts = Text::new();
    let remaining = tally.total - tally.passed - tally.failed - tally.skipped;
    let _ = write!(counts, "{} failed  {} skipped  {} to run", tally.failed, tally.skipped, remaining);
    let counts_width = counts.len as i32 * 6 * s;
    let score_width = score.len as i32 * 12 * s;
    if score_width + counts_width + 12 * s <= width {
        line(fb, counts.as_str(), x + width - counts_width, y + 7 * s, counts_width, Color::rgb(161, 183, 204),
            s as usize);
    }
    tally_bar(fb, tally, x, y + 19 * s, width, 9 * s);
}

/// How a check is drawn: a two-line card with the detail or failure reason, or a
/// one-line card with the name, the state and any failure reason.
#[derive(Clone, Copy)]
enum Cards {
    Full,
    Compact,
}

impl Cards {
    /// Card height and the distance to the next card, at scale 1.
    fn size(self) -> (i32, i32) {
        match self {
            Cards::Full => (23, 26),
            Cards::Compact => (11, 13),
        }
    }
}

/// The lowest y the body may reach at scale `s`, leaving room for the verdict bar, the
/// live strip when there is one, and, when `room_for_output`, for the output heading and
/// a few output lines.
fn bottom(panel: &Panel<'_>, height: i32, s: i32, room_for_output: bool) -> i32 {
    let reserve = if room_for_output { (14 + 5 * 11) * s } else { 0 };
    height - 42 * s - reserve - live_reserve(panel, height, s)
}

/// The live strip's least and most height at scale 1: it takes what the groups leave.
const LIVE_HEIGHT: i32 = 168;
const LIVE_MAX_HEIGHT: i32 = 270;
/// The shortest framebuffer, at scale 1, that gets the live strip.
const LIVE_MIN_HEIGHT: i32 = 480;

/// The height the live strip and the gap above it take at scale `s`, or 0 without one.
fn live_reserve(panel: &Panel<'_>, height: i32, s: i32) -> i32 {
    if panel.live.is_some() && height >= LIVE_MIN_HEIGHT * s { (LIVE_HEIGHT + 8) * s } else { 0 }
}

/// The height of a group's title, and of its progress bar when `scored`, at scale 1.
fn heading_height(scored: bool) -> i32 { if scored { 22 } else { 13 } }

/// A group's height when it shows at most `cap` rows of checks.
fn group_height(group: &Group<'_>, s: i32, cards: Cards, scored: bool, cap: usize) -> i32 {
    (heading_height(scored) + group.checks.len().min(cap) as i32 * cards.size().1 + 8) * s
}

/// Lay the panel's groups out below the banner (and the score, when scored) at scale
/// `s`: in order, down one column and on to the next, splitting them so the tallest
/// column is as short as possible, each group showing at most `cap` rows (see
/// `draw_group`). Draws when given a framebuffer, leaving out any heading or card that
/// would extend below `limit`. Returns the bottom of the tallest column as laid out and
/// how many checks were left out below `limit`.
fn layout_groups(mut fb: Option<&mut FrameBuf>, panel: &Panel<'_>, s: i32, cards: Cards,
    width: i32, limit: i32, cap: usize) -> (i32, usize) {
    let groups = panel.groups;
    let scored = panel.scored;
    let margin = 16 * s;
    let gap = 12 * s;
    let top = if scored { (64 + SCORE_HEIGHT) * s } else { 64 * s };
    let columns = ((width - 2 * margin) / (200 * s)).clamp(1, 4).min(groups.len().max(1) as i32);
    let column_width = (width - 2 * margin - gap * (columns - 1)) / columns;
    // Columns needed when each holds at most `column_limit` of height.
    let columns_for = |column_limit: i32| {
        let (mut used, mut filled) = (1, 0);
        for group in groups {
            let height = group_height(group, s, cards, scored, cap);
            if filled > 0 && filled + height > column_limit { used += 1; filled = 0; }
            filled += height;
        }
        used
    };
    let mut low = groups.iter().map(|group| group_height(group, s, cards, scored, cap)).max().unwrap_or(0);
    let mut high: i32 = groups.iter().map(|group| group_height(group, s, cards, scored, cap)).sum();
    while low < high {
        let mid = (low + high) / 2;
        if columns_for(mid) <= columns { high = mid; } else { low = mid + 1; }
    }
    let (mut column, mut filled, mut tallest, mut hidden) = (0, 0, 0, 0);
    for group in groups {
        let height = group_height(group, s, cards, scored, cap);
        if filled > 0 && filled + height > low { column += 1; filled = 0; }
        if let Some(fb) = fb.as_deref_mut() {
            let x = margin + column * (column_width + gap);
            hidden += draw_group(fb, group, x, top + filled, column_width, s, cards, scored, limit, cap);
        }
        filled += height;
        tallest = tallest.max(filled);
    }
    (top + tallest, hidden)
}

/// Draw one group's heading (with its progress bar and count when `scored`) and
/// cards, leaving out any that would extend below `limit`. A group of more than `cap`
/// checks shows `cap - 1` of them (`shown`) and a row counting the rest. Returns how
/// many checks were left out below `limit`.
fn draw_group(fb: &mut FrameBuf, group: &Group<'_>, x: i32, mut y: i32, width: i32, s: i32,
    cards: Cards, scored: bool, limit: i32, cap: usize) -> usize {
    let heading = heading_height(scored) * s;
    if y + heading > limit { return group.checks.len(); }
    if scored {
        let tally = Tally::of(group.checks);
        let mut count = Text::new();
        let _ = write!(count, "{}/{}", tally.passed, tally.total);
        let count_width = count.len as i32 * 6 * s;
        line(fb, group.title, x, y, width - count_width - 6 * s, HEADING, s as usize);
        line(fb, count.as_str(), x + width - count_width, y, count_width, HEADING, s as usize);
        tally_bar(fb, tally, x, y + 10 * s, width, 6 * s);
    } else {
        line(fb, group.title, x, y, width, HEADING, s as usize);
    }
    y += heading;
    let (card_height, pitch) = cards.size();
    let windowed = group.checks.len() > cap;
    let mut drawn = 0;
    for (index, check) in group.checks.iter().enumerate() {
        if windowed && !shown(group.checks, index, cap - 1) { continue; }
        if y + card_height * s > limit { return group.checks.len() - drawn; }
        drawn += 1;
        let (label, detail, fill) = match &check.state {
            CheckState::Pending => ("PENDING", "", PENDING),
            CheckState::Running => ("RUNNING", "", Color::rgb(82, 85, 144)),
            CheckState::Ok(detail) => (if scored { "PASS" } else { "OK" }, *detail, PASSED),
            CheckState::Fail(reason) => ("FAIL", *reason, FAILED),
            CheckState::Skip(reason) => ("SKIP", *reason, SKIPPED),
        };
        shapes::fill_rect(fb, x, y, width, card_height * s, fill);
        line(fb, check.name, x + 6 * s, y + 2 * s, width - 65 * s, Color::WHITE, s as usize);
        let label_width = label.len() as i32 * 6 * s;
        line(fb, label, x + width - 6 * s - label_width, y + 2 * s, label_width, Color::WHITE, s as usize);
        match (cards, &check.state) {
            (Cards::Full, _) => {
                line(fb, detail, x + 6 * s, y + 13 * s, width - 12 * s, Color::WHITE, s as usize);
            }
            // A one-line card keeps a failure's or skip's reason, after the name.
            (Cards::Compact, CheckState::Fail(reason) | CheckState::Skip(reason)) => {
                let reason_x = x + 6 * s + (check.name.len() as i32 + 2) * 6 * s;
                line(fb, reason, reason_x, y + 2 * s, x + width - 65 * s - reason_x, Color::WHITE,
                    s as usize);
            }
            (Cards::Compact, _) => {}
        }
        y += pitch * s;
    }
    if windowed {
        if y + card_height * s > limit { return group.checks.len() - drawn; }
        let hidden = |state: fn(&CheckState<'_>) -> bool| group.checks.iter().enumerate()
            .filter(|&(index, check)| !shown(group.checks, index, cap - 1) && state(&check.state))
            .count();
        let failed = hidden(|state| matches!(state, CheckState::Fail(_)));
        let mut more = Text::new();
        let _ = write!(more, "+{} MORE", group.checks.len() - drawn);
        if failed > 0 {
            let _ = write!(more, " ({} FAILED)", failed);
        }
        shapes::fill_rect(fb, x, y, width, card_height * s, PENDING);
        line(fb, more.as_str(), x + 6 * s, y + 2 * s, width - 12 * s, HEADING, s as usize);
    }
    0
}

/// Whether check `index` is in its group's window of `count` checks: the running
/// check first, then failures, then the checks nearest the focus (the running
/// check, else the last one finished), earlier ones first on a tie.
fn shown(checks: &[Check<'_>], index: usize, count: usize) -> bool {
    let focus = checks.iter().position(|check| matches!(check.state, CheckState::Running))
        .or_else(|| checks.iter().rposition(|check| !matches!(check.state, CheckState::Pending)))
        .unwrap_or(0);
    let key = |i: usize| {
        let class = match checks[i].state {
            CheckState::Running => 0,
            CheckState::Fail(_) => 1,
            _ => 2,
        };
        (class, i.abs_diff(focus), i)
    };
    let mine = key(index);
    (0..checks.len()).filter(|&other| key(other) < mine).count() < count
}

const LIVE_BACKGROUND: Color = Color::rgb(16, 27, 43);
const LIVE_LABEL: Color = Color::rgb(126, 150, 176);
const LIVE_DIGITS: Color = Color::rgb(136, 214, 255);
const LIVE_COUNTDOWN: Color = Color::rgb(245, 186, 72);
const LIVE_INSIDE: Color = Color::rgb(78, 214, 135);
const LIVE_OUTSIDE: Color = Color::rgb(240, 88, 98);
const LIVE_TRACK: Color = Color::rgb(36, 50, 69);
const LIVE_BAND: Color = Color::rgb(30, 84, 66);
const LIVE_TICK: Color = Color::rgb(98, 116, 140);

/// The pixel height of a line of text at `scale`.
fn text_height(scale: i32) -> i32 { 7 * scale }

/// Draw the live strip into `area`, as [`draw`] placed it, over whatever was there: a
/// caller may redraw it many times a second and flush just `area`.
///
/// Left to right, as far as the width allows: the clocks (CLOCK_MONOTONIC to the
/// millisecond, CLOCK_REALTIME as UTC wall time with a bar sweeping each second); the
/// wait (a countdown draining in real time, then the value the check reported when it
/// ended, or how long the check has run); and the check's latest values, each with
/// the range it accepts as a band on a gauge and the value as a needle, green inside
/// and red outside. A check that reports `expiries` and a `period` with a range is
/// drawn as a pulse train: its expiries at the observed period against ticks at the
/// programmed one, the middle of the period's range, across the wait. A taller strip
/// draws larger digits and more values.
pub fn draw_live(fb: &mut FrameBuf, area: LiveArea, live: &Live<'_>) {
    let s = area.s;
    let (x, y, w) = (area.x, area.y, area.w);
    shapes::fill_rect(fb, x, y, w, area.h, LIVE_BACKGROUND);
    shapes::fill_rect(fb, x, y, w, s, HEADING);
    let margin = 16 * s;
    let mut heading = Text::new();
    let _ = write!(heading, "LIVE  {}", if live.check.is_empty() { "suite finished" } else { live.check });
    line(fb, heading.as_str(), x + margin, y + 7 * s, w - 2 * margin, HEADING, 2 * s as usize);

    let top = y + 30 * s;
    let height = area.h - 34 * s;
    // Digit scales for the clocks and for the wait: larger in a tall strip.
    let tall = height >= 190 * s && w >= 1100 * s;
    let (clock_digits, wait_digits) = if tall { (5, 7) } else { (3, 4) };
    let gap = 22 * s;
    let mut left = x + margin;
    let right = x + w - margin;
    // Room for "00:00:00.000" and for "10000 ms" at those scales (see `line`).
    let clock_width = 12 * 6 * clock_digits * s;
    let wait_width = (8 * 6 * wait_digits * s).max(220 * s);
    if left + clock_width <= right {
        draw_clocks(fb, live, left, top, clock_width, s, clock_digits);
        left += clock_width + gap;
    }
    if left + wait_width <= right {
        draw_wait(fb, live, left, top, wait_width, s, wait_digits);
        left += wait_width + gap;
    }
    if left + 220 * s <= right {
        draw_values(fb, live, left, top, right - left, height, s);
    }
}

/// `n` thousandths as `<n / 1000>.<three digits>`.
fn thousandths(text: &mut Text, n: i64) {
    let sign = if n < 0 { "-" } else { "" };
    let n = n.unsigned_abs();
    let _ = write!(text, "{sign}{}.{:03}", n / 1000, n % 1000);
}

fn draw_clocks(fb: &mut FrameBuf, live: &Live<'_>, x: i32, mut y: i32, width: i32, s: i32, digits: i32) {
    let su = s as usize;
    line(fb, "CLOCK_MONOTONIC", x, y, width, LIVE_LABEL, su);
    y += 11 * s;
    let mut mono = Text::new();
    thousandths(&mut mono, live.monotonic_ns.div_euclid(1_000_000));
    let _ = write!(mono, " s");
    line(fb, mono.as_str(), x, y, width, LIVE_DIGITS, (digits * s) as usize);
    y += text_height(digits * s) + 9 * s;

    let secs = live.realtime_ns.div_euclid(1_000_000_000);
    let ms = live.realtime_ns.rem_euclid(1_000_000_000) / 1_000_000;
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let day_secs = secs.rem_euclid(86_400);
    let mut date = Text::new();
    let _ = write!(date, "CLOCK_REALTIME  {year:04}-{month:02}-{day:02} UTC");
    line(fb, date.as_str(), x, y, width, LIVE_LABEL, su);
    y += 11 * s;
    let mut clock = Text::new();
    let _ = write!(clock, "{:02}:{:02}:{:02}.{ms:03}", day_secs / 3600, day_secs / 60 % 60, day_secs % 60);
    line(fb, clock.as_str(), x, y, width, Color::WHITE, (digits * s) as usize);
    y += text_height(digits * s) + 7 * s;

    // The current second, sweeping, in tenths.
    shapes::fill_rect(fb, x, y, width, 5 * s, LIVE_TRACK);
    shapes::fill_rect(fb, x, y, (width as i64 * ms / 1000) as i32, 5 * s, LIVE_DIGITS);
    for tenth in 1..10 {
        shapes::fill_rect(fb, x + width * tenth / 10, y, s, 5 * s, LIVE_BACKGROUND);
    }
    y += 13 * s;
    if !live.check.is_empty() {
        let mut ran = Text::new();
        let _ = write!(ran, "this check has run ");
        thousandths(&mut ran, live.check_ms);
        let _ = write!(ran, " s");
        line(fb, ran.as_str(), x, y, width, LIVE_LABEL, su);
    }
}

/// The date of `days` since 1970-01-01 in the proleptic Gregorian calendar.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A value with its unit, in the next larger unit with one decimal when it runs long.
fn quantity(text: &mut Text, number: i64, unit: &str) {
    let larger = match unit {
        "ns" => Some("us"),
        "us" => Some("ms"),
        "ms" => Some("s"),
        _ => None,
    };
    match larger {
        Some(larger) if number.unsigned_abs() >= 10_000 => {
            let sign = if number < 0 { "-" } else { "" };
            let n = number.unsigned_abs();
            let _ = write!(text, "{sign}{}.{} {larger}", n / 1000, n % 1000 / 100);
        }
        _ if unit.is_empty() => { let _ = write!(text, "{number}"); }
        _ => { let _ = write!(text, "{number} {unit}"); }
    }
}

fn reading_color(reading: &Reading<'_>) -> Color {
    match reading.expect {
        Some((low, high)) if (low..=high).contains(&reading.number) => LIVE_INSIDE,
        Some(_) => LIVE_OUTSIDE,
        None => Color::WHITE,
    }
}

fn draw_wait(fb: &mut FrameBuf, live: &Live<'_>, x: i32, y: i32, width: i32, s: i32, digits: i32) {
    let su = s as usize;
    let now_ms = live.monotonic_ns.div_euclid(1_000_000);
    let mut label = Text::new();
    let mut big = Text::new();
    let mut caption = Text::new();
    let mut color = Color::WHITE;
    // The bar: what part of the whole is left, and its color.
    let mut bar: Option<(i64, i64, Color)> = None;
    match live.wait {
        _ if live.check.is_empty() => {
            let _ = write!(label, "SUITE");
            let _ = write!(big, "{}", live.idle);
        }
        Some(wait) => {
            let left = wait.until_ms - now_ms;
            if live.values_from != live.check {
                let _ = write!(label, "LAST CHECK ");
            }
            let _ = write!(label, "WAIT ");
            quantity(&mut label, wait.for_ms, "ms");
            if let Some(reading) = wait.result.and_then(|index| live.values.get(index)) {
                let _ = write!(label, "  ENDED");
                quantity(&mut big, reading.number, reading.unit);
                color = reading_color(reading);
                let _ = write!(caption, "{}", reading.name);
                if let Some((low, high)) = reading.expect {
                    let _ = write!(caption, ", expect ");
                    quantity(&mut caption, low, reading.unit);
                    let _ = write!(caption, " to ");
                    quantity(&mut caption, high, reading.unit);
                }
                bar = Some((0, 1, color));
            } else if left > 0 {
                let _ = write!(big, "{left} ms");
                color = LIVE_COUNTDOWN;
                let _ = write!(caption, "ends at ");
                thousandths(&mut caption, wait.until_ms);
                let _ = write!(caption, " s");
                bar = Some((left, wait.for_ms.max(1), LIVE_COUNTDOWN));
            } else {
                let _ = write!(label, "  OVER");
                let _ = write!(big, "+{} ms", -left);
                let _ = write!(caption, "past its end; the check has not said");
                bar = Some((0, 1, LIVE_COUNTDOWN));
            }
        }
        None => {
            let _ = write!(label, "CHECK RUNNING");
            thousandths(&mut big, live.check_ms);
            let _ = write!(big, " s");
        }
    }
    line(fb, label.as_str(), x, y, width, LIVE_LABEL, su);
    let mut scale = digits;
    while scale > 1 && font::text_width(big.as_str().as_bytes(), (scale * s) as usize) as i32 > width {
        scale -= 1;
    }
    line(fb, big.as_str(), x, y + 12 * s, width, color, (scale * s) as usize);
    let bar_y = y + 12 * s + text_height(digits * s) + 9 * s;
    let bar_h = if digits > 4 { 20 * s } else { 14 * s };
    if let Some((part, whole, fill)) = bar {
        shapes::fill_rect(fb, x, bar_y, width, bar_h, LIVE_TRACK);
        shapes::fill_rect(fb, x, bar_y, (width as i64 * part.clamp(0, whole) / whole) as i32, bar_h, fill);
        for quarter in 1..4 {
            shapes::fill_rect(fb, x + width * quarter / 4, bar_y, s, bar_h, LIVE_BACKGROUND);
        }
        if part == 0 {
            // Over: a mark at the end, in the result's color.
            shapes::fill_rect(fb, x + width - 4 * s, bar_y - 3 * s, 4 * s, bar_h + 6 * s, fill);
        }
    }
    line(fb, caption.as_str(), x, bar_y + bar_h + 7 * s, width, LIVE_LABEL, su);
}

/// The latest reading named `name`.
fn named<'a>(values: &'a [Reading<'a>], name: &str) -> Option<&'a Reading<'a>> {
    values.iter().rev().find(|reading| reading.name == name)
}

fn draw_values(fb: &mut FrameBuf, live: &Live<'_>, x: i32, y: i32, width: i32, height: i32, s: i32) {
    let mut heading = Text::new();
    let _ = write!(heading, "MEASURED ({})", live.values.len());
    if live.values_from != live.check && !live.values.is_empty() {
        let _ = write!(heading, " in the last check, {}", live.values_from);
    }
    line(fb, heading.as_str(), x, y, width, LIVE_LABEL, s as usize);
    let row = 44 * s;
    let mut rows = ((height - 12 * s) / row).max(0) as usize;
    let mut row_y = y + 13 * s;
    let pulses = match (named(live.values, "expiries"), named(live.values, "period"), live.wait) {
        (Some(expiries), Some(period), Some(wait)) if rows > 0 && period.expect.is_some() => {
            Some((expiries.number, period, wait.for_ms))
        }
        _ => None,
    };
    if pulses.is_some() { rows -= 1; }
    let first = live.values.len().saturating_sub(rows);
    for reading in &live.values[first..] {
        draw_gauge(fb, reading, x, row_y, width, s);
        row_y += row;
    }
    if let Some((expiries, period, window_ms)) = pulses {
        draw_pulses(fb, expiries, period, window_ms, (x, row_y, width), s);
    }
}

fn draw_gauge(fb: &mut FrameBuf, reading: &Reading<'_>, x: i32, y: i32, width: i32, s: i32) {
    let su = s as usize;
    let mut number = Text::new();
    quantity(&mut number, reading.number, reading.unit);
    let left = (width * 2 / 5).min(196 * s);
    line(fb, reading.name, x, y, left, LIVE_LABEL, su);
    let scale = if font::text_width(number.as_str().as_bytes(), 2 * su) as i32 <= left - 6 * s { 2 } else { 1 };
    line(fb, number.as_str(), x, y + 11 * s, left, reading_color(reading), scale * su);
    let Some((low, high)) = reading.expect else { return };
    let (gx, gw) = (x + left, width - left);
    if gw < 60 * s { return; }
    let pad = ((high - low) / 2).max(1);
    let (axis_low, axis_high) = (low - pad, high + pad);
    let at = |v: i64| gx + ((gw - 1) as i128 * (v.clamp(axis_low, axis_high) - axis_low) as i128
        / (axis_high - axis_low) as i128) as i32;
    let track_y = y + 9 * s;
    shapes::fill_rect(fb, gx, track_y, gw, 10 * s, LIVE_TRACK);
    shapes::fill_rect(fb, at(low), track_y, (at(high) - at(low)).max(s), 10 * s, LIVE_BAND);
    let color = reading_color(reading);
    let needle = at(reading.number);
    shapes::fill_rect(fb, needle - s, track_y - 4 * s, 3 * s, 18 * s, color);
    if reading.number < axis_low || reading.number > axis_high {
        // Off the scale: an arrow at the edge it went past.
        let dir = if reading.number < axis_low { 1 } else { -1 };
        for i in 0..4 * s {
            shapes::fill_rect(fb, needle + dir * (3 * s + i), track_y + i, s, 10 * s - 2 * i, color);
        }
    }
    let mut low_label = Text::new();
    quantity(&mut low_label, low, reading.unit);
    let mut high_label = Text::new();
    quantity(&mut high_label, high, reading.unit);
    let label_y = y + 23 * s;
    let low_width = font::text_width(low_label.as_str().as_bytes(), su) as i32;
    let high_width = font::text_width(high_label.as_str().as_bytes(), su) as i32;
    let low_x = (at(low) - low_width / 2).clamp(gx, gx + gw - low_width);
    line(fb, low_label.as_str(), low_x, label_y, low_width + 6 * s, LIVE_LABEL, su);
    let high_x = (at(high) - high_width / 2).clamp(gx, gx + gw - high_width);
    if high_x > low_x + low_width + 6 * s {
        line(fb, high_label.as_str(), high_x, label_y, high_width + 6 * s, LIVE_LABEL, su);
    }
}

fn draw_pulses(fb: &mut FrameBuf, expiries: i64, period: &Reading<'_>, window_ms: i64, (x, y, width): (i32, i32, i32),
    s: i32) {
    let Some((low, high)) = period.expect else { return };
    let programmed = (low + high) / 2;
    let unit_ns = match period.unit { "ns" => 1, "us" => 1_000, "ms" => 1_000_000, "s" => 1_000_000_000, _ => return };
    let window = window_ms * 1_000_000;
    if programmed <= 0 || period.number <= 0 || window <= 0 { return; }
    let mut label = Text::new();
    let _ = write!(label, "{expiries} EXPIRIES  every ");
    quantity(&mut label, period.number, period.unit);
    let _ = write!(label, ", programmed ");
    quantity(&mut label, programmed, period.unit);
    line(fb, label.as_str(), x, y, width, LIVE_LABEL, s as usize);
    let train_y = y + 11 * s;
    let train_h = 24 * s;
    shapes::fill_rect(fb, x, train_y + train_h - s, width, s, LIVE_TICK);
    let at = |ns: i64| x + ((width - 3 * s) as i128 * ns as i128 / window as i128) as i32;
    for k in 1..=(window / (programmed * unit_ns)).min(1000) {
        shapes::fill_rect(fb, at(k * programmed * unit_ns), train_y, s, train_h, LIVE_TICK);
    }
    let color = reading_color(period);
    for k in 1..=expiries.clamp(0, 1000) {
        let ns = k * period.number * unit_ns;
        if ns > window { break; }
        shapes::fill_rect(fb, at(ns), train_y + 7 * s, 3 * s, train_h - 7 * s, color);
    }
}
