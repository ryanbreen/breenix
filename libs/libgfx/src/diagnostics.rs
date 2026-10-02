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
pub fn draw(fb: &mut FrameBuf, panel: &Panel<'_>) {
    let width = fb.width as i32;
    let height = fb.height as i32;
    if width < 160 || height < 120 { return; }
    let fits_capped = |s: i32, cards: Cards, room_for_output: bool, cap: usize| {
        layout_groups(None, panel, s, cards, width, i32::MAX, cap).0
            <= bottom(height, s, room_for_output)
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

    let body_bottom = bottom(height, s, false);
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

/// The lowest y the body may reach at scale `s`, leaving room for the verdict bar and,
/// when `room_for_output`, for the output heading and a few output lines.
fn bottom(height: i32, s: i32, room_for_output: bool) -> i32 {
    let reserve = if room_for_output { (14 + 5 * 11) * s } else { 0 };
    height - 42 * s - reserve
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
