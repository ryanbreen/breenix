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
pub fn draw(fb: &mut FrameBuf, panel: &Panel<'_>) {
    let width = fb.width as i32;
    let height = fb.height as i32;
    if width < 160 || height < 120 { return; }
    fb.clear(Color::rgb(12, 19, 31));
    shapes::fill_rect(fb, 0, 0, width, 54, Color::rgb(24, 39, 59));
    line(fb, panel.title, 16, 10, width - 32, Color::WHITE, 2);
    line(fb, panel.subtitle, 16, 36, width - 32, Color::rgb(161, 183, 204), 1);

    let mut y = 64i32;
    let gap = 6i32;
    let columns = if width >= 360 { 2 } else { 1 };
    let tile_width = (width - 32 - gap * (columns - 1)) / columns;
    for group in panel.groups {
        if y + 16 >= height - 42 { break; }
        line(fb, group.title, 16, y, width - 32, Color::rgb(110, 168, 212), 1);
        y += 13;
        for (index, check) in group.checks.iter().enumerate() {
            let row_y = y + (index as i32 / columns) * 26;
            if row_y + 23 >= height - 42 { break; }
            let x = 16 + (index as i32 % columns) * (tile_width + gap);
            let (label, detail, fill) = match &check.state {
                CheckState::Pending => ("PENDING", "", Color::rgb(57, 67, 80)),
                CheckState::Running => ("RUNNING", "", Color::rgb(82, 85, 144)),
                CheckState::Ok(detail) => ("OK", *detail, Color::rgb(26, 112, 77)),
                CheckState::Fail(reason) => ("FAIL", *reason, Color::rgb(145, 50, 58)),
            };
            shapes::fill_rect(fb, x, row_y, tile_width, 23, fill);
            line(fb, check.name, x + 6, row_y + 2, tile_width - 65, Color::WHITE, 1);
            line(fb, label, x + tile_width - 54, row_y + 2, 50, Color::WHITE, 1);
            line(fb, detail, x + 6, row_y + 13, tile_width - 12, Color::WHITE, 1);
        }
        y += ((group.checks.len() as i32 + columns - 1) / columns) * 26 + 4;
    }

    if !panel.output.is_empty() && y + 25 < height - 42 {
        line(fb, "RECENT OUTPUT", 16, y, width - 32, Color::rgb(110, 168, 212), 1);
        y += 14;
        let count = ((height - 48 - y) / 11).max(0) as usize;
        let first = panel.output.len().saturating_sub(count);
        for output in &panel.output[first..] {
            line(fb, output, 16, y, width - 32, Color::WHITE, 1);
            y += 11;
        }
    }

    let (text, fill) = match &panel.verdict {
        Verdict::Running(text) => (*text, Color::rgb(48, 67, 96)),
        Verdict::Passed(text) => (*text, Color::rgb(26, 112, 77)),
        Verdict::Failed(text) => (*text, Color::rgb(145, 50, 58)),
    };
    shapes::fill_rect(fb, 0, height - 38, width, 38, fill);
    line(fb, text, 16, height - 27, width - 32, Color::WHITE, 1);
}
