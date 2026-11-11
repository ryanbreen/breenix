//! Shared keyboard scripts and pointer resolution over session input.
use libbui::tree::{Flag, Node};

// Widen before forming half-open endpoints: tree rectangles carry signed i64s.
fn edges([x, y, w, h]: [i64; 4]) -> Option<[i128; 4]> {
    if w <= 0 || h <= 0 {
        return None;
    }
    Some([
        i128::from(x),
        i128::from(y),
        i128::from(x) + i128::from(w),
        i128::from(y) + i128::from(h),
    ])
}
fn intersection(a: [i128; 4], b: [i128; 4]) -> Option<[i128; 4]> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// Resolve window-local bounds and clip against exposed rectangles in screen pixels.
/// Choose the largest intersection, breaking equal-area ties topmost then leftmost.
/// The caller confirms this screen point with the current scene's window hit test.
pub fn click_target(
    node: &Node,
    origin: (i32, i32),
    visible_region: &[[i64; 4]],
) -> Result<(i32, i32), String> {
    let missing = || format!("input-target: {} has no visible area", node.key);
    if !node.flags.contains(&Flag::Visible) {
        return Err(missing());
    }
    let bounds = edges(node.bounds).ok_or_else(missing)?;
    let clip = edges(node.clip).ok_or_else(missing)?;
    let mut local = intersection(bounds, clip).ok_or_else(missing)?;
    for i in [0, 2] {
        local[i] += i128::from(origin.0);
    }
    for i in [1, 3] {
        local[i] += i128::from(origin.1);
    }
    let best = visible_region
        .iter()
        .filter_map(|&rect| intersection(local, edges(rect)?))
        .max_by_key(|r| ((r[2] - r[0]) * (r[3] - r[1]), -r[1], -r[0]));
    let r = best.ok_or_else(missing)?;
    let x = r[0] + (r[2] - r[0]) / 2;
    let y = r[1] + (r[3] - r[1]) / 2;
    let x = i32::try_from(x)
        .map_err(|_| format!("input-target: {} x outside pointer range", node.key))?;
    let y = i32::try_from(y)
        .map_err(|_| format!("input-target: {} y outside pointer range", node.key))?;
    Ok((x, y))
}

/// Home to (0,0), then reach the screen point with exact relative deltas.
/// bwm adds each delta without acceleration and clamps it to 0..dimension-1.
pub fn home_then_move(
    screen: (usize, usize),
    target: (i32, i32),
) -> Result<[(i32, i32); 2], String> {
    let width =
        i32::try_from(screen.0).map_err(|_| "input-target: screen width outside pointer range")?;
    let height =
        i32::try_from(screen.1).map_err(|_| "input-target: screen height outside pointer range")?;
    if width <= 0
        || height <= 0
        || target.0 < 0
        || target.1 < 0
        || target.0 >= width
        || target.1 >= height
    {
        return Err(format!(
            "input-target: point {target:?} outside screen {screen:?}"
        ));
    }
    Ok([(-width, -height), target])
}

/// Wake a newly observed app through the middle of its exposed content.
pub fn tree_probe_point(
    content: [i64; 4],
    screen: [i64; 4],
    covers: &[[i64; 4]],
) -> Result<(i32, i32), String> {
    let rect = exposed_region(content, screen, covers).into_iter()
        .max_by_key(|r| (i128::from(r[2]) * i128::from(r[3]), -i128::from(r[1]), -i128::from(r[0])))
        .ok_or("input tree has no exposed content")?;
    let x = i32::try_from(i128::from(rect[0]) + i128::from(rect[2]) / 2)
        .map_err(|_| "input tree probe x outside pointer range")?;
    let y = i32::try_from(i128::from(rect[1]) + i128::from(rect[3]) / 2)
        .map_err(|_| "input tree probe y outside pointer range")?;
    Ok((x, y))
}

/// Sample the baseline with the mapping held, then wait for a later serialization.
/// A cumulative count can describe a tree left by a reader that already detached.
pub fn tree_ready_after_attach(count: u64, baseline: u64) -> bool {
    count > baseline
}

/// Metadata-only acknowledgments keep the current pixel/tree publication.
pub fn tree_capture_current(captured: u64, latest: u64, before: u64, after: u64) -> bool {
    captured != 0 && captured <= latest && before != 0 && before == after
}

/// A stable widget address, resolved afresh for each new operation.
#[derive(Clone, Debug)]
pub struct Target {
    pub window: u64,
    pub key: String,
}
impl Target {
    pub fn new(window: u64, key: &str) -> Self {
        Self {
            window,
            key: key.into(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Resolved {
    pub window: u64,
    pub point: (i32, i32),
    pub scene: u64,
    pub commit: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub serial: u64,
    pub window: u64,
}
#[derive(Debug)]
pub struct Click {
    pub point: (i32, i32),
    pub press: Delivery,
    pub release: Delivery,
}
#[derive(Debug)]
pub struct Drag {
    pub press: Delivery,
    pub motions: Vec<Delivery>,
    pub release: Delivery,
}

/// The live implementation waits for source outcomes and processed commit watermarks.
/// Host implementations exercise the same ordering without a compositor.
pub trait Backend {
    fn resolve(&mut self, target: &Target) -> Result<Resolved, String>;
    fn move_to(&mut self, point: (i32, i32)) -> Result<(), String>;
    fn confirm(&mut self, resolved: &Resolved) -> Result<(), String>;
    fn button(&mut self, pressed: bool, window: u64) -> Result<Delivery, String>;
    fn motion(&mut self, delta: (i32, i32), window: u64) -> Result<Delivery, String>;
    fn wheel(&mut self, dy: i32, window: u64) -> Result<Delivery, String>;
}
pub struct Pointer<B: Backend> {
    backend: B,
}
impl<B: Backend> Pointer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    fn position(&mut self, target: &Target) -> Result<Resolved, String> {
        let resolved = self.backend.resolve(target)?;
        self.backend.move_to(resolved.point)?;
        self.backend.confirm(&resolved)?;
        Ok(resolved)
    }
    pub fn click(&mut self, target: &Target) -> Result<Click, String> {
        let resolved = self.position(target)?;
        // A delivery timeout may follow an accepted press; release on that path too.
        let press = self.backend.button(true, target.window);
        let release = self.backend.button(false, target.window);
        Ok(Click {
            point: resolved.point,
            press: press?,
            release: release?,
        })
    }
    pub fn scroll(&mut self, target: &Target, dy: i32) -> Result<Delivery, String> {
        self.position(target)?;
        self.backend.wheel(dy, target.window)
    }
    /// Steps are relative device-pixel deltas, sent within the existing capture. The hook can coordinate application changes between processed steps.
    pub fn drag(
        &mut self,
        target: &Target,
        steps: &[(i32, i32)],
        mut after: impl FnMut(usize, &mut B) -> Result<(), String>,
    ) -> Result<Drag, String> {
        self.position(target)?;
        let result: Result<(Delivery, Vec<Delivery>), String> = (|| {
            let press = self.backend.button(true, target.window)?;
            after(0, &mut self.backend)?;
            let mut motions = Vec::with_capacity(steps.len());
            for (index, &delta) in steps.iter().enumerate() {
                motions.push(self.backend.motion(delta, target.window)?);
                after(index + 1, &mut self.backend)?;
            }
            Ok((press, motions))
        })();
        let release = self.backend.button(false, target.window);
        let (press, motions) = result?;
        Ok(Drag {
            press,
            motions,
            release: release?,
        })
    }
}

/// Subtract opaque, screen-space window frames from the target's exposed region.
pub fn exposed_region(content: [i64; 4], screen: [i64; 4], covers: &[[i64; 4]]) -> Vec<[i64; 4]> {
    let Some(r) = edges(content).and_then(|r| intersection(r, edges(screen)?)) else {
        return Vec::new();
    };
    let mut rects = vec![r];
    for &cover in covers {
        let Some(cover) = edges(cover) else { continue };
        rects = rects
            .into_iter()
            .flat_map(|r| {
                let Some(cut) = intersection(r, cover) else {
                    return vec![r];
                };
                [
                    [r[0], r[1], r[2], cut[1]],
                    [r[0], cut[3], r[2], r[3]],
                    [r[0], cut[1], cut[0], cut[3]],
                    [cut[2], cut[1], r[2], cut[3]],
                ]
                .into_iter()
                .filter(|p| p[0] < p[2] && p[1] < p[3])
                .collect()
            })
            .collect();
    }
    rects
        .into_iter()
        .map(|r| {
            [
                r[0] as i64,
                r[1] as i64,
                (r[2] - r[0]) as i64,
                (r[3] - r[1]) as i64,
            ]
        })
        .collect()
}
#[cfg(target_os = "breenix")]
#[path = "pointer_live.rs"]
mod live;
#[cfg(target_os = "breenix")]
pub use live::{delivered, dispositions, wait_pointer_idle, LivePointer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: u16,
    pub shift: bool,
}
/// Literal US physical-key table, independent of the compositor translator.
/// Newline and Tab use physical key actions.
pub fn key_stroke(ch: char) -> Option<KeyStroke> {
    let (code, shift) = match ch {
        ' ' => (57, false),
        '!' => (2, true),
        '"' => (40, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '&' => (8, true),
        '\'' => (40, false),
        '(' => (10, true),
        ')' => (11, true),
        '*' => (9, true),
        '+' => (13, true),
        ',' => (51, false),
        '-' => (12, false),
        '.' => (52, false),
        '/' => (53, false),
        '0' => (11, false),
        '1' => (2, false),
        '2' => (3, false),
        '3' => (4, false),
        '4' => (5, false),
        '5' => (6, false),
        '6' => (7, false),
        '7' => (8, false),
        '8' => (9, false),
        '9' => (10, false),
        ':' => (39, true),
        ';' => (39, false),
        '<' => (51, true),
        '=' => (13, false),
        '>' => (52, true),
        '?' => (53, true),
        '@' => (3, true),
        'A' => (30, true),
        'B' => (48, true),
        'C' => (46, true),
        'D' => (32, true),
        'E' => (18, true),
        'F' => (33, true),
        'G' => (34, true),
        'H' => (35, true),
        'I' => (23, true),
        'J' => (36, true),
        'K' => (37, true),
        'L' => (38, true),
        'M' => (50, true),
        'N' => (49, true),
        'O' => (24, true),
        'P' => (25, true),
        'Q' => (16, true),
        'R' => (19, true),
        'S' => (31, true),
        'T' => (20, true),
        'U' => (22, true),
        'V' => (47, true),
        'W' => (17, true),
        'X' => (45, true),
        'Y' => (21, true),
        'Z' => (44, true),
        '[' => (26, false),
        '\\' => (43, false),
        ']' => (27, false),
        '^' => (7, true),
        '_' => (12, true),
        '`' => (41, false),
        'a' => (30, false),
        'b' => (48, false),
        'c' => (46, false),
        'd' => (32, false),
        'e' => (18, false),
        'f' => (33, false),
        'g' => (34, false),
        'h' => (35, false),
        'i' => (23, false),
        'j' => (36, false),
        'k' => (37, false),
        'l' => (38, false),
        'm' => (50, false),
        'n' => (49, false),
        'o' => (24, false),
        'p' => (25, false),
        'q' => (16, false),
        'r' => (19, false),
        's' => (31, false),
        't' => (20, false),
        'u' => (22, false),
        'v' => (47, false),
        'w' => (17, false),
        'x' => (45, false),
        'y' => (21, false),
        'z' => (44, false),
        '{' => (26, true),
        '|' => (43, true),
        '}' => (27, true),
        '~' => (41, true),
        '\n' => (28, false),
        '\t' => (15, false),
        _ => return None,
    };
    Some(KeyStroke { code, shift })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Super,
}
impl Modifier {
    pub(crate) fn code(self) -> u16 {
        match self {
            Self::Shift => 42,
            Self::Ctrl => 29,
            Self::Alt => 56,
            Self::Super => 125,
        }
    }
}
pub trait KeyboardBackend {
    fn key(&mut self, code: u16, pressed: bool, window: u64) -> Result<Delivery, String>;
    fn text(&mut self, text: &str, window: u64) -> Result<Delivery, String>;
    fn focus(&mut self, target: &Target) -> Result<(), String>;
}
pub struct Keyboard<B: KeyboardBackend> {
    backend: B,
}
impl<B: KeyboardBackend> Keyboard<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn into_backend(self) -> B { self.backend }
    /// Release attempted presses in reverse order, including after delivery failure.
    pub fn chord(
        &mut self,
        window: u64,
        modifiers: &[Modifier],
        code: u16,
    ) -> Result<Vec<Delivery>, String> {
        if !(1..=255).contains(&code)
            || modifiers
                .iter()
                .enumerate()
                .any(|(i, m)| m.code() == code || modifiers[..i].contains(m))
        {
            return Err("input chord has invalid or duplicate keys".into());
        }
        let mut held = Vec::new();
        let mut deliveries = Vec::new();
        let result: Result<(), String> = (|| {
            for code in modifiers.iter().map(|m| m.code()).chain([code]) {
                held.push(code);
                deliveries.push(self.backend.key(code, true, window)?);
            }
            Ok(())
        })();
        let mut release_error = None;
        for code in held.into_iter().rev() {
            match self.backend.key(code, false, window) {
                Ok(d) => deliveries.push(d),
                Err(e) => {
                    if release_error.is_none() {
                        release_error = Some(e);
                    }
                }
            }
        }
        result?;
        if let Some(error) = release_error {
            return Err(error);
        }
        Ok(deliveries)
    }
    pub fn type_text(&mut self, window: u64, text: &str) -> Result<Vec<Delivery>, String> {
        let mut deliveries = Vec::new();
        for ch in text.chars() {
            if let Some(stroke) = key_stroke(ch) {
                deliveries.extend(self.chord(
                    window,
                    if stroke.shift {
                        &[Modifier::Shift]
                    } else {
                        &[]
                    },
                    stroke.code,
                )?);
            } else {
                let mut bytes = [0; 4];
                deliveries.push(self.backend.text(ch.encode_utf8(&mut bytes), window)?);
            }
        }
        Ok(deliveries)
    }
    pub fn focus_then_type(
        &mut self,
        target: &Target,
        text: &str,
    ) -> Result<Vec<Delivery>, String> {
        self.backend.focus(target)?;
        self.type_text(target.window, text)
    }
}
/// Derived events sort by their source and derivation index, not their wire bits.
pub fn client_processed(processed: u64, source: u64) -> bool {
    let wanted = libbreenix::input::abi::source_order(source);
    wanted.is_some() && libbreenix::input::abi::source_order(processed) >= wanted
}

/// A read or event dequeue alone cannot satisfy delivery: require a commit too.
pub fn delivery_ready(commit: u64, processed: u64, source: u64) -> bool {
    commit != 0 && client_processed(processed, source)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerEntry<'a> {
    pub serial: u64,
    pub disposition: &'a str,
    pub window: u64,
    pub reason: &'a str,
}
fn parse_ledger(ledger: &str) -> Result<std::collections::BTreeMap<u64, LedgerEntry<'_>>, String> {
    let mut rows = std::collections::BTreeMap::new();
    for line in ledger.lines() {
        let mut fields = line.split_whitespace();
        let invalid = || format!("invalid input ledger row {line:?}");
        let serial = fields.next().and_then(|s| s.parse::<u64>().ok())
            .filter(|&s| s != 0).ok_or_else(invalid)?;
        let disposition = fields.next().ok_or_else(invalid)?;
        if !matches!(disposition, "delivered" | "wm-handled" | "rejected" | "client-rejected" | "lost") {
            return Err(invalid());
        }
        let window = fields.next().and_then(|s|s.parse().ok()).ok_or_else(invalid)?;
        let reason = fields.next().ok_or_else(invalid)?;
        if fields.next().is_some() || rows.insert(serial, LedgerEntry {serial,disposition,window,reason}).is_some() {
            return Err(invalid());
        }
    }
    Ok(rows)
}
pub fn ledger_entry(ledger: &str, serial: u64) -> Result<Option<LedgerEntry<'_>>, String> {
    Ok(parse_ledger(ledger)?.get(&serial).copied())
}

/// Return the first requested source serial absent from a terminal ledger snapshot.
/// Requested serials come from the script, since unrelated devices can interleave.
pub fn first_missing_disposition(ledger: &str, serials: &[u64]) -> Result<Option<u64>, String> {
    let finished = parse_ledger(ledger)?;
    Ok(serials.iter().copied().find(|serial| !finished.contains_key(serial)))
}
