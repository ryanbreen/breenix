//! Deterministic run showcase, BreenGL contract section 23.
//! Inputs are immutable result snapshots. The caller owns the UI clock and display handoff.
use crate::{color::Color, font, framebuf::FrameBuf, shapes};
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    Pending,
    Running,
    Pass,
    Fail,
    NotReached,
}
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Copy)]
pub struct Tile<'a> {
    pub id: &'a str,
    pub stamp: Stamp,
    pub key: u64,
    pub thumbnail: Option<Image<'a>>,
}
#[derive(Clone, Copy)]
pub struct Milestone<'a> {
    pub id: &'a str,
    pub first: usize,
    pub end: usize,
}
#[derive(Clone, Copy)]
pub enum Spotlight<'a> {
    None,
    /// A judged surface without a supplied independent reference.
    Actual(Image<'a>),
    Picture {
        expected: Image<'a>,
        actual: Image<'a>,
        first: Option<(usize, usize)>,
        differing: usize,
        bound: usize,
    },
    Timing {
        frame_us: &'a [u32],
        budget_us: u32,
    },
}
#[derive(Clone, Copy)]
pub struct Finale<'a> {
    pub started_ms: u64,
    pub slowest: &'a str,
    pub slowest_ms: u64,
}
impl Finale<'_> {
    pub fn hold_complete(&self, ui_time_ms: u64) -> bool {
        ui_time_ms.saturating_sub(self.started_ms) >= 5000
    }
}
pub struct Run<'a> {
    pub suite: &'a str,
    pub target: &'a str,
    pub case_number: usize,
    pub ui_time_ms: u64,
    pub fps: u32,
    pub tiles: &'a [Tile<'a>],
    pub milestones: &'a [Milestone<'a>],
    pub selected: usize,
    pub spotlight: Spotlight<'a>,
    pub finale: Option<Finale<'a>>,
    pub boundary: Observation,
}
/// The observation record fields that constrain the stage's boundary notice.
#[derive(Clone, Copy)]
pub struct Observation { pub backend: u32, pub flags: u32, pub boundary: u32 }
impl Observation {
    pub fn notice(self) -> Result<&'static str, StageError> {
        boundary_notice(self.backend,self.flags,self.boundary)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum StageError {
    Size,
    Manifest,
    Image,
    Timing,
    Boundary,
}

const BG: Color = Color::rgb(12, 19, 31);
const HEADER: Color = Color::rgb(24, 39, 59);
const INSET: Color = Color::rgb(18, 29, 44);
const DIM: Color = Color::rgb(161, 183, 204);
const PASS: Color = Color::rgb(26, 112, 77);
const FAIL: Color = Color::rgb(145, 50, 58);
const GREY: Color = Color::rgb(57, 67, 80);
const OUTLINE: Color = Color::rgb(48, 67, 96);
const BAR: Color = Color::rgb(64, 160, 224);

struct Text {
    bytes: [u8; 192],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 192],
            len: 0,
        }
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl core::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.bytes() {
            if self.len == self.bytes.len() {
                break;
            }
            self.bytes[self.len] = ch;
            self.len += 1;
        }
        Ok(())
    }
}
fn fill(fb: &mut FrameBuf, x: usize, y: usize, w: usize, h: usize, c: Color) {
    if w != 0 && h != 0 {
        shapes::fill_rect(fb, x as i32, y as i32, w as i32, h as i32, c);
    }
}
fn text(
    fb: &mut FrameBuf,
    bytes: &[u8],
    x: usize,
    y: usize,
    width: usize,
    scale: usize,
    color: Color,
) {
    if width == 0 { return; }
    fb.push_clip(crate::framebuf::Rect::new(x as i32,y as i32,width as i32,(7*scale) as i32));
    for (i, &ch) in bytes.iter().enumerate() {
        if i * 6 * scale >= width { break; }
        font::draw_char(fb, ch, x + i * 6 * scale, y, color, scale);
    }
    fb.pop_clip();
}
impl Image<'_> {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= 16384
            && self.height <= 16384
            && self
                .width
                .checked_mul(self.height)
                .and_then(|n| n.checked_mul(4))
                .is_some_and(|n| n == self.pixels.len())
    }
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let o = (y * self.width + x) * 4;
        [self.pixels[o], self.pixels[o + 1], self.pixels[o + 2], 255]
    }
}
fn color(p: [u8; 4]) -> Color {
    Color::rgb(p[2], p[1], p[0])
}
fn stamp(state: Stamp, ui_time_ms: u64) -> Color {
    match state {
        Stamp::Pass => PASS,
        Stamp::Fail => FAIL,
        Stamp::NotReached => GREY,
        Stamp::Pending => OUTLINE,
        Stamp::Running => {
            let q = 500 - (ui_time_ms % 1000).abs_diff(500);
            Color::rgb(64, 128 + (127 * q / 500) as u8, 224)
        }
    }
}
fn counts(run: &Run<'_>) -> Text {
    let passed = run.tiles.iter().filter(|t| t.stamp == Stamp::Pass).count();
    let failed = run.tiles.iter().filter(|t| t.stamp == Stamp::Fail).count();
    let missing = run
        .tiles
        .iter()
        .filter(|t| t.stamp == Stamp::NotReached)
        .count();
    let mut out = Text::new();
    let _ = write!(
        out,
        "{passed} passed  {failed} failed  {missing} not reached"
    );
    out
}
fn validate(fb: &FrameBuf, run: &Run<'_>) -> Result<(), StageError> {
    if fb.width != 1280 || fb.height != 800 {
        return Err(StageError::Size);
    }
    if run.tiles.is_empty()
        || run.selected >= run.tiles.len()
        || run.case_number == 0
        || run.case_number > run.tiles.len()
        || run.milestones.is_empty()
        || run.milestones.len() > 64
        || run.tiles.len() + run.milestones.len() - 1 > 420
    {
        return Err(StageError::Manifest);
    }
    let mut end = 0;
    for m in run.milestones {
        if m.id.is_empty() || m.first != end || m.end <= m.first || m.end > run.tiles.len() {
            return Err(StageError::Manifest);
        }
        end = m.end;
    }
    if end != run.tiles.len() {
        return Err(StageError::Manifest);
    }
    for tile in run.tiles {
        if tile.id.is_empty() {
            return Err(StageError::Manifest);
        }
        if tile.thumbnail.is_some_and(|im| !im.valid()) {
            return Err(StageError::Image);
        }
    }
    match run.spotlight {
        Spotlight::Picture {
            expected,
            actual,
            first,
            differing,
            ..
        } => {
            if !expected.valid()
                || !actual.valid()
                || expected.width != actual.width
                || expected.height != actual.height
                || differing > actual.width * actual.height
                || first.is_some_and(|(x, y)| x >= actual.width || y >= actual.height)
                || (differing == 0) != first.is_none()
            {
                return Err(StageError::Image);
            }
            let mut count = 0;
            let mut first_pixel = None;
            for y in 0..actual.height { for x in 0..actual.width {
                if expected.pixel(x,y) != actual.pixel(x,y) {
                    count += 1;
                    first_pixel.get_or_insert((x,y));
                }
            } }
            if differing != count || first != first_pixel { return Err(StageError::Image); }
        }
        Spotlight::Timing {
            frame_us,
            budget_us,
        } => {
            if frame_us.is_empty() || frame_us.len() > 64 || budget_us == 0 {
                return Err(StageError::Timing);
            }
        }
        Spotlight::Actual(image) => {if !image.valid(){return Err(StageError::Image);}}
        Spotlight::None => {}
    }
    run.boundary.notice()?;
    Ok(())
}
/// Draw between cases only. Validation happens before any pixel changes.
pub fn draw(fb: &mut FrameBuf, run: &Run<'_>) -> Result<(), StageError> {
    validate(fb, run)?;
    fill(fb, 0, 0, 1280, 800, BG);
    fill(fb, 0, 0, 1280, 96, HEADER);
    let mut title = Text::new();
    let _ = write!(title, "BREENIX / {}", run.suite);
    text(fb, title.bytes(), 24, 16, 1256, 3, Color::WHITE);
    let mut progress = Text::new();
    let _ = write!(
        progress,
        "{}  case {} of {}  elapsed {} ms  fps {}",
        run.target,
        run.case_number,
        run.tiles.len(),
        run.ui_time_ms,
        run.fps
    );
    text(fb, progress.bytes(), 24, 48, 1256, 1, Color::WHITE);
    text(fb, counts(run).bytes(), 24, 68, 1256, 2, Color::WHITE);
    for (i, tile) in run.tiles.iter().enumerate() {
        let slot = i + run.milestones.iter().filter(|m| m.end <= i).count();
        let (x, y) = (16 + 40 * (slot % 20), 112 + 28 * (slot / 20));
        let c = stamp(tile.stamp, run.ui_time_ms);
        fill(fb, x, y, 38, 1, c);
        fill(fb, x, y + 25, 38, 1, c);
        fill(fb, x, y + 1, 1, 24, c);
        fill(fb, x + 37, y + 1, 1, 24, c);
        fill(fb, x + 2, y + 2, 16, 12, INSET);
        if let Some(image) = tile.thumbnail {
            thumbnail(fb, image, x + 2, y + 2);
        }
        let mut ordinal = Text::new();
        let _ = write!(ordinal, "{}", i + 1);
        text(fb, ordinal.bytes(), x + 20, y + 2, 18, 1, Color::WHITE);
        if tile.stamp != Stamp::Pending {
            fill(fb, x + 1, y + 15, 36, 10, c);
            let mut key = Text::new();
            if tile.key > 99999 { let _ = write!(key, ">"); } else { let _ = write!(key, "{}", tile.key); }
            text(fb, key.bytes(), x + 2, y + 17, 34, 1, Color::WHITE);
        }
    }
    draw_spotlight(fb, run);
    for (i, m) in run.milestones.iter().enumerate() {
        let left = 16 + 1248 * i / run.milestones.len();
        let right = 16 + 1248 * (i + 1) / run.milestones.len();
        let c = if run.tiles[m.first..m.end]
            .iter()
            .all(|t| t.stamp == Stamp::Pass)
        {
            PASS
        } else {
            OUTLINE
        };
        fill(fb, left, 728, right - left - 2, 24, c);
        text(
            fb,
            m.id.as_bytes(),
            left + 4,
            736,
            right - left - 6,
            1,
            Color::WHITE,
        );
    }
    text(fb, run.boundary.notice()?.as_bytes(), 24, 776, 1256, 1, DIM);
    if let Some(finale) = run.finale {
        fill(fb, 48, 240, 720, 240, HEADER);
        text(fb, b"RUN COMPLETE", 72, 264, 696, 3, Color::WHITE);
        text(fb, counts(run).bytes(), 72, 312, 696, 2, Color::WHITE);
        let mut slowest = Text::new();
        let _ = write!(
            slowest,
            "slowest {} {} ms",
            finale.slowest, finale.slowest_ms
        );
        text(fb, slowest.bytes(), 72, 352, 696, 1, Color::WHITE);
    }
    Ok(())
}
fn thumbnail(fb: &mut FrameBuf, image: Image<'_>, left: usize, top: usize) {
    let (w, h) = (image.width.min(16), image.height.min(12));
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x * image.width / w, (x + 1) * image.width / w);
            let (y0, y1) = (y * image.height / h, (y + 1) * image.height / h);
            let n = ((x1 - x0) * (y1 - y0)) as u64;
            let mut sum = [0u64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = image.pixel(sx, sy);
                    for i in 0..3 {
                        sum[i] += p[i] as u64;
                    }
                }
            }
            let p = [
                ((sum[0] + n / 2) / n) as u8,
                ((sum[1] + n / 2) / n) as u8,
                ((sum[2] + n / 2) / n) as u8,
                255,
            ];
            fb.put_pixel(left + x, top + y, color(p));
        }
    }
}
fn draw_spotlight(fb: &mut FrameBuf, run: &Run<'_>) {
    fill(fb, 832, 112, 432, 588, INSET);
    let selected = &run.tiles[run.selected];
    let (group, id) = selected.id.split_once('/').unwrap_or(("", selected.id));
    let mut heading = Text::new();
    let _ = write!(heading, "CASE {} / {}", run.selected + 1, group);
    text(fb, heading.bytes(), 840, 120, 424, 1, Color::WHITE);
    text(fb, id.as_bytes(), 840, 132, 424, 1, Color::WHITE);
    let mut key = Text::new();
    let _ = write!(key, "key {}", selected.key);
    text(fb, key.bytes(), 840, 144, 424, 1, Color::WHITE);
    match run.spotlight {
        Spotlight::None => {}
        Spotlight::Actual(image) => {
            text(fb,b"actual (reference not supplied)",840,152,424,1,Color::WHITE);
            for y in 0..128 {for x in 0..128 {fb.put_pixel(984+x,168+y,color(image.pixel(x*image.width/128,y*image.height/128)));}}
        }
        Spotlight::Picture {
            expected,
            actual,
            first,
            differing,
            bound,
        } => {
            for (i, label) in [b"expected".as_slice(), b"actual", b"difference"]
                .iter()
                .enumerate()
            {
                text(fb, label, 840 + i * 144, 152, 128, 1, Color::WHITE);
            }
            for y in 0..128 {
                for x in 0..128 {
                    let (sx, sy) = (x * actual.width / 128, y * actual.height / 128);
                    let (a, b) = (expected.pixel(sx, sy), actual.pixel(sx, sy));
                    let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap();
                    fb.put_pixel(840 + x, 168 + y, color(a));
                    fb.put_pixel(984 + x, 168 + y, color(b));
                    fb.put_pixel(
                        1128 + x,
                        168 + y,
                        if d == 0 {
                            Color::BLACK
                        } else {
                            Color::rgb(255, 255 - d, 0)
                        },
                    );
                }
            }
            if let Some((x, y)) = first {
                let (cx, cy) = (x * 128 / actual.width, y * 128 / actual.height);
                for xx in cx.saturating_sub(2)..=(cx + 2).min(127) {
                    fb.put_pixel(1128 + xx, 168 + cy, Color::WHITE);
                }
                for yy in cy.saturating_sub(2)..=(cy + 2).min(127) {
                    fb.put_pixel(1128 + cx, 168 + yy, Color::WHITE);
                }
            }
            let mut label = Text::new();
            let _ = write!(label, "{differing} pixels differ; bound {bound}");
            text(fb, label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
        Spotlight::Timing {
            frame_us,
            budget_us,
        } => {
            let max = 2 * budget_us as u64;
            for (i, &us) in frame_us.iter().enumerate() {
                let (left, right) = (
                    848 + 400 * i / frame_us.len(),
                    848 + 400 * (i + 1) / frame_us.len(),
                );
                let height = (192 * (us as u64).min(max) / max) as usize;
                fill(
                    fb,
                    left,
                    528 - height,
                    right - left - 1,
                    height,
                    if us > budget_us { FAIL } else { BAR },
                );
            }
            fill(fb, 848, 432, 400, 1, Color::WHITE);
            let mut label = Text::new();
            let _ = write!(label, "frame times us; budget {budget_us}");
            text(fb, label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
    }
}
/// Decode precisely the observation strength the record supports.
pub fn boundary_notice(
    backend: u32,
    flags: u32,
    boundary: u32,
) -> Result<&'static str, StageError> {
    match (backend, flags, boundary) {
        (1, 1, 1) => Ok("host copy and window outside guest view"),
        (2, 0, 2) => Ok("host scanning outside guest view"),
        (3, 1, 1) => Ok("picture not host-verified"),
        (4, 0, 3) => Ok("host window outside guest view"),
        _ => Err(StageError::Boundary),
    }
}
/// One segment per producer-reached stage; callers must supply the manifest order.
pub fn draw_boot_ribbon(fb: &mut FrameBuf, reached: &[bool]) -> Result<(), StageError> {
    if fb.width != 1280 || fb.height != 800 {
        return Err(StageError::Size);
    }
    if reached.is_empty() || reached.len() > 416 {
        return Err(StageError::Manifest);
    }
    fill(fb, 16, 752, 1248, 16, BG);
    for (i, &lit) in reached.iter().enumerate() {
        let (left, right) = (
            16 + 1248 * i / reached.len(),
            16 + 1248 * (i + 1) / reached.len(),
        );
        fill(
            fb,
            left,
            752,
            right - left - 2,
            16,
            if lit { PASS } else { OUTLINE },
        );
    }
    Ok(())
}
