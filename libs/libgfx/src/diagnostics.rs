//! Reusable, syscall-free diagnostics panel. The caller owns the framebuffer and flushes it.

use crate::{color::Color, font, framebuf::FrameBuf, shapes};

/// A check's current state and the detail or failure reason to display.
pub enum CheckState<'a> {
    Pending,
    Running,
    Ok(&'a str),
    Fail(&'a str),
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
/// the cards drop to one line (a failure keeps its reason, after the name); if they still
/// do not fit, the cards that would reach the bar are left out and a line says so.
pub fn draw(fb: &mut FrameBuf, panel: &Panel<'_>) {
    let width = fb.width as i32;
    let height = fb.height as i32;
    if width < 160 || height < 120 { return; }
    let fits = |s: i32, cards: Cards, room_for_output: bool| {
        layout_groups(None, panel.groups, s, cards, width, i32::MAX).0
            <= bottom(height, s, room_for_output)
    };
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

    let body_bottom = bottom(height, s, false);
    let line_height = 11 * s;
    let mut y = if fits(s, cards, false) {
        layout_groups(Some(&mut *fb), panel.groups, s, cards, width, body_bottom).0
    } else {
        // Keep the last body line for a note about the checks left out.
        let limit = body_bottom - line_height;
        let (groups_bottom, hidden) =
            layout_groups(Some(&mut *fb), panel.groups, s, cards, width, limit);
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

fn group_height(group: &Group<'_>, s: i32, cards: Cards) -> i32 {
    (13 + group.checks.len() as i32 * cards.size().1 + 8) * s
}

/// Lay the groups out below the banner at scale `s`: in order, down one column and on
/// to the next, splitting them so the tallest column is as short as possible. Draws
/// when given a framebuffer, leaving out any heading or card that would extend below
/// `limit`. Returns the bottom of the tallest column as laid out and how many checks
/// were left out.
fn layout_groups(mut fb: Option<&mut FrameBuf>, groups: &[Group<'_>], s: i32, cards: Cards,
    width: i32, limit: i32) -> (i32, usize) {
    let margin = 16 * s;
    let gap = 12 * s;
    let top = 64 * s;
    let columns = ((width - 2 * margin) / (200 * s)).clamp(1, 4).min(groups.len().max(1) as i32);
    let column_width = (width - 2 * margin - gap * (columns - 1)) / columns;
    // Columns needed when each holds at most `column_limit` of height.
    let columns_for = |column_limit: i32| {
        let (mut used, mut filled) = (1, 0);
        for group in groups {
            let height = group_height(group, s, cards);
            if filled > 0 && filled + height > column_limit { used += 1; filled = 0; }
            filled += height;
        }
        used
    };
    let mut low = groups.iter().map(|group| group_height(group, s, cards)).max().unwrap_or(0);
    let mut high: i32 = groups.iter().map(|group| group_height(group, s, cards)).sum();
    while low < high {
        let mid = (low + high) / 2;
        if columns_for(mid) <= columns { high = mid; } else { low = mid + 1; }
    }
    let (mut column, mut filled, mut tallest, mut hidden) = (0, 0, 0, 0);
    for group in groups {
        let height = group_height(group, s, cards);
        if filled > 0 && filled + height > low { column += 1; filled = 0; }
        if let Some(fb) = fb.as_deref_mut() {
            let x = margin + column * (column_width + gap);
            hidden += draw_group(fb, group, x, top + filled, column_width, s, cards, limit);
        }
        filled += height;
        tallest = tallest.max(filled);
    }
    (top + tallest, hidden)
}

/// Draw one group's heading and cards, leaving out any that would extend below
/// `limit`. Returns how many checks were left out.
fn draw_group(fb: &mut FrameBuf, group: &Group<'_>, x: i32, mut y: i32, width: i32, s: i32,
    cards: Cards, limit: i32) -> usize {
    if y + 13 * s > limit { return group.checks.len(); }
    line(fb, group.title, x, y, width, HEADING, s as usize);
    y += 13 * s;
    let (card_height, pitch) = cards.size();
    for (index, check) in group.checks.iter().enumerate() {
        if y + card_height * s > limit { return group.checks.len() - index; }
        let (label, detail, fill) = match &check.state {
            CheckState::Pending => ("PENDING", "", Color::rgb(57, 67, 80)),
            CheckState::Running => ("RUNNING", "", Color::rgb(82, 85, 144)),
            CheckState::Ok(detail) => ("OK", *detail, Color::rgb(26, 112, 77)),
            CheckState::Fail(reason) => ("FAIL", *reason, Color::rgb(145, 50, 58)),
        };
        shapes::fill_rect(fb, x, y, width, card_height * s, fill);
        line(fb, check.name, x + 6 * s, y + 2 * s, width - 65 * s, Color::WHITE, s as usize);
        let label_width = label.len() as i32 * 6 * s;
        line(fb, label, x + width - 6 * s - label_width, y + 2 * s, label_width, Color::WHITE, s as usize);
        match (cards, &check.state) {
            (Cards::Full, _) => {
                line(fb, detail, x + 6 * s, y + 13 * s, width - 12 * s, Color::WHITE, s as usize);
            }
            // A one-line card keeps a failure's reason, after the name.
            (Cards::Compact, CheckState::Fail(reason)) => {
                let reason_x = x + 6 * s + (check.name.len() as i32 + 2) * 6 * s;
                line(fb, reason, reason_x, y + 2 * s, x + width - 65 * s - reason_x, Color::WHITE,
                    s as usize);
            }
            (Cards::Compact, _) => {}
        }
        y += pitch * s;
    }
    0
}
