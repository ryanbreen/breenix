//! The runner's real between-case stage and its live self-checks (contract §23).
//!
//! The suite binary's painter draws the stage on the GPU (libgfx's `stage`, gpu.md section
//! 2.5); `libgfx-ref`'s CPU stage draws the same frame as the oracle each presented frame is
//! compared against. The stage's pictures are GPU samples of the cases' own frames (ruling
//! B36): tile thumbnails in an atlas and the spotlight's panes. It uploads only a failing
//! case's expected and difference panes, tagged Fixture as failure artifacts, plus B40's
//! separately reported reset recovery (gpu.md section 2.4). A forked case's live checks have the runner draw the stage for them (`remote`), since
//! GPU handles are not inherited across fork.
use super::suite::{check, CaseResult, FrameTiming, JudgedFrame, Outcome, State, Suite};
use crate::{
    gpu::{self, abi::record},
    graphics, process, time,
    window::{self as win, abi},
};
use abi::{op, Plain};
use libgfx_ref::{
    framebuf::{Format, FrameBuf},
    stage::{self, Finale, Image, Milestone, Observation, Spotlight, StageFont, Stamp, Tile},
};
use std::{
    boxed::Box,
    string::String,
    sync::{Arc, Mutex},
    vec::Vec,
};

/// A picture the painter draws: the `width` x `height` region at `(x, y)` of GPU resource
/// `handle` of this process, with `cover`, when set, a region of it in picture pixels drawn
/// over it in one colour, `[r, g, b]`.
#[derive(Clone, Copy, Debug)]
pub struct PictureRef {
    pub handle: u64,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub cover: Option<([usize; 4], [u8; 3])>,
}

/// A tile's thumbnail: a picture, or a solid block (the planted substitute frame).
#[derive(Clone, Copy, Debug)]
pub enum ThumbRef {
    Picture(PictureRef),
    Solid { width: usize, height: usize, rgb: [u8; 3] },
}

/// The difference pane: a failure artifact, or black, then heat rectangles and the first
/// mismatch's cross, in pane pixels.
#[derive(Clone, Debug)]
pub struct DiffRef {
    pub artifact: Option<PictureRef>,
    pub heat: Vec<([usize; 4], [u8; 3])>,
    pub cross: Option<(usize, usize)>,
}

/// The spotlight's panes when the oracle's spotlight is a picture: the 400 x 250 actual pane,
/// or the three 128 x 128 panes.
#[derive(Clone, Debug)]
pub enum SpotRef {
    None,
    Actual(PictureRef),
    Picture { expected: PictureRef, actual: PictureRef, difference: DiffRef },
}

/// One stage frame for the painter: the oracle's run (text, stamps, numbers and spotlight
/// kind), with the pictures as GPU references.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub run: &'a stage::Run<'a>,
    pub thumbs: &'a [Option<ThumbRef>],
    pub spot: &'a SpotRef,
}

/// The stage's GPU renderer, which the suite binary that shows the stage registers.
pub trait Painter: Send {
    /// This process's `width` x `height` canvas target, made on first use and kept.
    fn canvas(&mut self, width: u32, height: u32) -> Result<u64, String>;
    /// The context the painter draws in, which it keeps.
    fn context(&mut self) -> Result<u64, String>;
    /// The bytes its context uploaded from declared tables and crossing geometry, the only
    /// uploads it makes (gpu.md section 2.4).
    fn declared_bytes(&mut self) -> u64;
    /// Draw `frame` into the canvas and wait for its render.
    fn paint(&mut self, frame: &Frame<'_>) -> Result<(), String>;
    /// Wait until the painter's submissions are complete.
    fn settle(&mut self) -> Result<(), String>;
    /// Release the painter's targets, caches and context after the finale is retained.
    fn retire(&mut self) -> Result<(), String>;
}

static PAINTER: Mutex<Option<Box<dyn Painter>>> = Mutex::new(None);

/// Register the stage's painter.
pub fn set_painter(painter: Box<dyn Painter>) {
    *PAINTER.lock().unwrap() = Some(painter);
}

fn painter<R>(f: impl FnOnce(&mut dyn Painter) -> Result<R, String>) -> Result<R, String> {
    let mut guard = PAINTER.lock().unwrap();
    f(guard.as_deref_mut().ok_or("no stage painter is registered")?)
}

use crate::ui_clock as clock;
static SHOW: Mutex<Option<Show>> = Mutex::new(None);
/// The stage's DejaVu Sans and its glyph cache, kept across frames.
static FONT: Mutex<StageFont> = Mutex::new(StageFont::new());
#[derive(Clone)]
struct Picture {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    source_hash: [u8; 32],
    thumb_hash: [u8; 32],
}
impl Picture {
    fn image(&self) -> Image<'_> {
        Image {
            pixels: &self.pixels,
            width: self.width,
            height: self.height,
        }
    }
}
#[derive(Clone)]
struct Entry {
    id: String,
    stamp: Stamp,
    key: u64,
    line: String,
    picture: Option<Picture>,
    /// The case published a GPU source; independent of the runner's current holdings.
    published: bool,
    timing: Option<FrameTiming>,
    input: Option<super::suite::InputTrace>,
    reading: Option<super::suite::Value>,
}
#[derive(Clone)]
struct Show {
    suite: String,
    entries: Vec<Entry>,
    groups: Vec<(String, usize, usize)>,
    current: usize,
    selected: usize,
    last: Option<Arc<JudgedFrame>>,
    first_failure: Option<(usize, Option<Arc<JudgedFrame>>)>,
    observation: Observation,
    started_ms: u64,
    finale: Option<u64>,
    fps: u32,

    /// Each measured case window: the runner's kernel present count at its start and end,
    /// and how many of those presents the case's live checks asked the runner for.
    windows: Vec<(u64, u64, u64)>,
    /// The open window's start: the runner's present count and `requested`.
    measured: Option<(u64, u64)>,
    /// The runner's presents its live-check requests made (`remote`), counted from the
    /// kernel's present records around each one.
    requested: u64,
    pace_ms: u64,
    /// The runner's process: pictures there are only GPU samples of the cases' frames.
    runner: u64,
}
fn fault_bits() -> u32 {
    std::fs::read_to_string("/proc/breenix/stage-control")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
/// What the stage reads from one scanout observation: core record version 2 (contract
/// section 20.1).
#[derive(Clone, Copy)]
struct Seen {
    backend: u32,
    flags: u32,
    boundary: u32,
    width: usize,
    height: usize,
    present_fence: u64,
}
fn word(rec: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([rec[at], rec[at + 1], rec[at + 2], rec[at + 3]])
}
fn quad(rec: &[u8], at: usize) -> u64 {
    u64::from(word(rec, at)) | u64::from(word(rec, at + 4)) << 32
}
/// The display's current mode, which sizes the stage's screen buffers (ruling B8).
fn mode() -> Result<gpu::abi::Mode, String> {
    gpu::current_mode().map_err(|r| std::format!("stage CURRENT_MODE: errno {}", -r))
}
/// Observe the whole scanout into `pixels` by the 48-byte SCANOUT_READBACK.
fn observe(pixels: &mut Vec<u8>) -> Result<Seen, String> {
    let m = mode()?;
    pixels.resize(m.width as usize * m.height as usize * 4, 0);
    let mut rec = [0u8; record::SIZE];
    let mut rb = abi::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = rec.as_mut_ptr() as u64;
    rb.record_len = rec.len() as u32;
    if let Err(e) = win::call(op::SCANOUT_READBACK, &mut rb) {
        return Err(std::format!("stage readback: {e}, reason {}", rb.reason));
    }
    let (width, height) = (word(&rec, record::WIDTH) as usize, word(&rec, record::HEIGHT) as usize);
    if word(&rec, record::VERSION_OFF) != record::VERSION
        || word(&rec, record::SIZE_OFF) as usize != record::SIZE
        || quad(&rec, record::TARGET) != 0
        || word(&rec, record::FLAGS) & 1 == 0
        || word(&rec, record::UNWRITTEN) != 0
        || (width, height) != (m.width as usize, m.height as usize)
    {
        return Err(String::from("stage readback: not a version 2 record of the whole scanout"));
    }
    Ok(Seen {
        backend: word(&rec, record::BACKEND),
        flags: word(&rec, record::FLAGS),
        boundary: word(&rec, record::BOUNDARY),
        width,
        height,
        present_fence: quad(&rec, record::PRESENT_FENCE),
    })
}
fn snapshot() -> Result<(Vec<u8>, Seen), String> {
    let mut pixels = Vec::new();
    let seen = observe(&mut pixels)?;
    Ok((pixels, seen))
}
/// The stage check: the scanout is bound by the stage's own PRESENT (its context fence),
/// and each physical pixel is the stage frame's pixel, rendered at the display's physical
/// size, compared on B, G and R (X normalized, ruling B11), or the panel cursor's where the
/// stage drew it (`cursor`, its pointer).
fn shown(fb: &FrameBuf, present: Option<u64>, pixels: &[u8], seen: &Seen, cursor: Option<(i32, i32)>) -> Result<(), String> {
    let fence = present.ok_or("the stage frame was not presented by a GPU PRESENT")?;
    if seen.present_fence != fence {
        return Err(std::format!(
            "the scanout shows present fence {:#x}, not the stage's {fence:#x}",
            seen.present_fence
        ));
    }
    if (seen.width, seen.height) != (fb.width, fb.height) {
        return Err(std::format!(
            "the {}x{} scanout is not the stage's {}x{} frame",
            seen.width, seen.height, fb.width, fb.height
        ));
    }
    let (outline, fill) = cursor.map_or((Vec::new(), Vec::new()), |(x, y)| crate::compositor::sprite(abi::cursor_shape::ARROW, x, y));
    let inside = |rects: &[[i32; 4]], x: usize, y: usize| {
        let (x, y) = (x as i32, y as i32);
        rects.iter().any(|r| (r[0]..r[2]).contains(&x) && (r[1]..r[3]).contains(&y))
    };
    for y in 0..fb.height {
        for x in 0..fb.width {
            let p = fb.get_pixel(x, y);
            let want = if inside(&fill, x, y) {
                [255, 255, 255]
            } else if inside(&outline, x, y) {
                [0, 0, 0]
            } else {
                [p.b, p.g, p.r]
            };
            let o = (y * seen.width + x) * 4;
            if pixels[o..o + 3] != want {
                return Err(std::format!("the scanout differs from the stage frame at physical ({x}, {y})"));
            }
        }
    }
    Ok(())
}
/// The thumbnail atlas: one 16 x 12 cell per tile, 32 cells to a row, room for 1,024 tiles.
const ATLAS: (u32, u32) = (512, 384);
fn cell(tile: usize) -> (usize, usize) {
    ((tile % 32) * 16, (tile / 32) * 12)
}
/// The actual-only pane and the three picture panes, in logical pixels (section 23.4).
const WIDE: (usize, usize) = (400, 250);
const SQUARE: usize = 128;
/// The hash of pictures sampled before the case's judged frame was collected.
const PENDING: [u8; 32] = [0xFF; 32];

/// One judged frame's spotlight panes, made from its GPU frame, and, for a failing case, its
/// expected and difference panes as failure artifacts.
struct Panes {
    hash: [u8; 32],
    wide: u64,
    square: u64,
    artifacts: Option<(u64, u64)>,
    /// Sampled targets, kept for the next frame's panes when these are let go.
    sampled: bool,
    /// The verified judged frame a case published these panes from, once it is bound: the
    /// runner draws its panes again from it when its objects are made again.
    frame: Option<Arc<JudgedFrame>>,
}
impl Panes {
    fn destroy(&self) {
        let (e, d) = self.artifacts.unwrap_or((0, 0));
        for handle in [self.wide, self.square, e, d] {
            if handle != 0 {
                let _ = gpu::resource_destroy(handle);
            }
        }
    }
}

/// A tile's atlas cell: the SHA-256 of the judged frame its thumbnail is of, and the verified
/// thumbnail's pixels, `width` x `height` BGRX, which the runner draws into a new atlas when
/// its objects are made again (empty while the hash is `PENDING`).
#[derive(Clone)]
struct Cell {
    hash: [u8; 32],
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

/// The pictures a case published, kept across the stage's objects being made again: each
/// tile's cell and each bound frame whose panes are held, with whether a failing case's
/// artifacts went with them.
struct Kept {
    cells: Vec<Option<Cell>>,
    frames: Vec<(Arc<JudgedFrame>, bool)>,
}
/// Pictures whose objects were let go and are not drawn again yet.
static KEPT: Mutex<Option<(Kept, recovery::ResetRecovery)>> = Mutex::new(None);

/// What the stage contexts let go during the run uploaded, for the finale's accounting:
/// Image bytes, Fixture bytes, failure artifacts, B40 recovery and reset counts, and
/// how many had counters that could not be read.
#[derive(Clone, Copy)]
struct Retired {
    image: u64,
    fixture: u64,
    artifacts: u64,
    recovery: u64,
    resets: u64,
    unread: u32,
}
static RETIRED: Mutex<Retired> = Mutex::new(Retired { image: 0, fixture: 0, artifacts: 0, recovery: 0, resets: 0, unread: 0 });

/// Horizontal runs of `pick(x, y)`, an ARGB colour, over the `w` x `h` pixels at `at` in
/// `target`, as `(target, colour, rect)`.
fn runs(out: &mut Vec<(u64, u32, [i32; 4])>, target: u64, at: (usize, usize), w: usize, h: usize, pick: impl Fn(usize, usize) -> u32) {
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let colour = pick(x, y);
            let mut end = x + 1;
            while end < w && pick(end, y) == colour {
                end += 1;
            }
            let (x0, y0) = ((at.0 + x) as i32, (at.1 + y) as i32);
            out.push((target, colour, [x0, y0, (at.0 + end) as i32, y0 + 1]));
            x = end;
        }
    }
}

/// The run stage's GPU objects, kept for the run in the runner's process: its context, the
/// painter's canvas (the frame drawn at the display's physical size, and a sampling
/// composite's scene target), the two scanout targets it presents, the thumbnail atlas and
/// the panes. The context is apart from the painter's, so its uploads are the failure
/// artifacts alone.
struct Gpu {
    pid: u64,
    context: u64,
    canvas: u64,
    scanout: [u64; 2],
    /// The scanout target the stage presented last.
    shown: usize,
    width: u32,
    height: u32,
    seen: Vec<u8>,
    atlas: u64,
    /// Per tile, the judged frame whose thumbnail its atlas cell holds.
    cells: Vec<Option<Cell>>,
    /// A stage frame failed: the scanout targets are made again before the next one.
    stale: bool,
    panes: Vec<Panes>,
    /// Pane targets let go, made again into the next sample's panes rather than destroyed
    /// and created for each case.
    spare: Vec<(u64, u64)>,
    /// The Fixture bytes the stage's context uploaded as failure artifacts.
    artifact_bytes: u64,
    recovery_bytes: u64,
    /// The panel cursor, which the stage's compositor draws (ruling B35): where the pointer
    /// was when these objects were made, where the cursor is drawn once the pointer has moved
    /// from there, and whether test point 10 stopped it restoring what it covered, until the
    /// next stage frame.
    pointer_start: Option<(u32, u32)>,
    cursor: Option<(i32, i32)>,
    no_restore: bool,
}
static GPU: Mutex<Option<Gpu>> = Mutex::new(None);
/// The case running now leaves the display with the stage, whose panel cursor follows the
/// pointer while it runs.
static FOLLOW: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The panel cursor's draws for its pointer at `at`, inside a `width` by `height` target: the
/// compositor's arrow sprite, outline then fill.
fn encode_cursor(stream: &mut Vec<u32>, at: (i32, i32), width: u32, height: u32) {
    use gpu::abi::{pipeline, stream::encode};
    let (outline, fill) = crate::compositor::sprite(abi::cursor_shape::ARROW, at.0, at.1);
    for (rects, colour) in [(outline, 0xFF00_0000), (fill, 0xFFFF_FFFF)] {
        let rects: Vec<[i32; 4]> = rects
            .iter()
            .map(|r| [r[0].max(0), r[1].max(0), r[2].min(width as i32), r[3].min(height as i32)])
            .filter(|r| r[0] < r[2] && r[1] < r[3])
            .collect();
        if !rects.is_empty() {
            encode::draw(stream, pipeline::SOLID, colour, &rects);
        }
    }
}

/// One instance of `nearest_scaled` or `box_filter` from the `src` rectangle of `source` to
/// the `dst` rectangle, each `[x, y, width, height]`.
fn sampled(stream: &mut Vec<u32>, pipe: u32, source: u64, dst: [usize; 4], src: [usize; 4]) {
    use gpu::abi::draw;
    let mut params = [0u32; 16];
    for (slot, v) in params.iter_mut().zip(dst.iter().chain(&src)) {
        *slot = *v as u32;
    }
    let rect = [dst[0] as i32, dst[1] as i32, (dst[0] + dst[2]) as i32, (dst[1] + dst[3]) as i32];
    draw::encode(stream, pipe, 0, source, 0, 0, &[draw::Instance { rect, params }]);
}

/// The window op `op` on `value`: the kernel's raw return.
fn raw<T: Plain>(op: u32, value: &mut T) -> i64 {
    // SAFETY: `value` is a live plain structure of its own size.
    unsafe { win::raw_call(op, value as *mut T as *mut u8, core::mem::size_of::<T>()) }
}

impl Gpu {
    /// Create the stage's objects for the current mode, which must be the stage's logical
    /// layout at a whole display scale, and wait, bounded, for the context's warm-up.
    fn new(pid: u64) -> Result<Gpu, String> {
        let m = stage_mode()?;
        let canvas = painter(|p| p.canvas(m.width, m.height))?;
        let context = gpu::context_create().map_err(|e| std::format!("stage context: errno {}", -e))?;
        let mut g = Gpu {
            pid,
            context,
            canvas,
            scanout: [0; 2],
            shown: 0,
            width: m.width,
            height: m.height,
            seen: Vec::new(),
            atlas: 0,
            cells: Vec::new(),
            stale: false,
            panes: Vec::new(),
            spare: Vec::new(),
            artifact_bytes: 0,
            recovery_bytes: 0,
            pointer_start: graphics::mouse_pos().ok(),
            cursor: None,
            no_restore: false,
        };
        if let Err(e) = g.make_scanouts(m.width, m.height) {
            g.destroy();
            return Err(e);
        }
        match gpu::target_create(gpu::abi::format::B8G8R8A8, ATLAS.0, ATLAS.1) {
            Ok(t) => g.atlas = t,
            Err(e) => {
                g.destroy();
                return Err(std::format!("stage thumbnail atlas: errno {}", -e));
            }
        }
        // Written now, so the memory record's first sample already counts its pages.
        g.seen.resize(m.width as usize * m.height as usize * 4, 0);
        // The device warms one context at a time, lowest handle first: a stage context
        // still warming would hold the next case's contexts behind it inside its limit.
        let start = time::now_monotonic().map_or(0, |t| t.as_nanos());
        while gpu::counters(g.context).1.state == gpu::abi::context_state::WARMING
            && time::now_monotonic().map_or(0, |t| t.as_nanos()).saturating_sub(start) < 10_000_000_000
        {
            let _ = time::sleep_ms(5);
        }
        Ok(g)
    }

    fn destroy(&self) {
        for p in &self.panes {
            p.destroy();
        }
        for &(wide, square) in &self.spare {
            let _ = gpu::resource_destroy(wide);
            let _ = gpu::resource_destroy(square);
        }
        for handle in [self.scanout[0], self.scanout[1], self.atlas] {
            if handle != 0 {
                let _ = gpu::resource_destroy(handle);
            }
        }
        let _ = gpu::context_destroy(self.context);
    }

    /// Make the two `width` x `height` scanout targets, in place of any held.
    fn make_scanouts(&mut self, width: u32, height: u32) -> Result<(), String> {
        for slot in 0..2 {
            if self.scanout[slot] != 0 {
                let _ = gpu::resource_destroy(core::mem::take(&mut self.scanout[slot]));
            }
        }
        for slot in 0..2 {
            self.scanout[slot] = gpu::scanout_target_create(gpu::abi::format::B8G8R8A8, width, height)
                .map_err(|e| std::format!("stage scanout target: errno {}", -e))?;
        }
        Ok(())
    }

    /// After a mode change or a failed frame, while the context is live: make the scanout
    /// targets again at the current mode and take the painter's canvas for it, keeping the
    /// atlas and the panes.
    fn redisplay(&mut self) -> Result<(), String> {
        let m = stage_mode()?;
        self.make_scanouts(m.width, m.height)?;
        self.canvas = painter(|p| p.canvas(m.width, m.height))?;
        (self.width, self.height, self.shown, self.cursor, self.stale) = (m.width, m.height, 0, None, false);
        Ok(())
    }

    /// The pictures these objects hold for the cases that published them.
    fn keep_pictures(&self) -> Kept {
        Kept {
            cells: self.cells.iter().map(|c| c.clone().filter(|c| c.hash != PENDING)).collect(),
            frames: self.panes.iter().filter_map(|p| p.frame.clone().map(|f| (f, p.artifacts.is_some()))).collect(),
        }
    }

    /// B40: only a confirmed device reset permits reconstruction from CPU pixels.
    /// Uploads remain Fixture at the wire boundary, but are accounted as recovery,
    /// separately from a passing picture source and from failure artifacts.
    fn restore_pictures(&mut self, kept: &Kept, _reset: &recovery::ResetRecovery) -> Result<(), String> {
        use gpu::abi::{pipeline, stream::encode};
        let mut stream = Vec::new();
        for (tile, c) in kept.cells.iter().enumerate() {
            if let Some(c) = c {
                let texture = self.upload_fixture(c.width, c.height, &c.pixels)?;
                self.recovery_bytes += c.pixels.len() as u64;
                encode::begin(&mut stream, self.atlas);
                let (x, y) = cell(tile);
                sampled(&mut stream, pipeline::NEAREST_SCALED, texture, [x, y, c.width, c.height], [0, 0, c.width, c.height]);
                let copied = self.run(&stream, "recovered thumbnail");
                let _ = gpu::resource_destroy(texture);
                copied?;
                stream.clear();
            }
        }
        self.cells = kept.cells.clone();
        for (f, failing) in &kept.frames {
            let nearest = |w: usize, h: usize| -> Vec<u8> {
                let mut p = Vec::with_capacity(w * h * 4);
                for y in 0..h { for x in 0..w {
                    let i = ((y * f.height / h) * f.width + x * f.width / w) * 4;
                    p.extend_from_slice(&f.pixels[i..i + 4]);
                }}
                p
            };
            let wide_pixels = nearest(WIDE.0, WIDE.1);
            let wide = self.upload_fixture(WIDE.0, WIDE.1, &wide_pixels)?;
            self.recovery_bytes += wide_pixels.len() as u64;
            let square_pixels = nearest(SQUARE, SQUARE);
            let square = match self.upload_fixture(SQUARE, SQUARE, &square_pixels) {
                Ok(t) => t,
                Err(e) => { let _ = gpu::resource_destroy(wide); return Err(e); }
            };
            self.recovery_bytes += square_pixels.len() as u64;
            let mut panes = Panes { hash: f.sha256, wide, square, artifacts: None, sampled: false, frame: Some(f.clone()) };
            if let Some(e) = f.expected.as_ref().filter(|_| *failing) {
                let (e, d) = artifact_panes(f, e);
                let artifacts = (|| {
                    let et = self.upload_fixture(SQUARE, SQUARE, &e)?;
                    self.recovery_bytes += e.len() as u64;
                    match self.upload_fixture(SQUARE, SQUARE, &d) {
                        Ok(dt) => { self.recovery_bytes += d.len() as u64; Ok((et, dt)) }
                        Err(error) => { let _ = gpu::resource_destroy(et); Err(error) }
                    }
                })();
                match artifacts {
                    Ok(a) => panes.artifacts = Some(a),
                    Err(e) => { panes.destroy(); return Err(e); }
                }
            }
            self.keep(panes);
        }
        Ok(())
    }

    /// Submit `stream` in the stage's context and wait for its render.
    fn run(&self, stream: &[u32], what: &str) -> Result<(), String> {
        let (r, submitted) = gpu::submit(self.context, stream);
        if r != 0 {
            return Err(std::format!("{what} submit: errno {}, code {} at {}", -r, submitted.error, submitted.error_index));
        }
        let (r, waited) = gpu::wait(self.context, submitted.fence, 2_000_000_000);
        if r != 0 {
            return Err(std::format!("{what} render: errno {}, status {}", -r, waited.status));
        }
        Ok(())
    }

    /// Give the pictures sampled for tile `tile` before its frame was collected that frame,
    /// with its verified thumbnail, or drop them when the case handed over no frame.
    fn bind(&mut self, tile: usize, judged: Option<(&Picture, &Arc<JudgedFrame>)>) {
        if self.cell_hash(tile) == Some(PENDING) {
            self.set_cell(tile, judged.map(|(p, _)| Cell { hash: p.source_hash, pixels: p.pixels.clone(), width: p.width, height: p.height }));
        }
        if let Some(i) = self.panes.iter().position(|p| p.hash == PENDING) {
            match judged {
                Some((_, f)) => {
                    self.panes[i].hash = f.sha256;
                    self.panes[i].frame = Some(f.clone());
                }
                None => {
                    let p = self.panes.swap_remove(i);
                    self.retire(p);
                }
            }
        }
    }

    /// The panes of the judged frame with SHA-256 `hash`, when this process holds them.
    fn panes_for(&self, hash: &[u8; 32]) -> Option<&Panes> {
        self.panes.iter().find(|p| &p.hash == hash)
    }

    /// Let `p` go: its failure artifacts are destroyed, its sampled targets kept for reuse.
    fn retire(&mut self, p: Panes) {
        if !p.sampled {
            return p.destroy();
        }
        if let Some((e, d)) = p.artifacts {
            let _ = gpu::resource_destroy(e);
            let _ = gpu::resource_destroy(d);
        }
        self.spare.push((p.wide, p.square));
    }

    /// Keep `panes`, replacing any held for the same frame.
    fn keep(&mut self, panes: Panes) {
        if let Some(i) = self.panes.iter().position(|p| p.hash == panes.hash) {
            let old = self.panes.swap_remove(i);
            self.retire(old);
        }
        self.panes.push(panes);
    }

    /// Keep only the panes of the frames with hashes in `wanted`.
    fn prune(&mut self, wanted: &[[u8; 32]]) {
        let (keep, drop): (Vec<_>, Vec<_>) = core::mem::take(&mut self.panes).into_iter().partition(|p| wanted.contains(&p.hash));
        self.panes = keep;
        for p in drop {
            self.retire(p);
        }
    }

    fn set_cell(&mut self, tile: usize, cell: Option<Cell>) {
        if self.cells.len() <= tile {
            self.cells.resize(tile + 1, None);
        }
        self.cells[tile] = cell;
    }

    fn cell_hash(&self, tile: usize) -> Option<[u8; 32]> {
        self.cells.get(tile).and_then(|c| c.as_ref()).map(|c| c.hash)
    }

    /// Sample the `w` x `h` frame at `at` in `source`, a resource of this process, into tile
    /// `tile`'s atlas cell (the integer box filter, section 23.3) and into new panes (the
    /// nearest-scaled 400 x 250 and 128 x 128 samples, section 23.4), recorded as the
    /// pictures of the frame with SHA-256 `hash`. The box filter goes in a submission of its
    /// own: a whole 2560 x 1600 frame takes most of one submission's work cap.
    fn sample(&mut self, source: u64, at: (usize, usize), w: usize, h: usize, tile: usize, hash: [u8; 32]) -> Result<(), String> {
        use gpu::abi::{pipeline, stream::encode};
        if tile >= 1024 {
            return Err(String::from("the thumbnail atlas holds 1,024 tiles"));
        }
        let (cx, cy) = cell(tile);
        let mut stream = Vec::new();
        encode::begin(&mut stream, self.atlas);
        sampled(&mut stream, pipeline::BOX_FILTER, source, [cx, cy, w.min(16), h.min(12)], [at.0, at.1, w, h]);
        self.set_cell(tile, None);
        self.run(&stream, "stage thumbnail")?;
        let (wide, square) = match self.spare.pop() {
            Some(targets) => targets,
            None => {
                let format = gpu::abi::format::B8G8R8A8;
                let wide = gpu::target_create(format, WIDE.0 as u32, WIDE.1 as u32).map_err(|e| std::format!("stage pane: errno {}", -e))?;
                match gpu::target_create(format, SQUARE as u32, SQUARE as u32) {
                    Ok(square) => (wide, square),
                    Err(e) => {
                        let _ = gpu::resource_destroy(wide);
                        return Err(std::format!("stage pane: errno {}", -e));
                    }
                }
            }
        };
        let panes = Panes { hash, wide, square, artifacts: None, sampled: true, frame: None };
        let mut stream = Vec::new();
        encode::begin(&mut stream, panes.wide);
        sampled(&mut stream, pipeline::NEAREST_SCALED, source, [0, 0, WIDE.0, WIDE.1], [at.0, at.1, w, h]);
        encode::begin(&mut stream, panes.square);
        sampled(&mut stream, pipeline::NEAREST_SCALED, source, [0, 0, SQUARE, SQUARE], [at.0, at.1, w, h]);
        if let Err(e) = self.run(&stream, "stage panes") {
            panes.destroy();
            return Err(e);
        }
        self.set_cell(tile, Some(Cell { hash, pixels: Vec::new(), width: 0, height: 0 }));
        self.keep(panes);
        Ok(())
    }

    /// Sample the stage's own frame as it is shown now: a case judged the stage's scanout.
    fn sample_stage(&mut self, tile: usize) -> Result<(), String> {
        let (source, w, h) = (self.scanout[self.shown], self.width as usize, self.height as usize);
        self.sample(source, (0, 0), w, h, tile, PENDING)
    }

    /// Sample the `w` x `h` window `id` a case published with its judged frame: as display
    /// owner and compositor claimant, composite it into the canvas with a sample-only alias
    /// (syscall 414 op 19, the only route to another process's pixels), then sample the
    /// canvas.
    fn sample_window(&mut self, id: u64, w: usize, h: usize, tile: usize) -> Result<(), String> {
        if w > self.width as usize || h > self.height as usize {
            return Err(std::format!("the {w}x{h} judged frame does not fit the {}x{} canvas", self.width, self.height));
        }
        graphics::take_over_display().map_err(|e| std::format!("stage display ownership: {e:?}"))?;
        win::claim_compositor().map_err(|e| std::format!("stage compositor claim: {e:?}"))?;
        let composed = self.compose_window(id, w, h);
        let _ = win::release_compositor();
        composed?;
        self.sample(self.canvas, (0, 0), w, h, tile, PENDING)
    }

    /// One composite whose scene is the shown stage frame with window `id`'s published front
    /// copied over its top left, presented on the other scanout target. A stale or busy
    /// snapshot is retried, within 2 s.
    fn compose_window(&mut self, id: u64, w: usize, h: usize) -> Result<(), String> {
        use gpu::abi::stream::encode;
        const EAGAIN: i64 = -(abi::errno::EAGAIN as i64);
        const EBUSY: i64 = -(abi::errno::EBUSY as i64);
        const EINVAL: i64 = -(abi::errno::EINVAL as i64);
        win::set_geometry(id, 0, 0, 0, abi::geometry_flags::VISIBLE).map_err(|e| std::format!("judged window geometry: {e:?}"))?;
        let deadline = time::now_monotonic().map_or(0, |t| t.as_nanos()) + 2_000_000_000;
        let early = || time::now_monotonic().map_or(0, |t| t.as_nanos()) < deadline;
        loop {
            let mut q = abi::SceneSnapshot::ZERO;
            q.context = self.context;
            let mut r = raw(op::COMPOSITE_SNAPSHOT, &mut q);
            let mut windows = std::vec![abi::SceneWindow::ZERO; (q.window_count as usize).max(1)];
            let mut damage = std::vec![abi::Rect::ZERO; q.damage_count as usize];
            let mut s = abi::SceneSnapshot::ZERO;
            if r == 0 {
                s.context = self.context;
                s.windows = windows.as_mut_ptr() as u64;
                s.window_capacity = windows.len() as u32;
                s.damage = damage.as_mut_ptr() as u64;
                s.damage_capacity = damage.len() as u32;
                r = raw(op::COMPOSITE_SNAPSHOT, &mut s);
            }
            if matches!(r, EAGAIN | EBUSY | EINVAL) && early() {
                let _ = time::sleep_ms(1);
                continue;
            }
            if r != 0 {
                return Err(std::format!("judged window snapshot: errno {}", -r));
            }
            let sample = windows[..s.window_count as usize].iter().find(|x| x.id == id).map_or(0, |x| x.sample);
            if sample == 0 {
                // Not published yet: give the reservation back and look again.
                let mut cancel = abi::Composite::ZERO;
                cancel.snapshot = s.snapshot;
                cancel.flags = 1;
                let _ = raw(op::COMPOSITE, &mut cancel);
                if !early() {
                    return Err(std::format!("judged window {id} published no frame within 2 s"));
                }
                let _ = time::sleep_ms(1);
                continue;
            }
            let (width, height) = (self.width, self.height);
            let other = self.shown ^ 1;
            let mut words = Vec::new();
            encode::begin(&mut words, self.canvas);
            encode::copy(&mut words, self.scanout[self.shown], [0, 0, width, height], self.canvas, [0, 0]);
            encode::copy(&mut words, sample, [0, 0, w as u32, h as u32], self.canvas, [0, 0]);
            encode::begin(&mut words, self.scanout[other]);
            encode::copy(&mut words, self.canvas, [0, 0, width, height], self.scanout[other], [0, 0]);
            let mut c = abi::Composite::ZERO;
            c.snapshot = s.snapshot;
            c.target = self.scanout[other];
            c.stream = words.as_ptr() as u64;
            c.length = (words.len() * 4) as u64;
            c.scene_target = self.canvas;
            match raw(op::COMPOSITE, &mut c) {
                0 => {
                    self.shown = other;
                    return Ok(());
                }
                EAGAIN if early() => {
                    let _ = time::sleep_ms(1);
                }
                r => return Err(std::format!("judged window composite: errno {}, code {} at {}, reason {}", -r, c.error, c.error_index, c.reason)),
            }
        }
    }

    /// Upload `pixels` (`w` x `h`, canonical BGRX) into a new texture, tagged Fixture: the
    /// stage's upload boundary for failure artifacts and B40's reset recovery.
    fn upload_fixture(&mut self, w: usize, h: usize, pixels: &[u8]) -> Result<u64, String> {
        use gpu::abi::purpose;
        let texture = gpu::texture_create(gpu::abi::format::B8G8R8A8, w as u32, h as u32).map_err(|e| std::format!("stage fixture texture: errno {}", -e))?;
        let (r, _) = gpu::upload(self.context, texture, [0, 0, w as u32, h as u32], pixels, (w * 4) as u32, 0, purpose::FIXTURE);
        if r != 0 {
            let _ = gpu::resource_destroy(texture);
            return Err(std::format!("stage fixture upload: errno {}", -r));
        }
        Ok(texture)
    }

    /// A failing case's judged frame differs from its reference: its expected pane and its
    /// difference pane's heat map (section 23.4) exist only on the CPU, so they go up as
    /// failure artifacts (gpu.md section 2.4).
    fn artifacts(&mut self, f: &JudgedFrame) -> Result<(), String> {
        let Some(expected) = &f.expected else { return Ok(()) };
        let matches = expected.chunks_exact(4).zip(f.pixels.chunks_exact(4)).all(|(a, b)| a[..3] == b[..3]);
        if matches || self.panes_for(&f.sha256).is_none_or(|p| p.artifacts.is_some()) {
            return Ok(());
        }
        let (e, d) = artifact_panes(f, expected);
        let et = self.upload_fixture(SQUARE, SQUARE, &e)?;
        let dt = match self.upload_fixture(SQUARE, SQUARE, &d) {
            Ok(t) => t,
            Err(err) => {
                let _ = gpu::resource_destroy(et);
                return Err(err);
            }
        };
        self.artifact_bytes += 2 * (SQUARE * SQUARE * 4) as u64;
        if let Some(p) = self.panes.iter_mut().find(|p| p.hash == f.sha256) {
            p.artifacts = Some((et, dt));
        }
        Ok(())
    }

    /// Panes for a frame of at most `remote::DRAWN_PIXELS` pixels that a live check made,
    /// drawn on the GPU as one solid rectangle per source pixel, the nearest-scaled formula of
    /// section 23.4; for a failing entry also its expected and difference panes, which for
    /// such a frame are solid rectangles too, drawn rather than uploaded.
    fn drawn(&mut self, f: &JudgedFrame, failing: bool) -> Result<(), String> {
        use gpu::abi::{pipeline, stream::encode};
        if f.width * f.height > remote::DRAWN_PIXELS || f.pixels.len() != f.width * f.height * 4 {
            return Err(std::format!("a {}x{} frame is not one the stage draws", f.width, f.height));
        }
        let format = gpu::abi::format::B8G8R8A8;
        let mut made = Vec::new();
        let mut target = |w: usize, h: usize| -> Result<u64, String> {
            let t = gpu::target_create(format, w as u32, h as u32).map_err(|e| std::format!("stage pane: errno {}", -e))?;
            made.push(t);
            Ok(t)
        };
        let expected = f.expected.as_ref().filter(|e| failing && e.chunks_exact(4).zip(f.pixels.chunks_exact(4)).any(|(a, b)| a[..3] != b[..3]));
        let panes = (|| -> Result<Panes, String> {
            let wide = target(WIDE.0, WIDE.1)?;
            let square = target(SQUARE, SQUARE)?;
            let artifacts = match expected {
                Some(_) => Some((target(SQUARE, SQUARE)?, target(SQUARE, SQUARE)?)),
                None => None,
            };
            Ok(Panes { hash: f.sha256, wide, square, artifacts, sampled: false, frame: Some(Arc::new(f.clone())) })
        })();
        let panes = match panes {
            Ok(p) => p,
            Err(e) => {
                for t in made {
                    let _ = gpu::resource_destroy(t);
                }
                return Err(e);
            }
        };
        // Source column (or row) `s` of `n` covers pane columns ceil(s w / n) to ceil((s+1) w / n).
        let block = |s: usize, n: usize, w: usize| ((s * w).div_ceil(n), ((s + 1) * w).div_ceil(n));
        let colour = |p: &[u8]| 0xFF00_0000 | (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32;
        let mut stream = Vec::new();
        let fill = |stream: &mut Vec<u32>, target: u64, w: usize, h: usize, pick: &dyn Fn(usize) -> u32| {
            encode::begin(stream, target);
            let mut source_runs = Vec::new();
            runs(&mut source_runs, target, (0, 0), f.width, f.height, |x, y| pick((y * f.width + x) * 4));
            for (_, colour, rect) in source_runs {
                let (x0, _) = block(rect[0] as usize, f.width, w);
                let (_, x1) = block(rect[2] as usize - 1, f.width, w);
                let (y0, y1) = block(rect[1] as usize, f.height, h);
                if x0 < x1 && y0 < y1 {
                    encode::draw(stream, pipeline::SOLID, colour, &[[x0 as i32, y0 as i32, x1 as i32, y1 as i32]]);
                }
            }
        };
        let actual = |i: usize| colour(&f.pixels[i..i + 4]);
        fill(&mut stream, panes.wide, WIDE.0, WIDE.1, &actual);
        fill(&mut stream, panes.square, SQUARE, SQUARE, &actual);
        if let (Some(e), Some((et, dt))) = (expected, panes.artifacts) {
            fill(&mut stream, et, SQUARE, SQUARE, &|i| colour(&e[i..i + 4]));
            // The heat map of section 23.4: black where equal, else (255, 255 - delta, 0).
            fill(&mut stream, dt, SQUARE, SQUARE, &|i| {
                let delta = (0..3).map(|c| e[i + c].abs_diff(f.pixels[i + c])).max().unwrap_or(0);
                if delta == 0 { 0xFF00_0000 } else { 0xFFFF_0000 | ((255 - delta) as u32) << 8 }
            });
        }
        if let Err(e) = self.run(&stream, "drawn panes") {
            panes.destroy();
            return Err(e);
        }
        self.keep(panes);
        Ok(())
    }

    /// Where the panel cursor is to be drawn: at the pointer, once it has moved from where it
    /// was when these objects were made. Reading it is the kernel's read of the pointing
    /// device for the panel cursor (test point 17).
    fn pointer(&self) -> Option<(i32, i32)> {
        let p = graphics::mouse_pos().ok()?;
        (self.cursor.is_some() || Some(p) != self.pointer_start).then_some((p.0 as i32, p.1 as i32))
    }

    /// Follow the pointer with the panel cursor while a case runs with the display left to the
    /// stage: restore the old footprint from the stage frame, unless test point 10 stopped
    /// that until the next frame, draw the cursor at the pointer, and present.
    fn follow(&mut self, bits: u32) -> Result<(), String> {
        use gpu::abi::{draw, pipeline, stream::encode};
        if bits & (1 << abi::tc_point::PANEL_CURSOR_NO_RESTORE) != 0 {
            self.no_restore = true;
        }
        let Some(at) = self.pointer() else { return Ok(()) };
        if self.cursor == Some(at) {
            return Ok(());
        }
        let target = self.scanout[self.shown];
        let mut stream = Vec::new();
        encode::begin(&mut stream, target);
        let old = self.cursor.filter(|_| !self.no_restore).and_then(|(x, y)| {
            abi::Cursor { shape: abi::cursor_shape::ARROW, visible: 1, x, y }.footprint()
        });
        if let Some(f) = old {
            let r = [f.x.max(0), f.y.max(0), (f.x + f.w).min(self.width as i32), (f.y + f.h).min(self.height as i32)];
            if r[0] < r[2] && r[1] < r[3] {
                let mut params = [0u32; 16];
                params[2] = 1;
                draw::encode(&mut stream, pipeline::COPY, 0, self.canvas, 0, 0, &[draw::Instance { rect: r, params }]);
            }
        }
        encode_cursor(&mut stream, at, self.width, self.height);
        self.run(&stream, "panel cursor")?;
        let (r, presented) = gpu::present(self.context, target);
        if r != 0 {
            return Err(std::format!("panel cursor present: errno {}, answer {:#x}", -r, presented.answer));
        }
        self.cursor = Some(at);
        Ok(())
    }

    /// Show the scanout target again, as the stage last drew it, after a case that started
    /// above the kernel's screen.
    fn represent(&self) -> Result<(), String> {
        graphics::take_over_display().map_err(|e| std::format!("stage display ownership: {e:?}"))?;
        let (r, presented) = gpu::present(self.context, self.scanout[self.shown]);
        if r != 0 {
            return Err(std::format!("stage present: errno {}, answer {:#x}", -r, presented.answer));
        }
        Ok(())
    }

    /// Present the painted canvas with the panel cursor and observe the scanout. Returns the
    /// observation, the present's context fence and the cursor's pointer.
    fn show(&mut self) -> Result<(Seen, u64, Option<(i32, i32)>), String> {
        let (fence, cursor) = self.present(true)?;
        let seen = observe(&mut self.seen)?;
        Ok((seen, fence, cursor))
    }

    /// Copy the painted canvas into the scanout target not shown, on the GPU with the panel
    /// cursor over it when `cursor`, and present it. Returns the present's context fence and
    /// the cursor's pointer.
    fn present(&mut self, cursor: bool) -> Result<(u64, Option<(i32, i32)>), String> {
        use gpu::abi::{draw, pipeline, stream::encode};
        let next = self.shown ^ 1;
        let mut params = [0u32; 16];
        params[2] = 1;
        let mut stream = Vec::new();
        encode::begin(&mut stream, self.scanout[next]);
        draw::encode(&mut stream, pipeline::COPY, 0, self.canvas, 0, 0,
            &[draw::Instance { rect: [0, 0, self.width as i32, self.height as i32], params }]);
        let cursor = self.pointer().filter(|_| cursor);
        if let Some(at) = cursor {
            encode_cursor(&mut stream, at, self.width, self.height);
        }
        self.run(&stream, "stage")?;
        // The stage holds the display between cases (idempotent while it does) and gives
        // it back while a case runs (`release`).
        graphics::take_over_display().map_err(|e| std::format!("stage display ownership: {e:?}"))?;
        let (r, presented) = gpu::present(self.context, self.scanout[next]);
        // The device's native success answer (gpu.md section 1.8, ruling 47).
        let success = if gpu::caps().1.backend == gpu::abi::backend::SVGA3D_DX { 1 } else { 0x1100 };
        if r != 0 || presented.answer != success {
            return Err(std::format!("stage present: errno {}, answer {:#x}", -r, presented.answer));
        }
        self.shown = next;
        self.cursor = cursor;
        self.no_restore = false;
        Ok((presented.fence, cursor))
    }
}

/// A failing frame's expected pane and its difference pane's heat map, without the first
/// mismatch's cross, which the painter draws (section 23.4).
fn artifact_panes(f: &JudgedFrame, expected: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut e = Vec::with_capacity(SQUARE * SQUARE * 4);
    let mut d = Vec::with_capacity(SQUARE * SQUARE * 4);
    for y in 0..SQUARE {
        for x in 0..SQUARE {
            let i = ((y * f.height / SQUARE) * f.width + x * f.width / SQUARE) * 4;
            let delta = (0..3).map(|c| expected[i + c].abs_diff(f.pixels[i + c])).max().unwrap_or(0);
            e.extend_from_slice(&[expected[i], expected[i + 1], expected[i + 2], 255]);
            d.extend_from_slice(&if delta == 0 { [0, 0, 0, 255] } else { [0, 255 - delta, 255, 255] });
        }
    }
    (e, d)
}

/// Create the stage's GPU objects before the memory record's first sample, as the first
/// draw would, so both samples count them, and read the scanout's record into the kept
/// buffer, so the first frame's notice decodes a real record.
pub(super) fn prepare() -> Result<(), String> {
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    let seen = {
        let mut guard = GPU.lock().unwrap();
        if guard.as_ref().is_none_or(|g| g.pid != pid) {
            *guard = Some(Gpu::new(pid)?);
        }
        let g = guard.as_mut().ok_or("stage GPU objects missing")?;
        observe(&mut g.seen)?
    };
    // The font, too, is in both samples.
    FONT.lock().unwrap().ready().map_err(|e| std::format!("stage font: {e:?}"))?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        s.observation = Observation { backend: seen.backend, flags: seen.flags, boundary: seen.boundary };
        // And the painter's wall, glyph atlas and context: the first frame makes them.
        s.draw(0)?;
    }
    Ok(())
}

/// The current mode, which must be the stage's logical layout at a whole display scale.
fn stage_mode() -> Result<gpu::abi::Mode, String> {
    let m = mode()?;
    if m.scale == 0 || (m.width, m.height) != (1280 * m.scale, 800 * m.scale) {
        return Err(std::format!(
            "the {}x{} mode is not the stage's 1280x800 layout at scale {}",
            m.width, m.height, m.scale
        ));
    }
    Ok(m)
}

mod recovery {
    use crate::gpu;

    // Only this module can construct the authorization held with retained pictures.
    pub(super) struct ResetRecovery(());

    pub(super) fn confirmed(context: u64) -> Option<ResetRecovery> {
        let (status, counters) = gpu::counters(context);
        if status == 0 && counters.state == gpu::abi::context_state::RESET {
            Some(ResetRecovery(()))
        } else {
            None
        }
    }
}

/// The stage's GPU objects for this process. After a mode change or a failed frame the
/// scanout targets are made again. A confirmed reset authorizes B40 recovery of
/// pictures into replacement GPU objects; other context loss replaces the submission
/// context while retaining GPU picture resources. A forked case cannot use the runner's handles.
fn gpu_here(guard: &mut Option<Gpu>, runner: u64) -> Result<&mut Gpu, String> {
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    if pid != runner {
        return Err(String::from("a forked case's stage frames are the runner's to draw"));
    }
    if let Some(g) = guard.as_mut().filter(|g| g.pid == pid) {
        let (r, c) = gpu::counters(g.context);
        let live = r == 0 && matches!(c.state, gpu::abi::context_state::LIVE | gpu::abi::context_state::WARMING);
        let moved = mode().is_ok_and(|m| (m.width, m.height) != (g.width, g.height));
        if !live || ((g.stale || moved) && g.redisplay().is_err()) {
            let reset = recovery::confirmed(g.context);
            let replacement = if reset.is_none() {
                Some(gpu::context_create().map_err(|e| std::format!("stage replacement context: errno {}", -e))?)
            } else { None };
            let mut t = RETIRED.lock().unwrap();
            t.artifacts += g.artifact_bytes;
            t.recovery += g.recovery_bytes;
            if r == 0 {
                (t.image, t.fixture) = (t.image + c.uploads_image, t.fixture + c.uploads_fixture);
            } else {
                t.unread += 1;
            }
            if let Some(reset) = reset {
                *KEPT.lock().unwrap() = Some((g.keep_pictures(), reset));
                t.resets += 1;
                g.destroy();
                *guard = None;
            } else {
                // Resource handles belong to the process, not its submission
                // context. Keep the GPU pictures and replace only the dead context.
                let context = replacement.ok_or("stage replacement context missing")?;
                let _ = gpu::context_destroy(core::mem::replace(&mut g.context, context));
                g.artifact_bytes = 0;
                g.recovery_bytes = 0;
                g.stale = true;
                g.redisplay()?;
            }
        }
    }
    if guard.as_ref().is_none_or(|g| g.pid != pid) {
        let mut g = Gpu::new(pid)?;
        let mut kept = KEPT.lock().unwrap();
        if let Some((k, reset)) = kept.as_ref() {
            if let Err(e) = g.restore_pictures(k, reset) {
                g.destroy();
                return Err(e);
            }
        }
        *kept = None;
        *guard = Some(g);
    }
    guard.as_mut().ok_or_else(|| String::from("stage GPU objects missing"))
}

/// One judged frame's panes as a stage frame names them.
#[derive(Clone, Copy)]
struct PaneSet {
    wide: u64,
    square: u64,
    artifacts: Option<(u64, u64)>,
}

/// The pictures a stage frame can show: the thumbnails in the atlas, by tile, and the panes
/// held for each judged frame. The runner's GPU objects hold them; a forked case's oracle
/// reads what the runner holds (`remote::holdings`).
trait Holds {
    fn cell(&self, tile: usize) -> Option<[u8; 32]>;
    fn atlas(&self) -> u64;
    fn panes(&self, hash: &[u8; 32]) -> Option<PaneSet>;
}
impl Holds for Gpu {
    fn cell(&self, tile: usize) -> Option<[u8; 32]> {
        self.cell_hash(tile)
    }
    fn atlas(&self) -> u64 {
        self.atlas
    }
    fn panes(&self, hash: &[u8; 32]) -> Option<PaneSet> {
        self.panes_for(hash).map(|p| PaneSet { wide: p.wide, square: p.square, artifacts: p.artifacts })
    }
}

impl Show {
    /// Draw this stage frame into `fb`, when given, with the CPU oracle and, when `paint`
    /// gives the canvas's size, into the painter's canvas on the GPU, from one model. Tiles
    /// and the spotlight show pictures only where `g` holds their GPU samples, in both.
    fn render(&self, mut fb: Option<&mut FrameBuf>, bits: u32, g: &dyn Holds, paint: Option<(u32, u32)>) -> Result<(), String> {
        let held = |i: usize| {
            self.entries[i].picture.as_ref().filter(|p| g.cell(i) == Some(p.source_hash))
        };
        let mut tiles: Vec<Tile<'_>> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| Tile {
                id: &e.id,
                stamp: e.stamp,
                key: e.key,
                thumbnail: held(i).map(Picture::image),
            })
            .collect();
        let mut thumbs: Vec<Option<ThumbRef>> = (0..self.entries.len())
            .map(|i| {
                held(i).map(|p| {
                    let (x, y) = cell(i);
                    ThumbRef::Picture(PictureRef { handle: g.atlas(), x, y, width: p.width, height: p.height, cover: None })
                })
            })
            .collect();
        // The faults change the actual renderer input, while stored serial results stay intact.
        if bits & (1 << 12) != 0 {
            if let Some(t) = tiles.iter_mut().find(|t| t.stamp == Stamp::Pass) {
                t.stamp = Stamp::Fail;
            }
        }
        let mut substitute = std::vec![0u8;16*12*4];
        for p in substitute.chunks_exact_mut(4) {
            p.copy_from_slice(&[220, 10, 200, 255]);
        }
        if bits & (1 << 13) != 0 {
            if let Some(i) = tiles.iter().position(|t| t.thumbnail.is_some()) {
                tiles[i].thumbnail = Some(Image {
                    pixels: &substitute,
                    width: 16,
                    height: 12,
                });
                thumbs[i] = Some(ThumbRef::Solid { width: 16, height: 12, rgb: [200, 10, 220] });
            }
        }
        let groups: Vec<Milestone<'_>> = self
            .groups
            .iter()
            .map(|(id, first, end)| Milestone {
                id,
                first: *first,
                end: *end,
            })
            .collect();
        let chosen = if self.finale.is_some() {
            self.first_failure.as_ref().map_or(self.selected, |v| v.0)
        } else {
            self.selected
        };
        let picture = if self.finale.is_some() && self.first_failure.is_some() {
            self.first_failure.as_ref().and_then(|v| v.1.as_deref())
        } else {
            self.last.as_deref()
        };
        // A frame whose panes this process does not hold is shown as no picture.
        let panes = picture.and_then(|p| g.panes(&p.sha256));
        let picture = picture.filter(|_| panes.is_some());
        let mut substituted=picture.map(|p|p.pixels.clone()).unwrap_or_default();
        if bits & (1<<13)!=0 && !substituted.is_empty() { substituted[0]^=255; }
        // The pane pixels that sample source pixel (0, 0), which the substitution changes.
        let covered = |p: &JudgedFrame, w: usize, h: usize| [0, 0, w.div_ceil(p.width).min(w), h.div_ceil(p.height).min(h)];
        let reading = self.entries[chosen].reading.as_ref();
        let fallback = reading.map_or(Spotlight::None, |v| Spotlight::Measurement {
            name: &v.name, number: if bits & (1<<13) != 0 { v.number ^ 1 } else { v.number }, unit: &v.unit, bound: v.expect,
        });
        let mut spot = SpotRef::None;
        let spotlight = match (picture, panes) {
            (Some(p), Some(panes)) => {
                // The checked surface is available even when a case has no independent reference.
                // Such a case shows only its actual pane, with no expectation made up to match it.
                let actual = Image {
                    pixels: if bits & (1<<13)!=0 {&substituted} else {&p.pixels},
                    width: p.width,
                    height: p.height,
                };
                let cover = |w: usize, h: usize| {
                    (bits & (1 << 13) != 0).then(|| (covered(p, w, h), [substituted[2], substituted[1], substituted[0]]))
                };
                let pane = |handle: u64, w: usize, h: usize, cover| PictureRef { handle, x: 0, y: 0, width: w, height: h, cover };
                // Expected and difference panes beyond the actual frame exist only for a frame
                // that matches its reference, or as a failing case's failure artifacts.
                let unchanged = p.expected.as_ref().is_some_and(|e| e.chunks_exact(4).zip(p.pixels.chunks_exact(4)).all(|(a, b)| a[..3] == b[..3]));
                let failed = self.entries[chosen].stamp == Stamp::Fail;
                match &p.expected {
                    Some(expected) if unchanged || (failed && panes.artifacts.is_some()) => {
                        let mut differing = 0;
                        let mut first = None;
                        for (i, (a, b)) in expected
                            .chunks_exact(4)
                            .zip(actual.pixels.chunks_exact(4))
                            .enumerate()
                        {
                            if a[..3] != b[..3] {
                                differing += 1;
                                first.get_or_insert((i % p.width, i / p.width));
                            }
                        }
                        let heat = cover(SQUARE, SQUARE).map(|(r, _)| {
                            let delta = (0..3).map(|c| expected[c].abs_diff(substituted[c])).max().unwrap_or(0);
                            (r, if delta == 0 { [0, 0, 0] } else { [255, 255 - delta, 0] })
                        });
                        let artifacts = panes.artifacts.filter(|_| !unchanged);
                        spot = SpotRef::Picture {
                            expected: pane(artifacts.map_or(panes.square, |a| a.0), SQUARE, SQUARE, None),
                            actual: pane(panes.square, SQUARE, SQUARE, cover(SQUARE, SQUARE)),
                            difference: DiffRef {
                                artifact: artifacts.map(|a| pane(a.1, SQUARE, SQUARE, None)),
                                heat: heat.into_iter().collect(),
                                cross: first.map(|(x, y)| (x * SQUARE / p.width, y * SQUARE / p.height)),
                            },
                        };
                        Spotlight::Picture {
                            expected: Image {
                                pixels: expected,
                                width: p.width,
                                height: p.height,
                            },
                            actual,
                            first,
                            differing,
                            bound: p.bound,
                        }
                    }
                    _ => {
                        spot = SpotRef::Actual(pane(panes.wide, WIDE.0, WIDE.1, cover(WIDE.0, WIDE.1)));
                        Spotlight::Actual(actual)
                    }
                }
            }
            _ => fallback,
        };
        let spotlight =
            self.entries[chosen]
                .timing
                .as_ref().filter(|_| picture.is_none())
                .map_or(spotlight, |t| Spotlight::Timing {
                    frame_us: &t.frame_us,
                    budget_us: t.budget_us,
                });
        let mut substituted_input = self.entries[chosen].input.clone();
        if bits & (1<<13) != 0 {
            if let Some(input) = &mut substituted_input { input.hit = "substituted handler result".into(); }
        }
        let input=substituted_input.as_ref();
        let has_picture=picture.is_some();
        let spotlight = if has_picture {spotlight} else {input.map_or(spotlight, |t|
            Spotlight::Input { points: &t.points, hit: &t.hit })};
        let slow = self.entries.iter().max_by_key(|e| e.key).ok_or("stage has no cases")?;
        let mut run = stage::Run {
            suite: &self.suite,
            // Record version 2 names the GPU device, which ARM64 QEMU and Parallels
            // share, rather than the target (contract section 20.2).
            target: match self.observation.backend {
                gpu::abi::backend::VIRGL => "VirGL",
                gpu::abi::backend::SVGA3D_DX => "SVGA3D DX",
                _ => "unknown display",
            },
            case_number: self.current + 1,
            ui_time_ms: 0,
            fps: self.fps,
            tiles: &tiles,
            milestones: &groups,
            selected: chosen,
            spotlight,
            input: input.filter(|_|has_picture).map(|t|(t.points.as_slice(),t.hit.as_str())),
            finale: self.finale.map(|started_ms| Finale {
                started_ms,
                slowest: &slow.id,
                slowest_ms: slow.key,
            }),
            boundary: self.observation,
        };
        match fb.as_deref_mut() {
            Some(fb) => render_at_ui_time(fb, &mut run, self.started_ms).map_err(|e| std::format!("stage render: {e:?}"))?,
            None => run.ui_time_ms = (clock::now_ns() / 1_000_000).saturating_sub(self.started_ms),
        }
        // Reading also drains the kernel's producer matcher while the stage owns
        // the display and the console cannot refresh its boot ribbon.
        std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e| e.to_string())?;
        self.observation.notice().map_err(|e| std::format!("stage observation: {e:?}"))?;
        let Some((width, height)) = paint else { return Ok(()) };
        let frame = Frame { width, height, run: &run, thumbs: &thumbs, spot: &spot };
        painter(|p| p.paint(&frame))
    }

    /// Draw, present and check one stage frame; on failure the next attempt starts from new
    /// scanout targets, or from a new context and objects if the context was lost.
    fn attempt(&self, fb: &mut FrameBuf, bits: u32) -> Result<Seen, String> {
        let mut guard = GPU.lock().unwrap();
        let g = gpu_here(&mut guard, self.runner)?;
        let shown_frame = self.render(Some(&mut *fb), bits, g, Some((g.width, g.height)))
            .and_then(|()| g.show())
            .and_then(|(seen, fence, cursor)| shown(fb, Some(fence), &g.seen, &seen, cursor).map(|()| seen));
        g.stale = shown_frame.is_err();
        shown_frame
    }

    /// The oracle's frame alone, with the pictures `held`.
    fn oracle(&self, fb: &mut FrameBuf, held: &dyn Holds) -> Result<(), String> {
        self.render(Some(fb), 0, held, None)
    }

    /// Draw and present this stage frame for a forked case's live check, which judges the
    /// scanout itself: no CPU frame, readback or comparison here, and no panel cursor.
    fn paint_served(&self, bits: u32) -> Result<(), String> {
        let mut failed = String::new();
        for _ in 0..3 {
            let mut guard = GPU.lock().unwrap();
            let g = gpu_here(&mut guard, self.runner)?;
            let drawn = self.render(None, bits, g, Some((g.width, g.height))).and_then(|()| g.present(false)).map(|_| ());
            match drawn {
                Ok(()) => return Ok(()),
                Err(e) => {
                    g.stale = true;
                    failed = e;
                }
            }
        }
        Err(failed)
    }

    fn in_runner(&self) -> Result<bool, String> {
        Ok(process::getpid().map_err(|e| e.to_string())?.raw() as u64 == self.runner)
    }

    /// Draw and show this stage frame. In a forked case the runner draws it (`remote`).
    fn draw(&mut self, bits: u32) -> Result<(), String> {
        if !self.in_runner()? {
            return remote::draw(self, bits);
        }
        // As the GPU suite's view does: no test-control point left armed by a case can alter
        // the stage or be spent by it.
        let _ = gpu::tc_disarm_all();
        self.paint(bits)
    }

    /// Draw, present and check this stage frame with the runner's GPU objects.
    fn paint(&mut self, bits: u32) -> Result<(), String> {
        // Rendered at the display's physical size (contract section 23.2, ruling B24).
        let m = mode()?;
        let mut fb = FrameBuf::new(m.width as usize, m.height as usize, Format::BGRX).map_err(|e| e.to_string())?;
        // Before the stage's first frame, learn the actual backend from the scanout, so the
        // notice on this same frame decodes a real record.
        if self.observation.backend == 0 {
            let (_, rec) = snapshot()?;
            self.observation = Observation { backend: rec.backend, flags: rec.flags, boundary: rec.boundary };
        }
        // A case may reset the device (section 8.4's scratch reset does), which loses the
        // stage's and the painter's GPU objects: a failed frame is drawn again from new
        // objects, for at most three attempts.
        let mut failed = String::new();
        let mut rec = None;
        for _ in 0..3 {
            match self.attempt(&mut fb, bits) {
                Ok(seen) => {
                    rec = Some(seen);
                    break;
                }
                Err(e) => failed = e,
            }
        }
        let rec = rec.ok_or(failed)?;
        self.observation = Observation {
            backend: rec.backend,
            flags: rec.flags,
            boundary: rec.boundary,
        };
        Ok(())
    }
}
/// No composite by the runner completed inside any of these measured windows but those the
/// case asked for.
fn quiet(windows: &[(u64, u64, u64)]) -> bool {
    windows.iter().all(|&(before, after, asked)| after.checked_sub(before) == Some(asked))
}
pub(super) fn init(suite: &Suite) -> u64 {
    let mut entries = Vec::new();
    let mut groups: Vec<(String, usize, usize)> = Vec::new();
    for c in suite.categories {
        let first = entries.len();
        for k in c.cases {
            entries.push(Entry {
                id: std::format!("{}/{}", c.id, k.id),
                stamp: Stamp::Pending,
                key: 0,
                line: String::new(),
                picture: None,
                published: false,
                timing: None,
                input: None,
                reading: None,
            });
        }
        // Each manifest category gets its own completion light, including the three
        // image categories that share an effort milestone.
        groups.push((String::from(c.id), first, entries.len()));
    }
    let pace_ms = if suite.id == "breengel" { pace_ms() } else { 0 };
    *SHOW.lock().unwrap() = Some(Show {
        suite: String::from(suite.id),
        entries,
        groups,
        current: 0,
        selected: 0,
        last: None,
        first_failure: None,
        // Not yet observed: the first draw reads the scanout's record before rendering.
        observation: Observation {
            backend: 0,
            flags: 0,
            boundary: 0,
        },
        started_ms: clock::now_ns() / 1_000_000,
        finale: None,
        fps: 0,
        windows: Vec::new(),
        measured: None,
        requested: 0,
        pace_ms,
        runner: process::getpid().map_or(0, |p| p.raw() as u64),
    });
    pace_ms
}
pub(super) fn draw(states: &[Vec<State>]) -> Result<(), String> {
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    let mut index = 0;
    for cat in states {
        for state in cat {
            if matches!(state, State::Running) {
                s.current = index;
                s.entries[index].stamp = Stamp::Running;
            }
            index += 1;
        }
    }
    s.draw(fault_bits())
}
pub(super) fn record(
    line: &str,
    outcome: &Outcome,
    frame: Option<JudgedFrame>,
    timing: Option<FrameTiming>,
    fps: Option<u32>,
    values: &[super::suite::Value],
    limit_ms: u64,
    input: Option<super::suite::InputTrace>,
) -> Result<(), String> {
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    if let Some(fps) = fps {
        s.fps = fps;
    }
    let e = &mut s.entries[s.current];
    e.line = String::from(line);
    e.timing = timing;
    if input.is_some() { e.input = input; }
    (e.stamp, e.key) = match outcome {
        Outcome::Pass { ms } => (Stamp::Pass, *ms),
        Outcome::Fail { ms, .. } => (Stamp::Fail, *ms),
        Outcome::Skip { .. } => (Stamp::NotReached, 0),
    };
    e.reading = if matches!(outcome, Outcome::Skip { .. }) { None } else { Some(values.iter().rev().find(|v|v.expect.is_some()).or_else(||values.last()).cloned()
        .unwrap_or(super::suite::Value { name: String::from("Case execution time"), number: e.key as i64,
            unit: String::from("ms"), expect: Some((0,limit_ms as i64)) })) };
    let judged = frame.as_ref().map(|f| f.sha256);
    if let Some(f) = frame {
        verify_source(line, &f)?;
        let source = libpixcheck::Frame::new(
            &f.pixels,
            f.width,
            f.height,
            f.width * 4,
            libpixcheck::Format::Bgrx,
        )
        .map_err(|e| std::format!("thumbnail source: {e:?}"))?;
        let (width, height) = (f.width.min(16), f.height.min(12));
        let pixels = libpixcheck::thumbnail(source, width, height)
            .map_err(|e| std::format!("thumbnail: {e:?}"))?;
        e.picture = Some(Picture {
            width,
            height,
            source_hash: libpixcheck::sha256(&f.pixels),
            thumb_hash: libpixcheck::sha256(&pixels),
            pixels,
        });
        s.last = Some(Arc::new(f));
    } else if s.selected != s.current {
        s.last = None;
    }
    if matches!(outcome, Outcome::Fail { .. }) && s.first_failure.is_none() {
        s.first_failure = Some((s.current, s.last.clone()));
    }
    s.selected = s.current;
    // The pictures sampled while this case's process lived are those of the frame it handed
    // over; a failing frame's expected and difference panes go up as failure artifacts.
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    if let Some(g) = GPU.lock().unwrap().as_mut().filter(|g| g.pid == pid) {
        let picture = s.entries[s.current].picture.as_ref();
        g.bind(s.current, judged.and(picture.zip(s.last.as_ref())));
        if let Some(f) = s.last.as_deref().filter(|f| matches!(outcome, Outcome::Fail { .. }) && Some(f.sha256) == judged) {
            g.artifacts(f)?;
        }
        let wanted: Vec<[u8; 32]> = [s.last.as_ref().map(|f| f.sha256), s.first_failure.as_ref().and_then(|v| v.1.as_ref().map(|f| f.sha256))]
            .into_iter()
            .flatten()
            .collect();
        g.prune(&wanted);
    }
    Ok(())
}

/// What a case's process published with its judged frame, for the stage's pictures.
pub(super) enum Published {
    /// A window holding a GPU copy of the frame, `width` x `height`.
    Window { id: u64, width: usize, height: usize },
    /// The frame is the stage's own, as the scanout shows it.
    Stage,
}

/// Sample the frame the running case published into its tile's and the spotlight's
/// pictures, after the case's measured window and while its process still holds the frame.
pub(super) fn sample(published: Published) -> Result<(), String> {
    let (tile, runner) = {
        let mut guard = SHOW.lock().unwrap();
        let s = guard.as_mut().ok_or("stage is not initialized")?;
        s.entries[s.current].published = true;
        (s.current, s.runner)
    };
    // As the stage's draw does: no test point a case left armed alters the sampling.
    let _ = gpu::tc_disarm_all();
    let mut guard = GPU.lock().unwrap();
    let g = gpu_here(&mut guard, runner)?;
    match published {
        Published::Window { id, width, height } => g.sample_window(id, width, height, tile),
        Published::Stage => g.sample_stage(tile),
    }
}
/// Count kernel-published records for a process, not calls or runner timestamps.
fn kernel_serial(pid: u64) -> Result<u64, String> {
    let text = std::fs::read_to_string("/proc/breenix/stage-composites").map_err(|e| e.to_string())?;
    let mut last = 0;
    for line in text.lines() {
        let (owner, serial) = line.split_once(' ').ok_or("bad kernel composite record")?;
        if owner.parse::<u64>().map_err(|_| "bad composite owner")? == pid {
            last = serial.parse::<u64>().map_err(|_| "bad composite serial")?;
        }
    }
    Ok(last)
}
fn verify_source(line: &str, f: &JudgedFrame) -> Result<(), String> {
    let hash: String = libpixcheck::sha256(&f.pixels).iter().map(|b| std::format!("{b:02x}")).collect();
    if !line.split_whitespace().any(|w| w == std::format!("sha256={hash}")) {
        return Err("selected source pixels differ from the emitted CASE SHA-256".into());
    }
    Ok(())
}
pub(super) fn begin() -> Result<(), String> {
    // Ruling B37: the stage paints only between cases, and a case starts once the stage's
    // GPU work is fence-complete, its destroyed contexts' work included (at most 1 s).
    if GPU.lock().unwrap().is_some() {
        painter(|p| p.settle())?;
        let start = time::now_monotonic().map_or(0, |t| t.as_nanos());
        loop {
            let (r, c) = gpu::counters(0);
            if r != 0 || c.device_contexts <= c.contexts {
                break;
            }
            if time::now_monotonic().map_or(0, |t| t.as_nanos()).saturating_sub(start) >= 1_000_000_000 {
                return Err(std::format!("the stage's {} destroyed contexts still had work 1 s before the case", c.device_contexts - c.contexts));
            }
            let _ = time::sleep_ms(1);
        }
    }
    // The runner's presents, also when a forked case's live check runs this bracket.
    let runner = SHOW.lock().unwrap().as_ref().map_or(0, |s| s.runner);
    let serial = kernel_serial(runner)?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() { s.measured = Some((serial, s.requested)); }
    Ok(())
}
/// The cases whose property is the kernel's screen itself, or the depth of the ownership
/// stack: they start above the kernel's screen (ruling B29).
const ABOVE_KERNEL_SCREEN: [&str; 3] = ["composition/ownership-stack", "panel/kernel-screens", "panel/boot-ribbon"];
/// The cases whose property is the panel cursor, which the stage's compositor draws (ruling
/// B35): the display stays with the stage while they run.
const STAGE_CUSTODY: [&str; 2] = ["panel/cursor-restore", "panel/cursor-trail-detected"];

/// The cases whose property is the run stage's own frames, which the runner draws for them
/// (`remote`): the display stays with the stage while they run.
const LIVE_CHECKS: [&str; 4] = ["panel/judged-frame-shown", "panel/serial-agreement", "panel/quiet-window", "panel/boundary-notice"];

/// Whether case `name`'s process takes the display over when it starts, nesting above the
/// stage.
pub(super) fn case_takes_display(name: &str) -> bool {
    !STAGE_CUSTODY.contains(&name) && !LIVE_CHECKS.contains(&name)
}

/// Before a case runs (ruling B29): the stage keeps the display, and the case's own
/// takeover nests above it, so the matrix stays on screen until the case presents and is
/// restored when it ends, without the console showing between cases. The cases above the
/// kernel's screen start there: the stage gives the display back for them. The panel
/// cursor follows the pointer while a case runs with the display left to the stage.
pub(super) fn release_for(name: &str) -> Result<(), String> {
    FOLLOW.store(STAGE_CUSTODY.contains(&name), core::sync::atomic::Ordering::Relaxed);
    if ABOVE_KERNEL_SCREEN.contains(&name) {
        release()
    } else {
        Ok(())
    }
}

/// After a case that started above the kernel's screen, the stage takes the display back and
/// shows its frame at once, so the console does not show between cases.
pub(super) fn restore_after(name: &str) -> Result<(), String> {
    if !ABOVE_KERNEL_SCREEN.contains(&name) {
        return Ok(());
    }
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    match GPU.lock().unwrap().as_ref().filter(|g| g.pid == pid) {
        Some(g) => g.represent(),
        None => Ok(()),
    }
}

/// The stage gives the display back. Its next draw takes it again; an EPERM means the stage
/// does not hold it.
fn release() -> Result<(), String> {
    match graphics::give_back_display() {
        Ok(()) | Err(crate::error::Error::Os(crate::errno::Errno::EPERM)) => Ok(()),
        Err(e) => Err(std::format!("stage display give-back: {e}")),
    }
}
pub(super) fn end() -> Result<(), String> {
    FOLLOW.store(false, core::sync::atomic::Ordering::Relaxed);
    let runner = SHOW.lock().unwrap().as_ref().map_or(0, |s| s.runner);
    let serial = kernel_serial(runner)?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        if let Some((a, asked)) = s.measured.take() { s.windows.push((a, serial, s.requested - asked)); }
    }
    Ok(())
}
pub(super) fn tick() -> Result<(), String> {
    let bits = fault_bits();
    // Test point 14 plants a stage draw inside the measured window. It is drawn from a copy:
    // in a forked case the runner draws it, reading the stage itself.
    if bits & (1 << 14) != 0 {
        let mut s = SHOW.lock().unwrap().clone().ok_or("stage missing")?;
        s.draw(bits)?;
        if let Some(show) = SHOW.lock().unwrap().as_mut() {
            show.observation = s.observation;
        }
    }
    if FOLLOW.load(core::sync::atomic::Ordering::Relaxed) {
        let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
        if let Some(g) = GPU.lock().unwrap().as_mut().filter(|g| g.pid == pid) {
            g.follow(bits)?;
        }
    }
    Ok(())
}
pub(super) fn redraw() -> Result<(), String> {
    SHOW.lock().unwrap().as_mut().ok_or("stage missing")?.draw(0)
}
fn pace_ms() -> u64 {
    // QEMU publishes its boot fw_cfg value through procfs; firmware VMs receive
    // the same setting on the runner's private boot disk.
    ["/proc/breenix/pace", "/etc/breenix/pace"].iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .find_map(|s|s.trim().parse::<u32>().ok())
        .map_or(0,u64::from)
}
pub(super) fn completed() -> Result<(), String> {
    let pace_ms = {
        let mut guard=SHOW.lock().unwrap();
        let s=guard.as_mut().ok_or("stage missing")?;
        check(s.measured.is_none(),"spotlight hold entered a measured window").map_err(|e|std::format!("{e:?}"))?;
        s.draw(0)?;
        s.pace_ms
    };
    // Holds advance elapsed UI time but never case timers or frame-time samples.
    if pace_ms>0 {time::sleep_ms(pace_ms).map_err(|e|e.to_string())?;}
    Ok(())
}
pub(super) fn finale() -> Result<(), String> {
    let start = clock::now_ns() / 1_000_000;
    {
        let mut g = SHOW.lock().unwrap();
        let s = g.as_mut().ok_or("stage missing")?;
        s.finale = Some(start);
        s.draw(0)?;
        // Ruling B36: a passing run uploads no picture. Each context the stage draws with is
        // counted: the stage uploads failure artifacts and B40 recovery, and the painter
        // uploads declared tables and crossing geometry; Image remains forbidden. The stage's count includes
        // the contexts it let go when its objects were made again.
        let guard = GPU.lock().unwrap();
        let g = guard.as_ref().filter(|g| g.pid == s.runner).ok_or("the stage holds no GPU objects at the finale")?;
        let (painted, declared) = painter(|p| Ok((p.context()?, p.declared_bytes())))?;
        let t = *RETIRED.lock().unwrap();
        if t.unread != 0 {
            return Err(std::format!("the counters of {} stage contexts let go could not be read", t.unread));
        }
        check(g.recovery_bytes + t.recovery == 0 || t.resets > 0, "stage recovery uploads have no confirmed device reset").map_err(|e| std::format!("{e:?}"))?;
        super::suite::emit(&std::format!("BREENGEL UPLOADS recovery_bytes={} device_resets={} failure_artifact_bytes={}", g.recovery_bytes + t.recovery, t.resets, g.artifact_bytes + t.artifacts));
        for (context, what, earlier, allowed) in [(g.context, "stage", t, g.artifact_bytes + t.artifacts + g.recovery_bytes + t.recovery), (painted, "stage painter", Retired { image: 0, fixture: 0, artifacts: 0, recovery: 0, resets: 0, unread: 0 }, declared)] {
            let (r, c) = gpu::counters(context);
            check(r == 0, "the stage's COUNTERS failed").map_err(|e| std::format!("{e:?}"))?;
            let (image, fixture) = (c.uploads_image + earlier.image, c.uploads_fixture + earlier.fixture);
            if image != 0 || fixture != allowed {
                return Err(std::format!(
                    "the {what} contexts uploaded {image} Image and {fixture} Fixture bytes; they account for {allowed} Fixture bytes"
                ));
            }
        }
    }
    let watchdog = time::now_monotonic().map_err(|e|e.to_string())?.as_nanos();
    let finale = Finale { started_ms: start, slowest: "", slowest_ms: 0 };
    while !finale.hold_complete(clock::now_ns() / 1_000_000) {
        if time::now_monotonic().map_err(|e|e.to_string())?.as_nanos() - watchdog > 60_000_000_000 {
            return Err("finale UI clock did not complete its 5000 ms hold within the runner deadline".into());
        }
        time::sleep_ms(20).map_err(|e| e.to_string())?;
    }
    Ok(())
}

mod checks;
mod remote;
pub use checks::showcase_check;
pub(crate) use remote::serve;

/// Render a fixture using the same clock boundary as the live stage.
pub fn render_at_ui_time(fb: &mut FrameBuf, run: &mut stage::Run<'_>, started_ms: u64) -> Result<(), stage::StageError> {
    run.ui_time_ms = (clock::now_ns() / 1_000_000).saturating_sub(started_ms);
    stage::draw_with(fb, run, &mut FONT.lock().unwrap())
}

/// Retire the stage's owners after its five-second finale. The display core retains
/// the currently bound scanout independently of its public handle and context.
pub(super) fn retire() -> Result<(), String> {
    FOLLOW.store(false, core::sync::atomic::Ordering::Relaxed);
    painter(|p| p.settle())?;
    if let Some(g) = GPU.lock().unwrap().take() { g.destroy(); }
    painter(|p| p.retire())?;
    *SHOW.lock().unwrap() = None;
    *KEPT.lock().unwrap() = None;
    *FONT.lock().unwrap() = StageFont::new();
    // Destroy revokes handles immediately; charges end only when device unrefs
    // complete. Wait for that owner boundary, not for a guessed sleep interval.
    let start = time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
    loop {
        let (r, c) = gpu::counters(0);
        if r != 0 { return Err(std::format!("stage retirement counters: errno {}", -r)); }
        if c.contexts == 0 && c.device_contexts == 0 && c.resources == 0 && c.copy_bytes == 0 {
            break;
        }
        if time::now_monotonic().map_err(|e| e.to_string())?.as_nanos().saturating_sub(start) >= 5_000_000_000 {
            return Err(std::format!("stage retirement still holds {} contexts, {} device contexts, {} resources and {} copy bytes", c.contexts, c.device_contexts, c.resources, c.copy_bytes));
        }
        time::sleep_ms(5).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Ownership at the memory-record boundary, including the runner's CPU pictures.
pub(super) fn memory_owners(when: &str) {
    let (r, c) = gpu::counters(0);
    let threads = process::getpid().map_err(|e| e.to_string())
        .and_then(|pid| std::fs::read_dir(std::format!("/proc/{}/task", pid.raw())).map(|d| d.count()).map_err(|e| e.to_string()))
        .map_or_else(|e| std::format!("unavailable:{e}"), |n| n.to_string());
    let show = SHOW.lock().unwrap();
    let picture_bytes = show.as_ref().map_or(0, |s| {
        let frames = [s.last.as_ref(), s.first_failure.as_ref().and_then(|v| v.1.as_ref())];
        frames.into_iter().flatten().map(|f| f.pixels.len() + f.expected.as_ref().map_or(0, Vec::len)).sum::<usize>()
            + s.entries.iter().filter_map(|e| e.picture.as_ref()).map(|p| p.pixels.len()).sum::<usize>()
    });
    let guard = GPU.lock().unwrap();
    let (seen, panes, cells) = guard.as_ref().map_or((0, 0, 0), |g| (g.seen.len(), g.panes.len(), g.cells.iter().flatten().count()));
    super::suite::emit(&std::format!("BREENGEL OWNERS {when} gpu_status={r} contexts={} device_contexts={} resources={} resource_bytes={} copy_bytes={} picture_bytes={picture_bytes} readback_bytes={seen} panes={panes} thumbnails={cells} runner_threads={threads}", c.contexts, c.device_contexts, c.resources, c.resource_bytes, c.copy_bytes));
}

#[cfg(test)]
mod kept_picture_tests {
    use super::*;

    #[test]
    fn runs_paint_every_pixel_once_in_its_colour() {
        let (w, h) = (5, 3);
        let colour = |x: usize, y: usize| if (x + y) % 3 == 0 { 0xFF00_00FF } else { 0xFF10_2030 + y as u32 };
        let mut out = Vec::new();
        runs(&mut out, 9, (32, 12), w, h, colour);
        let mut painted = std::vec![None; w * h];
        for (target, c, r) in out {
            assert_eq!(target, 9);
            for y in r[1]..r[3] {
                for x in r[0]..r[2] {
                    let at = (y as usize - 12) * w + (x as usize - 32);
                    assert!(painted[at].replace(c).is_none(), "a pixel is painted twice");
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                assert_eq!(painted[y * w + x], Some(colour(x, y)));
            }
        }
    }
}
