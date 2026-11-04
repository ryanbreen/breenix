//! Pure serial-derived suite state and B36 panel geometry, without drawing or I/O.
//! The console renderer can consume this model in thread context.

use alloc::{format, string::String, vec::Vec};

pub const BACKGROUND: [u8; 3] = [12, 19, 31];
pub const TEXT: [u8; 3] = [214, 225, 244];
pub const PASS: [u8; 3] = [26, 112, 77];
pub const FAIL: [u8; 3] = [145, 50, 58];
pub const PENDING: [u8; 3] = [48, 67, 96];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelMode {
    Empty,
    Stages,
    Suite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Skip,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuiteStatus {
    suite: String,
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    cases: Vec<(String, Outcome)>,
    started_ms: u64,
    done_ms: Option<u64>,
}

impl SuiteStatus {
    /// Observe one complete captured serial line, timestamped by a monotonic clock.
    /// Invalid records leave the model unchanged, including its elapsed-time origin.
    pub fn observe(&mut self, line: &[u8], now_ms: u64) -> bool {
        let Some(record) = parse(line) else {
            return false;
        };
        match record {
            Record::Start { id, total } => {
                *self = Self {
                    suite: String::from(id),
                    total,
                    started_ms: now_ms,
                    ..Self::default()
                };
            }
            Record::Case { id, name, outcome } => {
                if id != self.suite
                    || self.done_ms.is_some()
                    || self.cases.len() >= self.total
                    || self.cases.iter().any(|(seen, _)| seen == name)
                {
                    return false;
                }
                match outcome {
                    Outcome::Pass => self.passed += 1,
                    Outcome::Fail => self.failed += 1,
                    Outcome::Skip => self.skipped += 1,
                }
                self.cases.push((String::from(name), outcome));
            }
            Record::Done { id, counts } => {
                if id != self.suite
                    || self.done_ms.is_some()
                    || counts != [self.passed, self.failed, self.skipped, self.total]
                    || self.cases.len() != self.total
                {
                    return false;
                }
                self.done_ms = Some(now_ms.max(self.started_ms));
            }
        }
        true
    }

    pub fn passed(&self) -> usize {
        self.passed
    }
    pub fn failed(&self) -> usize {
        self.failed
    }
    pub fn skipped(&self) -> usize {
        self.skipped
    }
    pub fn total(&self) -> usize {
        self.total
    }
    pub fn pending(&self) -> usize {
        self.total - self.cases.len()
    }
    /// CASE lines report results; this is the latest observed case, not a prediction.
    pub fn current_case(&self) -> Option<&str> {
        self.cases.last().map(|(name, _)| name.as_str())
    }
    pub fn elapsed(&self, now_ms: u64) -> (u64, u64) {
        let seconds = if self.suite.is_empty() {
            0
        } else {
            self.done_ms
                .unwrap_or(now_ms)
                .saturating_sub(self.started_ms)
                / 1000
        };
        (seconds / 60, seconds % 60)
    }
    pub fn mode(&self, boot_complete: bool) -> PanelMode {
        if !self.suite.is_empty() {
            PanelMode::Suite
        } else if boot_complete {
            PanelMode::Empty
        } else {
            PanelMode::Stages
        }
    }
    pub fn heading(&self, now_ms: u64) -> Option<String> {
        if self.suite.is_empty() {
            return None;
        }
        let (minutes, seconds) = self.elapsed(now_ms);
        let mut text = format!(
            "{}: {} passed · {} failed · {} pending of {} · {}:{:02} · case: {}",
            self.suite,
            self.passed,
            self.failed,
            self.pending(),
            self.total,
            minutes,
            seconds,
            self.current_case().unwrap_or("waiting")
        );
        if self.skipped > 0 {
            text.push_str(&format!(" · {} skipped", self.skipped));
        }
        Some(text)
    }
}

enum Record<'a> {
    Start {
        id: &'a str,
        total: usize,
    },
    Case {
        id: &'a str,
        name: &'a str,
        outcome: Outcome,
    },
    Done {
        id: &'a str,
        counts: [usize; 4],
    },
}

fn name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn selection(text: &str) -> bool {
    text.split('/').all(name)
}

fn number(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn field(text: &str, key: &str) -> Option<usize> {
    number(text.strip_prefix(key)?)
}

fn duration_and_hash(text: &str) -> bool {
    let mut fields = text.split(' ');
    if fields.next().and_then(|v| field(v, "ms=")).is_none() {
        return false;
    }
    if let Some(hash) = fields.next() {
        let Some(hex) = hash.strip_prefix("sha256=") else {
            return false;
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return false;
        }
    }
    fields.next().is_none()
}

fn parse(line: &[u8]) -> Option<Record<'_>> {
    // Match the existing serial scanner's complete-line limit; discard a cut record.
    if line.len() > 1024 {
        return None;
    }
    let text = core::str::from_utf8(line).ok()?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    if text.chars().any(char::is_control) {
        return None;
    }
    let (id, rest) = text.strip_prefix("SUITE ")?.split_once(' ')?;
    if !name(id) {
        return None;
    }
    let (kind, rest) = rest.split_once(' ')?;
    match kind {
        "START" => {
            let mut fields = rest.split(' ');
            let total = field(fields.next()?, "cases=")?;
            let (mut only, mut pace) = (false, false);
            for extra in fields {
                if let Some(value) = extra.strip_prefix("only=") {
                    if only || !selection(value) {
                        return None;
                    }
                    only = true;
                } else if let Some(value) = extra.strip_prefix("pace_ms=") {
                    if pace || number(value).is_none() {
                        return None;
                    }
                    pace = true;
                } else {
                    return None;
                }
            }
            Some(Record::Start { id, total })
        }
        "CASE" => {
            let (case, rest) = rest.split_once(' ')?;
            let (category, case_id) = case.split_once('/')?;
            if !name(category) || !name(case_id) {
                return None;
            }
            let (verdict, fields) = rest.split_once(' ')?;
            let outcome = match verdict {
                "PASS" if duration_and_hash(fields) => Outcome::Pass,
                "FAIL" => {
                    let (timing, message) = fields.split_once(" msg=")?;
                    if !duration_and_hash(timing) || message.trim().is_empty() {
                        return None;
                    }
                    Outcome::Fail
                }
                "SKIP" if !fields.strip_prefix("msg=")?.trim().is_empty() => Outcome::Skip,
                _ => return None,
            };
            Some(Record::Case {
                id,
                name: case,
                outcome,
            })
        }
        "DONE" => {
            let mut fields = rest.split(' ');
            let mut counts = [0; 4];
            for (value, key) in counts
                .iter_mut()
                .zip(["passed=", "failed=", "skipped=", "total="])
            {
                *value = field(fields.next()?, key)?;
            }
            if let Some(extra) = fields.next() {
                if !selection(extra.strip_prefix("only=")?) {
                    return None;
                }
            }
            if fields.next().is_some() {
                return None;
            }
            Some(Record::Done { id, counts })
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && y >= self.y && x - self.x < self.width && y - self.y < self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelGeometry {
    pub panel: Rect,
    pub heading: Rect,
    pub track: Rect,
    pub label: Rect,
    pub ribbon: Rect,
    /// Positive for the whole-scale stage layout; 0 for a native pixel layout.
    pub stage_scale: usize,
}

impl PanelGeometry {
    /// `glyph` and `line` come from the font selected for the full framebuffer.
    pub fn for_display(width: usize, height: usize, glyph: usize, line: usize) -> Option<Self> {
        if glyph == 0 || line < 6 {
            return None;
        }
        let s = width / 1280;
        if s > 0 && width == 1280 * s && height == 800 * s {
            let rect = |x, y, w, h| Rect::new(x * s, y * s, w * s, h * s);
            return Some(Self {
                panel: rect(0, 704, 1280, 96),
                heading: rect(16, 712, 1248, 24),
                track: rect(16, 744, 1248, 12),
                label: rect(16, 768, 80, 16),
                ribbon: rect(112, 772, 1152, 6),
                stage_scale: s,
            });
        }
        let reserved = line.checked_mul(2)?.checked_add(48)?;
        if width <= glyph.checked_mul(14)? || height < reserved {
            return None;
        }
        let top = height - reserved;
        Some(Self {
            panel: Rect::new(0, top, width, reserved),
            heading: Rect::new(2 * glyph, top + 8, width - 4 * glyph, line),
            track: Rect::new(2 * glyph, top + line + 16, width - 4 * glyph, 8),
            label: Rect::new(2 * glyph, top + line + 32, 10 * glyph, line),
            ribbon: Rect::new(
                12 * glyph,
                top + line + 32 + (line - 6) / 2,
                width - 14 * glyph,
                6,
            ),
            stage_scale: 0,
        })
    }

    pub fn text_rows(&self, line_height: usize) -> usize {
        self.panel.y.checked_div(line_height).unwrap_or(0).min(255)
    }

    /// Pure colour model for the progress track, separate from a future painter.
    pub fn progress_colour(&self, state: &SuiteStatus, x: usize, y: usize) -> Option<[u8; 3]> {
        if state.suite.is_empty() || !self.track.contains(x, y) {
            return None;
        }
        let scale = self.stage_scale.max(1);
        let (x, width) = ((x - self.track.x) / scale, self.track.width / scale);
        if state.total == 0 {
            return Some(PENDING);
        }
        for (i, (_, outcome)) in state
            .cases
            .iter()
            .filter(|(_, o)| *o != Outcome::Skip)
            .enumerate()
        {
            if (fraction(width, i, state.total)..fraction(width, i + 1, state.total)).contains(&x) {
                return Some(if *outcome == Outcome::Fail {
                    FAIL
                } else {
                    PASS
                });
            }
        }
        Some(PENDING)
    }

    /// Pure colour model for the relocated stage row, including the exact gap width.
    pub fn stage_colour(&self, reached: &[bool], x: usize, y: usize) -> Option<[u8; 3]> {
        if reached.is_empty() || !self.ribbon.contains(x, y) {
            return None;
        }
        let scale = self.stage_scale.max(1);
        let (x, width) = ((x - self.ribbon.x) / scale, self.ribbon.width / scale);
        for (i, &lit) in reached.iter().enumerate() {
            let (a, b) = (
                fraction(width, i, reached.len()),
                fraction(width, i + 1, reached.len()),
            );
            if (a..b).contains(&x) {
                return Some(if x >= b.saturating_sub(2).max(a) {
                    BACKGROUND
                } else if lit {
                    PASS
                } else {
                    PENDING
                });
            }
        }
        None
    }
}

fn fraction(width: usize, numerator: usize, denominator: usize) -> usize {
    ((width as u128 * numerator as u128) / denominator as u128) as usize
}
