//! The run stage drawn on the GPU (docs/gui/breengel.md section 23; gpu.md section 2.5): the
//! production renderer of the suite runner's stage. Its layout, colours and text are section
//! 23's, which `libgfx-ref`'s CPU stage also draws as the runner's oracle. Pictures (tile
//! thumbnails and spotlight panes) are GPU sources: regions of resources the caller holds,
//! drawn by integer scaling, not pixels from the CPU (ruling B36).
use core::fmt::Write;

use bgl::{pipeline, Instance};

use crate::blit::{self, Source};
use crate::color::Color;
use crate::font;
use crate::shapes;
use crate::surface::{Box64, Format, Rect, Surface};
use alloc::vec::Vec;
use crate::ttf_font::{self, CachedFont, Font};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    Pending,
    Running,
    Pass,
    Fail,
    NotReached,
}

/// A picture: the `width` x `height` region at `(x, y)` of `source`, drawn with each source
/// pixel as an `s` x `s` square, where `s` is the element's logical scale times the display
/// scale. `cover`, when set, is a region of the picture, in picture pixels, drawn in one
/// colour over it (the planted substitution of section 21, point 13).
#[derive(Clone, Copy)]
pub struct Picture<'a> {
    pub source: &'a dyn Source,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub cover: Option<([usize; 4], Color)>,
}

/// A tile's thumbnail: a picture, or a solid block (the planted substitute frame).
#[derive(Clone, Copy)]
pub enum Thumbnail<'a> {
    Picture(Picture<'a>),
    Solid { width: usize, height: usize, colour: Color },
}

#[derive(Clone, Copy)]
pub struct Tile<'a> {
    pub id: &'a str,
    pub stamp: Stamp,
    pub key: u64,
    pub thumbnail: Option<Thumbnail<'a>>,
}

#[derive(Clone, Copy)]
pub struct Milestone<'a> {
    pub id: &'a str,
    pub first: usize,
    pub end: usize,
}

/// The difference pane: a failure artifact, or black, then heat rectangles and the first
/// mismatch's cross, in pane pixels.
#[derive(Clone, Copy)]
pub struct Difference<'a> {
    pub artifact: Option<Picture<'a>>,
    pub heat: &'a [([usize; 4], Color)],
    pub cross: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
pub enum Spotlight<'a> {
    None,
    /// A judged surface without a supplied independent reference: its 400 x 250 pane.
    Actual(Picture<'a>),
    /// The expected, actual and difference panes, each 128 x 128.
    Picture {
        expected: Picture<'a>,
        actual: Picture<'a>,
        difference: Difference<'a>,
        differing: usize,
        bound: usize,
    },
    Timing {
        frame_us: &'a [u32],
        budget_us: u32,
    },
    Measurement {
        name: &'a str,
        number: i64,
        unit: &'a str,
        bound: Option<(i64, i64)>,
    },
    Input {
        points: &'a [InputPoint],
        hit: &'a str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPoint {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

#[derive(Clone, Copy)]
pub struct Finale<'a> {
    pub slowest: &'a str,
    pub slowest_ms: u64,
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
    pub input: Option<(&'a [InputPoint], &'a str)>,
    pub finale: Option<Finale<'a>>,
    /// The boundary notice, decoded by the caller from the observation record.
    pub notice: &'a str,
    /// The boot ribbon's segments, in manifest order: whether each stage was reached.
    pub ribbon: &'a [bool],
}

#[derive(Debug, PartialEq, Eq)]
pub enum StageError {
    Size,
    Font,
    Manifest,
    Device(i64),
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

/// Category headings occupy two slots, followed immediately by their cases. The largest
/// integer scale that fits the entire wall is used, without hiding a pending case.
#[derive(Clone, Copy)]
pub struct Wall {
    pub columns: usize,
    pub scale: usize,
}
impl Wall {
    pub fn for_tiles(tiles: &[Tile<'_>]) -> Result<Self, StageError> {
        if tiles.is_empty() {
            return Err(StageError::Manifest);
        }
        for scale in (1..=4).rev() {
            let wall = Self {
                columns: 800 / (40 * scale),
                scale,
            };
            let slot = wall.slot(tiles, tiles.len() - 1);
            if wall.origin(slot).1 + 24 * scale <= 700 {
                return Ok(wall);
            }
        }
        Err(StageError::Manifest)
    }
    pub fn slot(self, tiles: &[Tile<'_>], index: usize) -> usize {
        let mut slot = 0;
        for (i, t) in tiles[..=index].iter().enumerate() {
            if i == 0 || category(t.id) != category(tiles[i - 1].id) {
                if slot % self.columns == self.columns - 1 {
                    slot += 1;
                }
                slot += 2;
            }
            if i == index {
                return slot;
            }
            slot += 1;
        }
        slot
    }
    pub fn origin(self, slot: usize) -> (usize, usize) {
        (
            16 + 40 * self.scale * (slot % self.columns),
            112 + (if self.scale == 1 { 24 } else { 26 * self.scale }) * (slot / self.columns),
        )
    }
}
fn category(id: &str) -> &str {
    id.split_once('/').map_or(id, |v| v.0)
}

struct Text {
    bytes: [u8; 192],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self { bytes: [0; 192], len: 0 }
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.len == self.bytes.len() {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}
impl Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s.as_bytes());
        Ok(())
    }
}

/// The pinned DejaVu Sans the stage draws its text in above display scale 1, parsed on first
/// use; a holder keeps it, and its glyph atlas, across frames.
pub struct StageFont(Option<CachedFont>);
impl StageFont {
    pub const fn new() -> Self {
        Self(None)
    }
    fn get(&mut self) -> Result<&mut CachedFont, StageError> {
        if self.0.is_none() {
            let font = Font::parse(include_bytes!("../../../fonts/DejaVuSans.ttf")).map_err(|_| StageError::Font)?;
            self.0 = Some(CachedFont::with_default_cache(font));
        }
        self.0.as_mut().ok_or(StageError::Font)
    }
}
impl Default for StageFont {
    fn default() -> Self {
        Self::new()
    }
}

/// The display scale of a stage surface: the logical 1280 x 800 layout at a whole scale.
fn scale_of(fb: &Surface) -> Result<usize, StageError> {
    let n = fb.width() / 1280;
    if n == 0 || n > 8 || fb.width() != 1280 * n || fb.height() != 800 * n {
        return Err(StageError::Size);
    }
    Ok(n)
}

/// The stage's surface. Coordinates, extents, gaps and borders are logical and multiplied by
/// the display scale `n`; text at legacy scale s is 7s logical pixels per em, drawn at 7sn
/// pixels per em in DejaVu Sans, or with the 5x7 font at scale 1. The surface's top left is
/// logical `origin`: the frame's, or the wall's.
struct Canvas<'a> {
    fb: &'a mut Surface,
    n: usize,
    font: Option<&'a mut CachedFont>,
    origin: (usize, usize),
}
impl Canvas<'_> {
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let (n, (x, y)) = (self.n, (x - self.origin.0, y - self.origin.1));
        if w != 0 && h != 0 {
            shapes::fill_rect(self.fb, (x * n) as i32, (y * n) as i32, (w * n) as i32, (h * n) as i32, c);
        }
    }
    /// A one-pixel logical line, drawn n pixels thick. `line` weighs 8192 steps a fragment of
    /// its box, so each of the n x n lines is drawn in bands of rows, each a submission of at
    /// most 2^17 fragments; a clip leaves the pixels of the line it crosses unchanged (the
    /// pipeline starts from the line's first step inside it). Drawn outside a batch.
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        let n = self.n as i32;
        let (ox, oy) = (self.origin.0 as i32, self.origin.1 as i32);
        for dy in 0..n {
            for dx in 0..n {
                let (ax, ay, bx, by) = ((x0 - ox) * n + dx, (y0 - oy) * n + dy, (x1 - ox) * n + dx, (y1 - oy) * n + dy);
                let (left, top) = (ax.min(bx), ay.min(by));
                let (width, height) = (ax.abs_diff(bx) as i32 + 1, ay.abs_diff(by) as i32 + 1);
                let band = ((1i32 << 17) / width).max(1);
                let mut row = 0;
                while row < height {
                    let rows = band.min(height - row);
                    self.fb.push_clip(Rect::new(left, top + row, width, rows));
                    shapes::draw_line(self.fb, ax, ay, bx, by, c);
                    self.fb.pop_clip();
                    row += rows;
                }
            }
        }
    }
    /// Text with its pen at (x, y), clipped at the element's right edge, `width` from x.
    fn text(&mut self, bytes: &[u8], x: usize, y: usize, width: usize, scale: usize, color: Color) {
        let height = self.fb.height() / self.n + self.origin.1;
        self.text_in(bytes, x, y, width, height - y, scale, color);
    }
    /// `text`, also clipped at the element's bottom edge, `height` below y.
    fn text_in(&mut self, bytes: &[u8], x: usize, y: usize, width: usize, height: usize, scale: usize, color: Color) {
        if width == 0 {
            return;
        }
        let (n, (x, y)) = (self.n, (x - self.origin.0, y - self.origin.1));
        let Some(font) = self.font.as_deref_mut() else {
            self.fb.push_clip(Rect::new(x as i32, y as i32, width as i32, (7 * scale) as i32));
            for (i, &ch) in bytes.iter().enumerate() {
                if i * 6 * scale >= width {
                    break;
                }
                font::draw_char(self.fb, ch, x + i * 6 * scale, y, color, scale);
            }
            self.fb.pop_clip();
            return;
        };
        let left = -((self.origin.0 * n) as i32);
        self.fb.push_clip(Rect::new(left, (y * n) as i32, ((x + width) * n) as i32 - left, (height * n) as i32));
        ttf_font::draw_text_bytes(self.fb, font, bytes, (x * n) as i32, (y * n) as i32, (7 * scale * n) as f32, color);
        self.fb.pop_clip();
    }
    /// Picture `p` with its top left at logical (x, y), each picture pixel `scale` logical
    /// pixels square. `copy_scaled` weighs 8192 steps a fragment, so a large picture goes in
    /// bands of rows, each a submission of at most 2^17 fragments (2^30 steps), within the
    /// core's 2^31 work cap.
    fn picture(&mut self, p: &Picture<'_>, x: usize, y: usize, scale: usize) {
        let s = (scale * self.n) as i64;
        let (dx, dy) = (((x - self.origin.0) * self.n) as i64, ((y - self.origin.1) * self.n) as i64);
        let whole = Box64::wide(dx, dy, p.width as i64 * s, p.height as i64 * s);
        let band = ((1i64 << 17) / (p.width as i64 * s * s).max(1)).max(1) as usize;
        let flags = gpu_abi_opaque(p.source);
        let texture = p.source.resource();
        let mut row = 0;
        while row < p.height {
            let rows = band.min(p.height - row);
            let top = dy + row as i64 * s;
            let visible = Box64::wide(dx, top, p.width as i64 * s, rows as i64 * s).intersect(&self.fb.clip_box());
            if !visible.is_empty() {
                crate::work::add(1);
                // The first covered source pixel and where its square starts, as `blit_scaled`.
                let (kx, ky) = ((visible.x0 - dx) / s, (visible.y0 - top) / s);
                let (ox, oy) = (dx + kx * s, top + ky * s);
                let mut params = [0u32; 16];
                for (slot, v) in params.iter_mut().zip([ox, oy, kx + p.x as i64, ky + (p.y + row) as i64, s]) {
                    *slot = v as i32 as u32;
                }
                self.fb.submit(|c| c.draw(pipeline::COPY_SCALED, flags, texture, 0, 0, &[Instance { rect: visible.corners(), params }]));
            }
            row += rows;
        }
        self.fb.mark(whole);
        if let Some((r, c)) = p.cover {
            self.fill(x + r[0] * scale, y + r[1] * scale, r[2] * scale, r[3] * scale, c);
        }
    }
    /// Several small pictures at once, each `(picture, x, y)` drawn as `picture` draws it:
    /// those from one source share draws, each of at most 2^17 fragments. A picture with a
    /// cover, or one the clip cuts, is drawn on its own.
    fn pictures(&mut self, list: &[(Picture<'_>, usize, usize)], scale: usize) {
        let s = (scale * self.n) as i64;
        let clip = self.fb.clip_box();
        let mut pending: Vec<Instance> = Vec::new();
        let (mut source, mut area) = (0u64, 0i64);
        let mut flags = 0;
        for (p, x, y) in list {
            let (dx, dy) = (((x - self.origin.0) * self.n) as i64, ((y - self.origin.1) * self.n) as i64);
            let whole = Box64::wide(dx, dy, p.width as i64 * s, p.height as i64 * s);
            let fragments = (p.width * p.height) as i64 * s * s;
            if p.cover.is_some() || whole.intersect(&clip) != whole || fragments > 1 << 17 {
                self.picture(p, *x, *y, scale);
                continue;
            }
            if p.source.resource() != source || area + fragments > 1 << 17 {
                self.flush_pictures(&mut pending, source, flags);
                (source, area, flags) = (p.source.resource(), 0, gpu_abi_opaque(p.source));
            }
            let mut params = [0u32; 16];
            for (slot, v) in params.iter_mut().zip([dx, dy, p.x as i64, p.y as i64, s]) {
                *slot = v as i32 as u32;
            }
            crate::work::add(1);
            pending.push(Instance { rect: whole.corners(), params });
            area += fragments;
            self.fb.mark(whole);
        }
        self.flush_pictures(&mut pending, source, flags);
    }
    fn flush_pictures(&mut self, pending: &mut Vec<Instance>, texture: u64, flags: u32) {
        if pending.is_empty() {
            return;
        }
        let instances = core::mem::take(pending);
        self.fb.submit(|c| c.draw(pipeline::COPY_SCALED, flags, texture, 0, 0, &instances));
    }
}

/// The stage's pictures are opaque BGRX samples of judged frames.
fn gpu_abi_opaque(src: &dyn Source) -> u32 {
    if src.format().premultiplied {
        0
    } else {
        libbreenix::gpu::abi::draw::SOURCE_OPAQUE
    }
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
    let missing = run.tiles.iter().filter(|t| t.stamp == Stamp::NotReached).count();
    let mut out = Text::new();
    let _ = write!(out, "{passed} passed  {failed} failed  {missing} not reached");
    out
}

/// What a tile was drawn from: its stamp and the stamp's colour (a running tile's pulses with
/// UI time), its key and its thumbnail.
#[derive(Clone, Copy, PartialEq)]
struct TileKey {
    stamp: Stamp,
    colour: Color,
    key: u64,
    thumbnail: ThumbKey,
}
#[derive(Clone, Copy, PartialEq)]
enum ThumbKey {
    None,
    Picture { source: u64, at: [usize; 4], cover: Option<([usize; 4], Color)> },
    Solid { width: usize, height: usize, colour: Color },
}
impl ThumbKey {
    fn of(t: &Option<Thumbnail<'_>>) -> ThumbKey {
        match t {
            None => ThumbKey::None,
            Some(Thumbnail::Picture(p)) => ThumbKey::Picture { source: p.source.resource(), at: [p.x, p.y, p.width, p.height], cover: p.cover },
            Some(Thumbnail::Solid { width, height, colour }) => ThumbKey::Solid { width: *width, height: *height, colour: *colour },
        }
    }
}

/// The wall's logical rectangle, which holds the tiles and the category headings that
/// `Wall::origin` places.
const WALL: (usize, usize, usize, usize) = (16, 112, 800, 588);

/// The stage's renderer, kept across frames: the font and its glyph atlas, and the wall of
/// tiles drawn into a target of its own, where only a tile or a heading whose state changed
/// since the last frame is drawn again. Each frame copies the wall into the frame.
pub struct Stage {
    font: StageFont,
    wall: Option<Surface>,
    /// Tiles, columns, wall scale and display scale the wall was laid out for.
    layout: Option<(usize, usize, usize, usize)>,
    tiles: Vec<Option<TileKey>>,
    /// Per tile that starts a category: its heading's passed and total counts.
    heads: Vec<Option<(usize, usize)>>,
}
impl Stage {
    pub const fn new() -> Stage {
        Stage { font: StageFont::new(), wall: None, layout: None, tiles: Vec::new(), heads: Vec::new() }
    }

    /// Draw the stage at the display scale the surface's size gives. The caller validated
    /// the run against the oracle's rules.
    pub fn draw(&mut self, fb: &mut Surface, run: &Run<'_>) -> Result<(), StageError> {
        let n = scale_of(fb)?;
        if run.tiles.is_empty() || run.selected >= run.tiles.len() || run.milestones.is_empty() {
            return Err(StageError::Manifest);
        }
        let wall = Wall::for_tiles(run.tiles)?;
        let layout = (run.tiles.len(), wall.columns, wall.scale, n);
        let Stage { font, wall: target, layout: laid, tiles, heads } = self;
        if *laid != Some(layout) || target.as_ref().is_none_or(|w| w.error().is_some()) {
            *target = None;
            *laid = None;
            let mut w = Surface::new(WALL.2 * n, WALL.3 * n, Format::BGRX).map_err(|_| StageError::Size)?;
            w.clear(BG);
            *target = Some(w);
            *tiles = alloc::vec![None; run.tiles.len()];
            *heads = alloc::vec![None; run.tiles.len()];
            *laid = Some(layout);
        }
        let mut font = if n == 1 { None } else { Some(font.get()?) };
        let target = target.as_mut().ok_or(StageError::Size)?;
        target.batch(|w| update_wall(&mut Canvas { fb: w, n, font: font.as_deref_mut(), origin: (WALL.0, WALL.1) }, run, wall, tiles, heads));
        if let Some(e) = target.error() {
            *laid = None;
            return Err(StageError::Device(e));
        }
        let mut trails = Vec::new();
        fb.batch(|fb| paint(&mut Canvas { fb, n, font: font.as_deref_mut(), origin: (0, 0) }, run, target, &mut trails));
        let cv = &mut Canvas { fb, n, font: font.as_deref_mut(), origin: (0, 0) };
        for (points, x, y, w, h) in &trails {
            trail(cv, points, *x, *y, *w, *h);
        }
        fb.error().map_or(Ok(()), |e| Err(StageError::Device(e)))
    }
}
impl Default for Stage {
    fn default() -> Self {
        Self::new()
    }
}

/// Draw again each tile and category heading whose state changed since the wall was last
/// drawn: a tile's box is cleared to the background first, as the whole frame's is. The
/// changed elements go in three passes, fills, then thumbnails, then text, since no fill
/// of one element meets another's thumbnail or text: the fills share batches, and the
/// thumbnails from the atlas share draws.
fn update_wall(cv: &mut Canvas<'_>, run: &Run<'_>, wall: Wall, tiles: &mut [Option<TileKey>], heads: &mut [Option<(usize, usize)>]) {
    let scale = wall.scale;
    // The changed headings, (tile, passed, total, x, y), and tiles, (tile, colour, x, y).
    let mut headings = Vec::new();
    let mut changed = Vec::new();
    for (i, tile) in run.tiles.iter().enumerate() {
        let slot = wall.slot(run.tiles, i);
        if i == 0 || category(tile.id) != category(run.tiles[i - 1].id) {
            let end = run.tiles[i..].iter().position(|t| category(t.id) != category(tile.id)).map_or(run.tiles.len(), |n| i + n);
            let passed = run.tiles[i..end].iter().filter(|t| t.stamp == Stamp::Pass).count();
            if heads[i] != Some((passed, end - i)) {
                let (x, y) = wall.origin(slot - 2);
                headings.push((i, passed, end - i, x, y));
                heads[i] = Some((passed, end - i));
            }
        }
        let c = stamp(tile.stamp, run.ui_time_ms);
        let key = TileKey { stamp: tile.stamp, colour: c, key: tile.key, thumbnail: ThumbKey::of(&tile.thumbnail) };
        if tiles[i] != Some(key) {
            let (x, y) = wall.origin(slot);
            changed.push((i, c, x, y));
            tiles[i] = Some(key);
        }
    }
    for &(_, _, _, x, y) in &headings {
        cv.fill(x, y, 78 * scale, 24 * scale, HEADER);
    }
    for &(i, c, x, y) in &changed {
        cv.fill(x, y, 38 * scale, 24 * scale, BG);
        cv.fill(x, y, 38 * scale, 1, c);
        cv.fill(x, y + 24 * scale - 1, 38 * scale, 1, c);
        cv.fill(x, y, 1, 24 * scale, c);
        cv.fill(x + 38 * scale - 1, y, 1, 24 * scale, c);
        cv.fill(x + 2 * scale, y + 2 * scale, 16 * scale, 12 * scale, INSET);
        if let Some(Thumbnail::Solid { width, height, colour }) = &run.tiles[i].thumbnail {
            cv.fill(x + 2 * scale, y + 2 * scale, width * scale, height * scale, *colour);
        }
        if run.tiles[i].stamp != Stamp::Pending {
            cv.fill(x + 1, y + 15 * scale, 38 * scale - 2, 9 * scale - 1, c);
        }
    }
    let pictures: Vec<(Picture<'_>, usize, usize)> = changed
        .iter()
        .filter_map(|&(i, _, x, y)| match &run.tiles[i].thumbnail {
            Some(Thumbnail::Picture(p)) => Some((*p, x + 2 * scale, y + 2 * scale)),
            _ => None,
        })
        .collect();
    cv.pictures(&pictures, scale);
    for &(i, passed, total, x, y) in &headings {
        cv.text(category(run.tiles[i].id).as_bytes(), x + 2, y + 2, 76 * scale, scale, Color::WHITE);
        let mut count = Text::new();
        let _ = write!(count, "{passed}/{total} pass");
        cv.text(count.bytes(), x + 2, y + 14 * scale, 76 * scale, scale, Color::WHITE);
    }
    for &(i, _, x, y) in &changed {
        let mut ordinal = Text::new();
        let _ = write!(ordinal, "{}", i + 1);
        cv.text(ordinal.bytes(), x + 20 * scale, y + 2 * scale, 18 * scale, scale, Color::WHITE);
        let tile = &run.tiles[i];
        if tile.stamp != Stamp::Pending {
            let mut key = Text::new();
            if tile.key > 99999 {
                let _ = write!(key, ">");
            } else {
                let _ = write!(key, "{}", tile.key);
            }
            cv.text(key.bytes(), x + 2 * scale, y + 16 * scale, 34 * scale, scale, Color::WHITE);
        }
    }
}

/// An input trail to draw after the frame's batch: its points and its box.
type Trail = (Vec<InputPoint>, usize, usize, i64, i64);

fn paint(cv: &mut Canvas<'_>, run: &Run<'_>, wall: &Surface, trails: &mut Vec<Trail>) {
    cv.fill(0, 0, 1280, 800, BG);
    cv.fill(0, 0, 1280, 96, HEADER);
    let mut title = Text::new();
    let _ = write!(title, "BREENIX / {}", run.suite);
    cv.text(title.bytes(), 24, 16, 1256, 3, Color::WHITE);
    let mut progress = Text::new();
    let _ = write!(progress, "{}  case {} of {}  elapsed {} ms  fps {}", run.target, run.case_number, run.tiles.len(), run.ui_time_ms, run.fps);
    cv.text(progress.bytes(), 24, 48, 1256, 1, Color::WHITE);
    cv.text(counts(run).bytes(), 24, 68, 1256, 2, Color::WHITE);
    blit::blit(cv.fb, wall, (WALL.0 * cv.n) as i32, (WALL.1 * cv.n) as i32);
    draw_spotlight(cv, run, trails);
    for (i, m) in run.milestones.iter().enumerate() {
        let left = 16 + 1248 * i / run.milestones.len();
        let right = 16 + 1248 * (i + 1) / run.milestones.len();
        let c = if run.tiles[m.first..m.end].iter().all(|t| t.stamp == Stamp::Pass) { PASS } else { OUTLINE };
        cv.fill(left, 728, right - left - 2, 24, c);
        // The label's own box, between the ribbon's top and bottom bands (ruling B25).
        cv.text_in(m.id.as_bytes(), left + 4, 736, right - left - 6, 7, 1, Color::WHITE);
    }
    cv.text(run.notice.as_bytes(), 24, 776, 1256, 1, DIM);
    if let Some(finale) = run.finale {
        cv.fill(48, 240, 720, 240, HEADER);
        cv.text(b"RUN COMPLETE", 72, 264, 696, 3, Color::WHITE);
        cv.text(counts(run).bytes(), 72, 312, 696, 2, Color::WHITE);
        let mut slowest = Text::new();
        let _ = write!(slowest, "slowest {} {} ms", finale.slowest, finale.slowest_ms);
        cv.text(slowest.bytes(), 72, 352, 696, 1, Color::WHITE);
    }
    // One segment per producer-reached stage (section 23.7), in manifest order.
    cv.fill(16, 752, 1248, 16, BG);
    for (i, &lit) in run.ribbon.iter().enumerate() {
        let (left, right) = (16 + 1248 * i / run.ribbon.len(), 16 + 1248 * (i + 1) / run.ribbon.len());
        cv.fill(left, 752, right - left - 2, 16, if lit { PASS } else { OUTLINE });
    }
}

/// Words kept whole on up to five lines of `width / 6s` character cells, the fifth ending in
/// an ellipsis when the text does not fit.
fn wrapped(cv: &mut Canvas<'_>, bytes: &[u8], x: usize, y: usize, width: usize, scale: usize) {
    let count = width / (6 * scale);
    if count < 3 {
        return;
    }
    let mut rest = bytes;
    for line in 0..5 {
        while rest.first().is_some_and(u8::is_ascii_whitespace) {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            break;
        }
        if line == 4 && rest.len() > count {
            let mut end = count - 3;
            if let Some(space) = rest[..end].iter().rposition(u8::is_ascii_whitespace) {
                end = space;
            }
            let mut last = Text::new();
            last.push(&rest[..end]);
            last.push(b"...");
            cv.text(last.bytes(), x, y + line * 9 * scale, width, scale, Color::WHITE);
            break;
        }
        let end = if rest.len() <= count {
            rest.len()
        } else {
            rest[..=count].iter().rposition(u8::is_ascii_whitespace).filter(|n| *n > 0).unwrap_or(count)
        };
        cv.text(&rest[..end], x, y + line * 9 * scale, width, scale, Color::WHITE);
        rest = &rest[end..];
    }
}

/// An input trail: segments between the points and a 5 x 5 marker at each, mapped into the
/// `width` x `height` box at logical (x, y).
fn trail(cv: &mut Canvas<'_>, points: &[InputPoint], x: usize, y: usize, width: i64, height: i64) {
    let min_x = points.iter().map(|p| p.x).min().unwrap_or(0) as i64;
    let min_y = points.iter().map(|p| p.y).min().unwrap_or(0) as i64;
    let dx = (points.iter().map(|p| p.x).max().unwrap_or(1) as i64 - min_x).max(1);
    let dy = (points.iter().map(|p| p.y).max().unwrap_or(1) as i64 - min_y).max(1);
    let mapped = |p: &InputPoint| {
        (
            x + ((p.x as i64 - min_x) * width / dx) as usize,
            y + ((p.y as i64 - min_y) * height / dy) as usize,
        )
    };
    for pair in points.windows(2) {
        let (x0, y0) = mapped(&pair[0]);
        let (x1, y1) = mapped(&pair[1]);
        cv.line(x0 as i32, y0 as i32, x1 as i32, y1 as i32, BAR);
    }
    for p in points {
        let (px, py) = mapped(p);
        cv.fill(px - 2, py - 2, 5, 5, if p.pressed { PASS } else { Color::WHITE });
    }
}

/// The spotlight; its input trails go to `trails`, drawn after the frame's batch, since a
/// trail's lines are drawn in bands of their own submissions. The milestones, notice, finale
/// and ribbon drawn after them lie outside the trail boxes.
fn draw_spotlight(cv: &mut Canvas<'_>, run: &Run<'_>, trails: &mut Vec<Trail>) {
    cv.fill(832, 112, 432, 588, INSET);
    let selected = &run.tiles[run.selected];
    let (group, id) = selected.id.split_once('/').unwrap_or(("", selected.id));
    let mut heading = Text::new();
    let _ = write!(heading, "CASE {} / {}", run.selected + 1, group);
    cv.text(heading.bytes(), 840, 120, 424, 1, Color::WHITE);
    cv.text(id.as_bytes(), 840, 132, 424, 1, Color::WHITE);
    let mut key = Text::new();
    let _ = write!(key, "key {}", selected.key);
    cv.text(key.bytes(), 840, 144, 424, 1, Color::WHITE);
    let verdict: &[u8] = match selected.stamp {
        Stamp::Pass => b"PASS",
        Stamp::Fail => b"FAIL",
        Stamp::NotReached => b"NOT REACHED",
        _ => b"RUNNING",
    };
    cv.text(verdict, 848, 640, 400, 3, stamp(selected.stamp, run.ui_time_ms));
    match run.spotlight {
        Spotlight::None => {
            let words: &[u8] = if selected.stamp == Stamp::NotReached {
                b"Case did not run"
            } else {
                b"Awaiting case result"
            };
            cv.text(words, 848, 360, 400, 2, DIM);
        }
        Spotlight::Measurement { name, number, unit, bound } => {
            wrapped(cv, name.as_bytes(), 848, 208, 400, 2);
            let mut value = Text::new();
            let _ = write!(value, "{number} {unit}");
            wrapped(cv, value.bytes(), 848, 280, 400, 3);
            let mut label = Text::new();
            match bound {
                Some((lo, hi)) if lo == hi => {
                    let _ = write!(label, "Expected exactly {lo} {unit}");
                }
                Some((lo, hi)) => {
                    let _ = write!(label, "Bound: {lo} to {hi} {unit}");
                }
                None => {
                    let _ = write!(label, "No numeric bound supplied");
                }
            }
            wrapped(cv, label.bytes(), 848, 392, 400, 2);
        }
        Spotlight::Input { points, hit } => {
            let heading: &[u8] = if points.is_empty() { b"INPUT / events" } else { b"INPUT / pointer trail" };
            cv.text(heading, 848, 184, 400, 2, Color::WHITE);
            wrapped(cv, hit.as_bytes(), 848, 216, 400, 2);
            trails.push((points.to_vec(), 864, 336, 352, 192));
            if points.is_empty() {
                cv.text(b"Events (no pointer)", 848, 360, 400, 2, Color::WHITE);
            } else {
                cv.text(b"green: pointer down / white: up", 848, 568, 400, 1, DIM);
            }
        }
        Spotlight::Actual(p) => {
            cv.text(b"actual (reference not supplied)", 840, 152, 424, 1, Color::WHITE);
            cv.picture(&p, 848, 168, 1);
        }
        Spotlight::Picture {
            expected,
            actual,
            difference,
            differing,
            bound,
        } => {
            for (i, label) in [b"expected".as_slice(), b"actual", b"difference"].iter().enumerate() {
                cv.text(label, 840 + i * 144, 152, 128, 1, Color::WHITE);
            }
            cv.picture(&expected, 840, 168, 1);
            cv.picture(&actual, 984, 168, 1);
            match &difference.artifact {
                Some(p) => cv.picture(p, 1128, 168, 1),
                None => cv.fill(1128, 168, 128, 128, Color::BLACK),
            }
            for (r, c) in difference.heat {
                cv.fill(1128 + r[0], 168 + r[1], r[2], r[3], *c);
            }
            if let Some((cx, cy)) = difference.cross {
                let (x0, x1) = (cx.saturating_sub(2), (cx + 2).min(127));
                let (y0, y1) = (cy.saturating_sub(2), (cy + 2).min(127));
                cv.fill(1128 + x0, 168 + cy, x1 - x0 + 1, 1, Color::WHITE);
                cv.fill(1128 + cx, 168 + y0, 1, y1 - y0 + 1, Color::WHITE);
            }
            let mut label = Text::new();
            let _ = write!(label, "{differing} pixels differ; bound {bound}");
            cv.text(label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
        Spotlight::Timing { frame_us, budget_us } => {
            let max = 2 * budget_us as u64;
            for (i, &us) in frame_us.iter().enumerate() {
                let (left, right) = (848 + 400 * i / frame_us.len(), 848 + 400 * (i + 1) / frame_us.len());
                let height = (192 * (us as u64).min(max) / max) as usize;
                cv.fill(left, 528 - height, right - left - 1, height, if us > budget_us { FAIL } else { BAR });
            }
            cv.fill(848, 432, 400, 1, Color::WHITE);
            let mut label = Text::new();
            let _ = write!(label, "frame times us; budget {budget_us}");
            cv.text(label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
    }
    if let Some((points, hit)) = run.input {
        let heading: &[u8] = if points.is_empty() {
            b"INPUT / events and handler result"
        } else {
            b"INPUT / pointer trail and handler result"
        };
        cv.text(heading, 848, 432, 400, 1, Color::WHITE);
        wrapped(cv, hit.as_bytes(), 848, 448, 400, 1);
        trails.push((points.to_vec(), 864, 516, 352, 80));
        if points.is_empty() {
            cv.text(b"Events (no pointer)", 848, 536, 400, 2, Color::WHITE);
        }
    }
}
