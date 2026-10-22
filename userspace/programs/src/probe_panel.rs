//! Probe's status panel on the GPU core: the banner, the checks in groups and the verdict
//! bar, drawn with BreenGL (libgfx's 5×7 font and rectangles) into a GPU surface. Its
//! layout is the one the CPU diagnostics panel gave probe, which the GPU-only rule retires
//! from programs (`docs/gui/gpu.md` section 2.4).

use libgfx::{color::Color, font, shapes, surface::Surface};

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

const HEADING: Color = Color::rgb(110, 168, 212);
const PENDING: Color = Color::rgb(57, 67, 80);
const PASSED: Color = Color::rgb(26, 112, 77);
const FAILED: Color = Color::rgb(145, 50, 58);

fn line(fb: &mut Surface, text: &str, x: i32, y: i32, width: i32, color: Color, scale: usize) {
    if x < 0 || y < 0 || width <= 0 || y as usize >= fb.height() { return; }
    let max = (width as usize / (6 * scale)).min(text.len());
    let bytes = text.as_bytes();
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) { end -= 1; }
    font::draw_text(fb, &bytes[..end], x as usize, y as usize, color, scale);
}

/// Render the panel over the whole surface: a full-width banner and verdict bar, the
/// groups split in order across as many columns of at least 200 pixels as fit (at most
/// four, and no more than there are groups), then the recent output across the full
/// width. It draws at double size whenever the groups still fit that way. When the groups
/// do not fit above the verdict bar with two-line check cards, the cards drop to one line
/// (a failure keeps its reason, after the name). If they still do not fit, each group
/// longer than the space allows shows a window of its checks around the running one, and a
/// last row counting the checks it leaves out. Only if even the group headings do not fit
/// are whole groups left out, with a line saying so. A surface under 160x120 is left as it
/// was.
pub fn draw(fb: &mut Surface, panel: &Panel<'_>) {
    let width = fb.width() as i32;
    let height = fb.height() as i32;
    if width < 160 || height < 120 { return; }
    let body_limit = |s: i32, room_for_output: bool| bottom(height, s, room_for_output);
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

    let body_bottom = body_limit(s, false);
    let line_height = 11 * s;
    // The most checks a group may show (its window, the last row counting the rest)
    // so that the groups fit; at least two, the running check and that row.
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

/// The height of a group's title at scale 1.
const HEADING_HEIGHT: i32 = 13;

/// A group's height when it shows at most `cap` rows of checks.
fn group_height(group: &Group<'_>, s: i32, cards: Cards, cap: usize) -> i32 {
    (HEADING_HEIGHT + group.checks.len().min(cap) as i32 * cards.size().1 + 8) * s
}

/// Lay the panel's groups out below the banner at scale `s`: in order, down one column
/// and on to the next, splitting them so the tallest column is as short as possible, each
/// group showing at most `cap` rows (see `draw_group`). Draws when given a surface, leaving
/// out any heading or card that would extend below `limit`. Returns the bottom of the
/// tallest column as laid out and how many checks were left out below `limit`.
fn layout_groups(mut fb: Option<&mut Surface>, panel: &Panel<'_>, s: i32, cards: Cards,
    width: i32, limit: i32, cap: usize) -> (i32, usize) {
    let groups = panel.groups;
    let margin = 16 * s;
    let gap = 12 * s;
    let top = 64 * s;
    let columns = ((width - 2 * margin) / (200 * s)).clamp(1, 4).min(groups.len().max(1) as i32);
    let column_width = (width - 2 * margin - gap * (columns - 1)) / columns;
    // Columns needed when each holds at most `column_limit` of height.
    let columns_for = |column_limit: i32| {
        let (mut used, mut filled) = (1, 0);
        for group in groups {
            let height = group_height(group, s, cards, cap);
            if filled > 0 && filled + height > column_limit { used += 1; filled = 0; }
            filled += height;
        }
        used
    };
    let mut low = groups.iter().map(|group| group_height(group, s, cards, cap)).max().unwrap_or(0);
    let mut high: i32 = groups.iter().map(|group| group_height(group, s, cards, cap)).sum();
    while low < high {
        let mid = (low + high) / 2;
        if columns_for(mid) <= columns { high = mid; } else { low = mid + 1; }
    }
    let (mut column, mut filled, mut tallest, mut hidden) = (0, 0, 0, 0);
    for group in groups {
        let height = group_height(group, s, cards, cap);
        if filled > 0 && filled + height > low { column += 1; filled = 0; }
        if let Some(fb) = fb.as_deref_mut() {
            let x = margin + column * (column_width + gap);
            hidden += draw_group(fb, group, x, top + filled, column_width, s, cards, limit, cap);
        }
        filled += height;
        tallest = tallest.max(filled);
    }
    (top + tallest, hidden)
}

/// Draw one group's heading and cards, leaving out any that would extend below `limit`. A
/// group of more than `cap` checks shows `cap - 1` of them (`shown`) and a row counting the
/// rest. Returns how many checks were left out below `limit`.
fn draw_group(fb: &mut Surface, group: &Group<'_>, x: i32, mut y: i32, width: i32, s: i32,
    cards: Cards, limit: i32, cap: usize) -> usize {
    let heading = HEADING_HEIGHT * s;
    if y + heading > limit { return group.checks.len(); }
    line(fb, group.title, x, y, width, HEADING, s as usize);
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
            CheckState::Ok(detail) => ("OK", *detail, PASSED),
            CheckState::Fail(reason) => ("FAIL", *reason, FAILED),
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
    if windowed {
        if y + card_height * s > limit { return group.checks.len() - drawn; }
        let failed = group.checks.iter().enumerate()
            .filter(|&(index, check)| !shown(group.checks, index, cap - 1) && matches!(check.state, CheckState::Fail(_)))
            .count();
        let mut more = format!("+{} MORE", group.checks.len() - drawn);
        if failed > 0 {
            more.push_str(&format!(" ({} FAILED)", failed));
        }
        shapes::fill_rect(fb, x, y, width, card_height * s, PENDING);
        line(fb, &more, x + 6 * s, y + 2 * s, width - 12 * s, HEADING, s as usize);
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
