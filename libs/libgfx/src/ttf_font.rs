//! TrueType geometry on the CPU, coverage and glyph storage on the GPU.
//! Glyphs live in B8G8R8A8 targets (B15/B19): a cached glyph in a slot of an atlas page,
//! an uncached one in a never-used region of a scratch target. The coverage of a run of
//! glyphs is planned on the CPU and sent in as few submissions as the work budget allows
//! (B33); the atlas waits once for a run, and a caller that reads pixels back waits only
//! by its READBACK, which waits for the target's writes.
use alloc::{sync::Arc, vec, vec::Vec};
use core::cell::UnsafeCell;
use core::ops::Deref;
use core::sync::atomic::{AtomicU64, Ordering};
use bgl::{Instance, pipeline};
use libbreenix::gpu::{self, abi};
use libfont::{FontError, GlyphLayout, GlyphBitmap, SubpixelBitmap, CacheStats};
use libfont::crossings::CrossingTable;
use libfont::outline::LineSegment;
use crate::{color::Color, device, surface::{Surface, Format, Box64, word}, work};

pub type Error = FontError;

/// A parsed font whose raster entry points render GPU targets and return readback.
/// Parsing, metrics, outline expansion and flattening remain in libfont.
pub struct Font(libfont::Font);
impl Deref for Font { type Target = libfont::Font; fn deref(&self) -> &Self::Target { &self.0 } }
impl Font {
    pub fn parse(bytes: &[u8]) -> Result<Self, FontError> { libfont::Font::parse(bytes).map(Self) }
    pub fn size26(size: f32) -> Result<i32, FontError> { libfont::Font::size26(size) }
    pub fn rasterize_glyph(&self, g: u16, size: f32) -> Result<GlyphBitmap, Error> {
        self.rasterize_glyph26(g, Self::size26(size)?)
    }
    pub fn rasterize_glyph26(&self, g: u16, size26: i32) -> Result<GlyphBitmap, Error> {
        self.send_glyph26(g, size26)?.read()
    }
    /// `rasterize_glyph`'s coverage, sent to the GPU and read back later by `SentGlyph::read`,
    /// so a caller with several rasterizations waits for the device once rather than per call.
    pub fn send_glyph(&self, g: u16, size: f32) -> Result<SentGlyph, Error> {
        self.send_glyph26(g, Self::size26(size)?)
    }
    fn send_glyph26(&self, g: u16, size26: i32) -> Result<SentGlyph, Error> {
        self.glyph(g, size26, false).map(SentGlyph)
    }
    pub fn rasterize_glyph_subpixel(&self, g: u16, size: f32) -> Result<SubpixelBitmap, Error> {
        self.glyph(g, Self::size26(size)?, true)?.subpixel()
    }
    fn glyph(&self, g: u16, size26: i32, lcd: bool) -> Result<AtlasGlyph, Error> {
        let mut run = Run::default();
        let glyph = self.plan(g, size26, lcd, &mut run)?;
        run.send()?;
        Ok(glyph)
    }
    /// Queue glyph `g`'s coverage on `run`, in scratch regions; its pixels are there once
    /// `run` is sent.
    fn plan(&self, g: u16, size26: i32, lcd: bool, run: &mut Run) -> Result<AtlasGlyph, Error> {
        let outline = self.0.glyph_outline(g)?;
        match self.0.layout_glyph(&outline, g, size26)? {
            GlyphLayout::Empty { advance } => Ok(AtlasGlyph::empty(advance)),
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => {
                let region = if lcd { run.lcd(&segments, width, height)? } else {
                    admit(&segments, width, height)?;
                    let region = region(0, width, height)?;
                    run.spans(&segments, width, height, &region);
                    region
                };
                let mut glyph = AtlasGlyph::at(region, width, height);
                (glyph.x_offset, glyph.y_offset) = (x_offset, y_offset);
                Ok(glyph)
            }
        }
    }
}

/// Uncached glyphs: rows of the process's scratch targets, each handed out once, so a
/// glyph's region is as the core zeroed it and its draws and work are a new target's.
struct ScratchSlot { owner: AtomicU64, targets: UnsafeCell<[Option<Scratch>; 2]>, geometry: UnsafeCell<Option<Geometry>> }
// SAFETY: the targets and geometry are only reached with the PID-tagged owner lock held.
unsafe impl Sync for ScratchSlot {}
static SCRATCH: ScratchSlot = ScratchSlot { owner: AtomicU64::new(0), targets: UnsafeCell::new([None, None]), geometry: UnsafeCell::new(None) };

struct ScratchHeld(u64);
impl Drop for ScratchHeld { fn drop(&mut self) { SCRATCH.owner.store(self.0 << 1, Ordering::Release); } }
fn scratch_lock() -> Result<ScratchHeld, Error> {
    let me = device::pid();
    if me == 0 { return Err(Error::Device(-abi::errno::EIO)); }
    let inherited = device::acquire(&SCRATCH.owner, me);
    // A fork may copy either slot mid-update. Abandon foreign handles without reading them.
    if inherited { unsafe { SCRATCH.targets.get().write([None, None]); SCRATCH.geometry.get().write(None); } }
    Ok(ScratchHeld(me))
}

/// Release process-local uncached glyph targets and crossing storage at renderer shutdown.
pub(crate) fn retire_scratch() -> Result<(), Error> {
    let _held = scratch_lock()?;
    // SAFETY: scratch_lock owns both slots, and discards inherited handles first.
    unsafe {
        *SCRATCH.targets.get() = [None, None];
        *SCRATCH.geometry.get() = None;
    }
    Ok(())
}

/// A `width × height` region of scratch target `which`, its rows handed out once each.
fn region(which: usize, width: usize, height: usize) -> Result<Region, Error> {
    let _held = scratch_lock()?;
    // SAFETY: the PID-tagged lock is held for the rest of this call.
    let slot = unsafe { &mut (*SCRATCH.targets.get())[which] };
    let context = device::with(|d| Ok(d.context.handle())).map_err(Error::Device)?;
    let fits = slot.as_ref().is_some_and(|s| s.context == context && s.target.owned_here() && width <= s.target.width()
        && s.next_row + height <= s.target.height());
    if !fits {
        *slot = Some(Scratch { target: Arc::new(target(width.max(SCRATCH_SIDE), height.max(SCRATCH_SIDE))?), context, next_row: 0 });
    }
    let s = slot.as_mut().expect("scratch made above");
    let y = s.next_row;
    s.next_row += height;
    Ok(Region { target: s.target.clone(), x: 0, y })
}

/// The crossing wire layout, useful for independent host inspection.
pub fn crossing_table(segments: &[LineSegment], first_row: usize, rows: usize) -> Vec<u8> {
    CrossingTable::new(segments, first_row, rows).bytes().to_vec()
}

/// A scratch target's side when no glyph needs more; its rows are handed out once each.
const SCRATCH_SIDE: usize = 256;
// Context handles change when device::with replaces a reset context after EIO.
struct Scratch { target: Arc<Surface>, context: u64, next_row: usize }

/// A glyph-sized rectangle of a target, at (x, y).
struct Region { target: Arc<Surface>, x: usize, y: usize }

/// A target as `BEGIN` names it.
#[derive(Clone, Copy)]
struct Dest { handle: u64, width: u32, height: u32 }

/// What one draw of a run draws over its rectangle.
#[derive(Clone, Copy)]
enum Draw {
    /// coverage_spans from the run's crossing table `table`, with the glyph's origin and size.
    Spans { table: usize, origin: [i32; 4] },
    Solid(u32),
    Sampled { pipeline: u32, colour: u32, texture: u64, params: [i32; 8] },
}

/// One draw of a run: its target and rectangle, and its work estimate.
struct Step { dest: Dest, rect: [i32; 4], draw: Draw, cost: u64 }

/// The coverage of one or more glyphs (B33): draws and the crossing tables they read,
/// planned on the CPU, then sent by `send` in submissions that each keep within half the
/// work cap and `TABLE_BYTES` of tables. The targets they write and sample are kept until
/// then. One context runs its submissions in order, so a later draw sees an earlier one.
#[derive(Default)]
struct Run { tables: Vec<CrossingTable>, steps: Vec<Step>, keep: Vec<Arc<Surface>> }
impl Run {
    fn dest(&mut self, target: &Arc<Surface>) -> Dest {
        if !self.keep.iter().any(|k| Arc::ptr_eq(k, target)) { self.keep.push(target.clone()); }
        Dest { handle: target.handle(), width: target.width() as u32, height: target.height() as u32 }
    }

    /// Coverage of `segments` into the glyph-sized `region`, as the core cleared it. Each
    /// table's rows are split into draws by their actual crossings, each draw's estimate at
    /// most half the work cap; no pixel values reach the CPU.
    fn spans(&mut self, segments: &[LineSegment], width: usize, height: usize, region: &Region) {
        if width == 0 || height == 0 { return; }
        let budget = abi::limits::WORK / 2;
        let dest = self.dest(&region.target);
        let rows_per_table = (TABLE_BYTES / (32 + 64 * segments.len().max(1))).max(1);
        let (ox, oy) = (region.x as i32, region.y as i32);
        for row in (0..height).step_by(rows_per_table) {
            let count = rows_per_table.min(height - row);
            let table = CrossingTable::new(segments, row, count);
            // Crossings on each row: the difference of its first sub-scanline offsets.
            let offset = |i: usize| u32::from_le_bytes(table.bytes()[4 * i..4 * i + 4].try_into().unwrap()) as u64;
            // Each pixel costs 8192 per crossing on its row plus 8192 for accumulation, and the
            // blend copies it once.
            let pixel: Vec<u64> = (0..count).map(|r| 8192 * (1 + offset(8 * (r + 1)) - offset(8 * r)) + 1).collect();
            let draw = Draw::Spans { table: self.tables.len(), origin: [ox, oy + row as i32, width as i32, count as i32] };
            self.tables.push(table);
            let mut band = |x0: usize, x1: usize, r0: usize, r1: usize| {
                let rect = [ox + x0 as i32, oy + (row + r0) as i32, ox + x1 as i32, oy + (row + r1) as i32];
                let cost = (x1 - x0) as u64 * pixel[r0..r1].iter().sum::<u64>();
                self.steps.push(Step { dest, rect, draw, cost });
            };
            let mut r0 = 0;
            while r0 < count {
                let row_cost = width as u64 * pixel[r0];
                if row_cost > budget {
                    // A row can itself exceed the budget: split its rectangle in x; the
                    // parameters keep the glyph's width, so accumulation stays exact.
                    let columns = ((budget / pixel[r0]) as usize).max(1);
                    for x in (0..width).step_by(columns) { band(x, (x + columns).min(width), r0, r0 + 1); }
                    r0 += 1;
                    continue;
                }
                let (mut r1, mut cost) = (r0, 0);
                while r1 < count && cost + width as u64 * pixel[r1] <= budget {
                    cost += width as u64 * pixel[r1];
                    r1 += 1;
                }
                band(0, width, r0, r1);
                r0 = r1;
            }
        }
    }

    /// Draw `rect` of `region`'s target in bands with an estimate of at most half the work
    /// cap at `weight` steps per pixel. The instance parameters name origins, so each band
    /// draws the pixels the whole rectangle would there.
    fn banded(&mut self, region: &Region, rect: [i32; 4], weight: u64, draw: Draw) {
        let budget = abi::limits::WORK / 2;
        let dest = self.dest(&region.target);
        let width = (rect[2] - rect[0]).max(1) as u64;
        let columns = ((budget / weight) as i32).max(1).min(width as i32);
        let rows = ((budget / (weight * width)) as i32).max(1);
        for y in (rect[1]..rect[3]).step_by(rows as usize) {
            for x in (rect[0]..rect[2]).step_by(columns as usize) {
                let band = [x, y, (x + columns).min(rect[2]), (y + rows).min(rect[3])];
                let cost = (band[2] - band[0]) as u64 * (band[3] - band[1]) as u64 * weight;
                self.steps.push(Step { dest, rect: band, draw, cost });
            }
        }
    }

    /// LCD uses the exact same binary32 operation order at three columns per pixel.
    /// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
    /// Downsample each column class on the GPU, zero its other channels with a constant
    /// channel mask, then use it as an LCD coverage mask to assemble the packed glyph. Each
    /// stage is a fresh scratch region of target 0 or 1; a stage samples only regions of the
    /// other target. Returns the packed glyph's region.
    fn lcd(&mut self, segments: &[LineSegment], width: usize, height: usize) -> Result<Region, Error> {
        if libfont::bounds::raster_bound(segments.len(), height, width * 3) > libfont::bounds::MAX_RASTER_STEPS {
            return Err(FontError::GlyphTooExpensive);
        }
        let scaled = libfont::crossings::lcd_segments(segments);
        admit(&scaled, width * 3, height)?;
        let wide = region(0, width * 3, height)?;
        self.spans(&scaled, width * 3, height, &wide);
        let sample = region(0, width, height)?;
        // Padding permits the last column-class sampling rectangle to remain in bounds.
        let padded = region(1, width * 3 + 2, height)?;
        let mask = region(1, width, height)?;
        let packed = region(1, width, height)?;
        let (w, h) = (width as i32, height as i32);
        let at = |r: &Region| [r.x as i32, r.y as i32];
        let rect = |r: &Region, rw: i32| [r.x as i32, r.y as i32, r.x as i32 + rw, r.y as i32 + h];
        let params = |values: &[i32]| { let mut p = [0; 8]; p[..values.len()].copy_from_slice(values); p };
        // Per-pixel weights in gpu-abi.md: solid and copy 1, nearest_scaled 8192, coverage 256
        // plus its blend copy.
        self.banded(&packed, rect(&packed, w), 1, Draw::Solid(word(Color::BLACK)));
        let [px, py] = at(&padded);
        let [wx, wy] = at(&wide);
        // The copy writes wide's columns with output X forced opaque, as blit does.
        self.banded(&padded, rect(&padded, 3 * w), 1, Draw::Sampled { pipeline: pipeline::COPY, colour: 0,
            texture: wide.target.handle(), params: params(&[wx - px, wy - py, 1]) });
        let [sx, sy] = at(&sample);
        let [mx, my] = at(&mask);
        let [kx, ky] = at(&packed);
        for channel in 0..3 {
            self.banded(&sample, rect(&sample, w), 8192, Draw::Sampled { pipeline: pipeline::NEAREST_SCALED, colour: 0,
                texture: padded.target.handle(), params: params(&[sx, sy, w, h, px + channel, py, 3 * w, h]) });
            let colour = match channel { 0 => Color::rgb(0,255,255), 1 => Color::rgb(255,0,255), _ => Color::rgb(255,255,0) };
            self.banded(&mask, rect(&mask, w), 1, Draw::Solid(word(colour)));
            self.banded(&sample, rect(&sample, w), 257, Draw::Sampled { pipeline: pipeline::COVERAGE, colour: 0xff000000,
                texture: mask.target.handle(), params: params(&[sx, sy, mx, my, 1]) });
            self.banded(&packed, rect(&packed, w), 257, Draw::Sampled { pipeline: pipeline::COVERAGE, colour: 0xffffffff,
                texture: sample.target.handle(), params: params(&[kx, ky, sx, sy, 1]) });
        }
        Ok(packed)
    }

    /// Send the planned draws, each submission the longest run of them whose estimates sum
    /// within half the work cap and whose tables fit `TABLE_BYTES`, its tables uploaded once
    /// into the geometry buffer. Returns the last submission's fence.
    fn send(self) -> Result<Option<bgl::Fence>, Error> {
        if self.steps.is_empty() { return Ok(None); }
        let budget = abi::limits::WORK / 2;
        let _held = scratch_lock()?;
        device::with(|d| {
            let mut fence = None;
            // Each table's byte offset in the submission being built.
            let mut placed: Vec<Option<usize>> = vec![None; self.tables.len()];
            let mut start = 0;
            while start < self.steps.len() {
                let (mut end, mut work, mut bytes, mut members) = (start, 0u64, 0usize, Vec::new());
                while let Some(s) = self.steps.get(end) {
                    let new = match s.draw { Draw::Spans { table, .. } if placed[table].is_none() => Some(table), _ => None };
                    let more = new.map_or(0, |t| self.tables[t].bytes().len());
                    if end > start && (work + s.cost > budget || bytes + more > TABLE_BYTES) { break; }
                    if let Some(t) = new { placed[t] = Some(bytes); members.push(t); }
                    (work, bytes, end) = (work + s.cost, bytes + more, end + 1);
                }
                let sent = (|| {
                    let buffer = if bytes > 0 { geometry_buffer(d.context.handle(), bytes as u64)? } else { 0 };
                    for s in &self.steps[start..end] {
                        d.context.begin(s.dest.handle, s.dest.width, s.dest.height);
                        match s.draw {
                            Draw::Spans { table, origin: [x, y, w, h] } => d.context.draw(pipeline::COVERAGE_SPANS, 0xffffffff, 0,
                                buffer, bytes as u32, &[instance(s.rect, &[x, y, w, h, placed[table].expect("placed above") as i32])]),
                            Draw::Solid(colour) => d.context.solid(colour, &[s.rect]),
                            Draw::Sampled { pipeline, colour, texture, params } =>
                                d.context.draw(pipeline, colour, texture, 0, 0, &[instance(s.rect, &params)]),
                        }
                    }
                    if bytes > 0 {
                        let tables: Vec<_> = members.iter().map(|&t| &self.tables[t]).collect();
                        bgl::geometry_upload(CrossingTable::pack(&tables), &d.context, buffer)?;
                        d.declared += bytes as u64;
                    }
                    d.context.submit()
                })();
                match sent {
                    Ok(f) => fence = Some(f),
                    Err(e) => { d.context.discard(); return Err(e); }
                }
                for t in members { placed[t] = None; }
                start = end;
            }
            Ok(fence)
        }).map_err(Error::Device)
    }
}

fn target(width: usize, height: usize) -> Result<Surface, Error> {
    Surface::storage(width, height, Format::BGRX).map_err(|e| match e {
        crate::surface::SurfaceError::Device(e) => Error::Device(e), _ => Error::Device(-abi::errno::EINVAL),
    })
}
fn instance(rect: [i32; 4], values: &[i32]) -> Instance {
    let mut params = [0; 16];
    for (p, &v) in params.iter_mut().zip(values) { *p = v as u32; }
    Instance { rect, params }
}
/// The original total bound, admitted before any crossings or device work.
fn admit(segments: &[LineSegment], width: usize, height: usize) -> Result<(), Error> {
    use libfont::bounds::{raster_bound, MAX_RASTER_STEPS};
    if raster_bound(segments.len(), height, width) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive); }
    Ok(())
}

/// The bytes one crossing table may hold, and the tables one submission names; a taller
/// glyph's rows are split across tables.
const TABLE_BYTES: usize = 8 << 20;

/// The geometry buffer, charged by the core and replaced on context recovery. SUBMIT
/// snapshots parameter bytes before it returns, so a later submission's tables can
/// overwrite it while earlier draws consume their immutable snapshots. No coverage pixels
/// are cached.
struct Geometry { context: u64, handle: u64, bytes: u64 }
impl Drop for Geometry { fn drop(&mut self) { gpu::resource_destroy(self.handle); } }

/// The caller holds SCRATCH's owner lock and the process's drawing context lock.
fn geometry_buffer(context: u64, bytes: u64) -> Result<u64, i64> {
    // SAFETY: send holds the PID-tagged scratch lock through upload and SUBMIT.
    let slot = unsafe { &mut *SCRATCH.geometry.get() };
    if slot.as_ref().is_some_and(|g| g.context != context) { *slot = None; }
    if slot.as_ref().is_none_or(|g| g.bytes < bytes) {
        let capacity = bytes.max(4096).next_power_of_two();
        let handle = gpu::buffer_create(capacity)?;
        *slot = Some(Geometry { context, handle, bytes: capacity });
    }
    Ok(slot.as_ref().expect("geometry created above").handle)
}

/// A grayscale rasterization whose submissions are sent; `read` reads its coverage back.
pub struct SentGlyph(AtlasGlyph);
impl SentGlyph {
    pub fn read(self) -> Result<GlyphBitmap, Error> { self.0.read() }
}
/// An LCD rasterization whose submissions are sent; `read` reads its coverage back.
pub struct SentSubpixel(AtlasGlyph);
impl SentSubpixel {
    pub fn read(self) -> Result<SubpixelBitmap, Error> { self.0.subpixel() }
}

/// A glyph's coverage: its `width × height` region of a target, absent for an empty glyph.
pub struct AtlasGlyph {
    region: Option<Region>,
    pub width: usize, pub height: usize, pub x_offset: i32, pub y_offset: i32,
}
impl AtlasGlyph {
    fn empty(advance: usize) -> Self {
        AtlasGlyph { region: None, width: advance, height: 0, x_offset: 0, y_offset: 0 }
    }
    fn at(region: Region, width: usize, height: usize) -> Self {
        AtlasGlyph { region: Some(region), width, height, x_offset: 0, y_offset: 0 }
    }
    /// READBACK waits for the submissions that write the target, and reports one that
    /// failed as EIO, so reading back is the only wait.
    fn pixels(&self) -> Result<Vec<u8>, Error> {
        let mut pixels = vec![0; self.width * self.height * 4];
        if let Some(r) = &self.region {
            r.target.read_rect([r.x, r.y, self.width, self.height], &mut pixels, self.width * 4).map_err(Error::Device)?;
        }
        Ok(pixels)
    }
    fn read(&self) -> Result<GlyphBitmap, Error> {
        let coverage = self.pixels()?.chunks_exact(4).map(|p| p[0]).collect();
        Ok(GlyphBitmap { width: self.width, height: self.height, x_offset: self.x_offset, y_offset: self.y_offset, coverage })
    }
    fn subpixel(&self) -> Result<SubpixelBitmap, Error> {
        let coverage = self.pixels()?.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
        Ok(SubpixelBitmap { width: self.width, height: self.height, x_offset: self.x_offset, y_offset: self.y_offset, coverage })
    }
}

/// The kernel's charge for a `width × height` four-byte target: rows padded to 256 bytes,
/// the whole rounded up to 4 KiB pages (B23).
fn charge(width: usize, height: usize) -> usize { ((width * 4 + 255) / 256 * 256 * height + 4095) / 4096 * 4096 }

/// One atlas page: a target of equal slots stacked vertically, each `rows` tall at the
/// page's row pitch. Its first page holds one slot, so a single cached glyph's storage is
/// exactly its own target's charge; each new page of a class holds as many slots as the
/// class has, so pages, and resource handles, grow with the log of the entries.
struct Page { id: u64, pitch: usize, rows: usize, target: Arc<Surface>, used: Vec<bool>, drawn: Vec<bool> }
impl Page {
    fn bytes(&self) -> usize { charge(self.pitch / 4, self.rows * self.used.len()) }
}
/// A glyph's slot class: its row pitch, and its height rounded up to whole 4 KiB pages of
/// that pitch when the pitch divides a page, so a one-slot page costs the glyph's charge.
fn class(width: usize, height: usize) -> (usize, usize) {
    let pitch = (width * 4 + 255) / 256 * 256;
    let rows = if 4096 % pitch == 0 { let r = 4096 / pitch; height.div_ceil(r) * r } else { height };
    (pitch, rows)
}

struct Entry { glyph: u16, size26: i32, image: Arc<AtlasGlyph>, used: u64, slot: Option<(u64, usize)> }

/// The glyphs of one draw the atlas is rasterizing: their coverage, the images to draw in
/// order with their pen offsets once it is sent, and the slots it fills. Entries used at
/// or after `first` are drawn from this batch, so they stay in the atlas until it is sent.
#[derive(Default)]
struct Batch { run: Run, draws: Vec<(Arc<AtlasGlyph>, i64)>, slots: Vec<(u64, usize)>, first: u64 }

/// `bytes` is the sum of the atlas pages' charges, unused slots included (B15, B23).
pub struct CachedFont {
    font: Font, entries: Vec<Entry>, pages: Vec<Page>, next_page: u64, capacity: usize, max_bytes: usize, clock: u64,
    stats: CacheStats, readback: Option<GlyphBitmap>, owner: u64,
}
impl CachedFont {
    pub fn new(font: Font, capacity: usize) -> Self {
        Self { font, entries: Vec::new(), pages: Vec::new(), next_page: 0, capacity: capacity.max(1), max_bytes: usize::MAX,
            clock: 0, stats: CacheStats::default(), readback: None, owner: device::pid() }
    }
    pub fn with_default_cache(font: Font) -> Self {
        let mut f = Self::new(font, libfont::cache::DEFAULT_ENTRIES); f.max_bytes = libfont::cache::DEFAULT_BYTES; f
    }
    pub fn font(&self) -> &Font { &self.font }
    pub fn metrics(&self, size: f32) -> libfont::ScaledMetrics { self.font.metrics(size) }
    pub fn glyph_index(&self, ch: char) -> u16 { self.font.glyph_index(ch) }
    pub fn advance_width(&self, g: u16, size: f32) -> f32 { self.font.advance_width(g, size) }
    pub fn kern(&self, l: u16, r: u16, size: f32) -> f32 { self.font.kern(l,r,size) }

    /// The atlas image of `glyph` at `size26`, rasterized on a miss.
    fn glyph(&mut self, glyph: u16, size26: i32) -> Result<Arc<AtlasGlyph>, Error> {
        let mut image = Err(Error::Device(-abi::errno::EIO));
        self.run(&[(glyph, 0)], size26, |r| image = r.map(|(g, _)| g));
        image
    }

    /// Give `out` each glyph of `placed` (glyph, pen offset) at `size26` from the atlas, in
    /// order, or its error. The glyphs the atlas lacks are rasterized into it in one run,
    /// waited for once (B33); only a draw needing more glyphs than the atlas holds sends
    /// the run early, when an entry it draws would otherwise be evicted.
    fn run(&mut self, placed: &[(u16, i64)], size26: i32, mut out: impl FnMut(Result<(Arc<AtlasGlyph>, i64), Error>)) {
        if self.owner != device::pid() { self.clear(); self.owner = device::pid(); }
        let mut batch = Batch { first: self.clock + 1, ..Batch::default() };
        for &(glyph, dx) in placed {
            self.clock += 1;
            if let Some(e) = self.entries.iter_mut().find(|e| e.glyph == glyph && e.size26 == size26) {
                self.stats.hits += 1;
                e.used = self.clock;
                batch.draws.push((e.image.clone(), dx));
                continue;
            }
            self.stats.misses += 1;
            match self.queue(glyph, size26, &mut batch, &mut out) {
                Ok(image) => batch.draws.push((image, dx)),
                Err(e) => out(Err(e)),
            }
        }
        self.finish(&mut batch, &mut out);
    }

    /// Plan a missing glyph's coverage on `batch`, giving the image it will hold.
    fn queue(&mut self, glyph: u16, size26: i32, batch: &mut Batch,
        out: &mut impl FnMut(Result<(Arc<AtlasGlyph>, i64), Error>)) -> Result<Arc<AtlasGlyph>, Error> {
        let outline = self.font.glyph_outline(glyph)?;
        let (segments, width, height, x_offset, y_offset) = match self.font.layout_glyph(&outline, glyph, size26)? {
            GlyphLayout::Empty { advance } => {
                while !self.make_room(batch.first) { self.finish(batch, out); }
                return Ok(self.insert(glyph, size26, AtlasGlyph::empty(advance), None));
            }
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => (segments, width, height, x_offset, y_offset),
        };
        admit(&segments, width, height)?;
        let (pitch, rows) = class(width, height);
        if charge(pitch / 4, rows) > self.max_bytes {
            // Larger than the whole budget: rasterized, not kept.
            let region = region(0, width, height)?;
            batch.run.spans(&segments, width, height, &region);
            let mut image = AtlasGlyph::at(region, width, height);
            (image.x_offset, image.y_offset) = (x_offset, y_offset);
            return Ok(Arc::new(image));
        }
        while !self.make_room(batch.first) { self.finish(batch, out); }
        let (page, slot) = loop {
            match self.slot(pitch, rows, batch.first)? {
                Some(s) => break s,
                None => self.finish(batch, out),
            }
        };
        let p = self.pages.iter_mut().find(|p| p.id == page).expect("slot's page is live");
        p.used[slot] = true;
        let region = Region { target: p.target.clone(), x: 0, y: slot * rows };
        // A slot drawn before is zeroed again first, so it is as a new target is.
        if core::mem::replace(&mut p.drawn[slot], true) {
            batch.run.banded(&region, [0, region.y as i32, width as i32, (region.y + height) as i32], 1, Draw::Solid(0));
        }
        batch.run.spans(&segments, width, height, &region);
        let mut image = AtlasGlyph::at(region, width, height);
        (image.x_offset, image.y_offset) = (x_offset, y_offset);
        batch.slots.push((page, slot));
        Ok(self.insert(glyph, size26, image, Some((page, slot))))
    }

    /// Send `batch`'s run, wait for it once, and give its images to `out` in order. A
    /// failed run's glyphs leave the atlas. The batch is then empty, and the entries it
    /// drew may be evicted.
    fn finish(&mut self, batch: &mut Batch, out: &mut impl FnMut(Result<(Arc<AtlasGlyph>, i64), Error>)) {
        let run = core::mem::take(&mut batch.run);
        let draws = core::mem::take(&mut batch.draws);
        let slots = core::mem::take(&mut batch.slots);
        batch.first = self.clock;
        let sent = run.send().and_then(|f| f.map_or(Ok(()), |f| f.wait(u64::MAX).map_err(Error::Device)));
        match sent {
            Ok(()) => for d in draws { out(Ok(d)); },
            Err(e) => {
                self.entries.retain(|e| e.slot.is_none_or(|s| !slots.contains(&s)));
                self.stats.entries = self.entries.len();
                for (page, slot) in slots { self.free(page, slot); }
                out(Err(e));
            }
        }
    }

    fn insert(&mut self, glyph: u16, size26: i32, image: AtlasGlyph, slot: Option<(u64, usize)>) -> Arc<AtlasGlyph> {
        let image = Arc::new(image);
        self.entries.push(Entry { glyph, size26, image: image.clone(), used: self.clock, slot });
        self.stats.entries = self.entries.len();
        image
    }
    /// Evict until an entry fits the entry budget; false when the next to evict is one
    /// used at or after `first`.
    fn make_room(&mut self, first: u64) -> bool {
        while self.entries.len() >= self.capacity {
            if !self.evict(first) { return false; }
        }
        true
    }
    /// A free slot of class (pitch, rows), evicting least recently used entries until a
    /// page with one fits the byte budget; or asks the caller to send its batch first,
    /// when the next to evict was used at or after `first`.
    fn slot(&mut self, pitch: usize, rows: usize, first: u64) -> Result<Option<(u64, usize)>, Error> {
        loop {
            if let Some((id, slot)) = self.pages.iter().filter(|p| (p.pitch, p.rows) == (pitch, rows))
                .find_map(|p| p.used.iter().position(|u| !u).map(|s| (p.id, s))) {
                return Ok(Some((id, slot)));
            }
            let held: usize = self.pages.iter().filter(|p| (p.pitch, p.rows) == (pitch, rows)).map(|p| p.used.len()).sum();
            let mut slots = held.max(1).min(MAX_PAGE_ROWS / rows).max(1);
            while slots > 1 && self.stats.bytes + charge(pitch / 4, rows * slots) > self.max_bytes { slots /= 2; }
            if self.stats.bytes + charge(pitch / 4, rows * slots) <= self.max_bytes {
                let target = Arc::new(target(pitch / 4, rows * slots)?);
                let page = Page { id: self.next_page, pitch, rows, target, used: vec![false; slots], drawn: vec![false; slots] };
                self.next_page += 1;
                self.stats.bytes += page.bytes();
                self.pages.push(page);
                continue;
            }
            if self.entries.is_empty() { return Err(Error::Device(-abi::errno::ENOMEM)); }
            if !self.evict(first) { return Ok(None); }
        }
    }
    /// Evict the least recently used entry, unless it was used at or after `first`.
    fn evict(&mut self, first: u64) -> bool {
        let Some((oldest, used)) = self.entries.iter().enumerate().map(|(i, e)| (i, e.used)).min_by_key(|&(_, u)| u) else { return false };
        if used >= first { return false; }
        if let Some((page, slot)) = self.entries.swap_remove(oldest).slot { self.free(page, slot); }
        self.stats.entries = self.entries.len();
        true
    }
    /// Release a slot; a page with no glyph left is freed with its storage.
    fn free(&mut self, page: u64, slot: usize) {
        let Some(i) = self.pages.iter().position(|p| p.id == page) else { return };
        self.pages[i].used[slot] = false;
        if self.pages[i].used.iter().all(|u| !u) {
            self.stats.bytes -= self.pages.swap_remove(i).bytes();
        }
    }
    /// Explicit readback for checks and capture; drawing samples the atlas directly.
    pub fn rasterize_glyph(&mut self, glyph: u16, size: f32) -> Result<&GlyphBitmap, Error> {
        let image = self.glyph(glyph, Font::size26(size)?)?;
        self.readback = Some(image.read()?); Ok(self.readback.as_ref().unwrap())
    }
    pub fn cache_stats(&self) -> CacheStats { self.stats }
    pub fn clear(&mut self) {
        self.entries.clear(); self.pages.clear(); self.readback = None; self.stats.entries = 0; self.stats.bytes = 0;
    }
    pub fn clear_cache(&mut self) { self.clear(); }
}
/// The tallest page a slot class may have: the profile's target dimension cap.
const MAX_PAGE_ROWS: usize = crate::surface::MAX_DIMENSION;

/// GPU raster entry points with the same readback shape as libfont's CPU reference.
pub mod rasterizer {
    use super::*;
    pub use libfont::bounds::{raster_bound, MAX_RASTER_STEPS};
    pub fn rasterize(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<GlyphBitmap, Error> {
        send(s, w, h, x, y)?.read()
    }
    /// `rasterize`, sent to the GPU and read back later by `SentGlyph::read`.
    pub fn send(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<SentGlyph, Error> {
        if raster_bound(s.len(), h, w) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let mut glyph = if w == 0 || h == 0 { AtlasGlyph::empty(0) } else {
            let region = region(0, w, h)?;
            let mut run = Run::default();
            run.spans(s, w, h, &region);
            run.send()?;
            AtlasGlyph::at(region, w, h)
        };
        (glyph.width, glyph.height, glyph.x_offset, glyph.y_offset) = (w, h, x, y);
        Ok(SentGlyph(glyph))
    }
    pub fn rasterize_subpixel(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<SubpixelBitmap, Error> {
        send_subpixel(s, w, h, x, y)?.read()
    }
    /// `rasterize_subpixel`, sent to the GPU and read back later by `SentSubpixel::read`.
    pub fn send_subpixel(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<SentSubpixel, Error> {
        if raster_bound(s.len(), h, w*3) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let mut glyph = if w == 0 || h == 0 { AtlasGlyph::empty(0) } else {
            let mut run = Run::default();
            let packed = run.lcd(s, w, h)?;
            run.send()?;
            AtlasGlyph::at(packed, w, h)
        };
        (glyph.width, glyph.height, glyph.x_offset, glyph.y_offset) = (w, h, x, y);
        Ok(SentSubpixel(glyph))
    }
}
/// U+FFFD REPLACEMENT CHARACTER.
pub const REPLACEMENT: u32 = 0xFFFD;

/// Decode UTF-8, giving one U+FFFD for each maximal invalid subpart (Unicode §3.9,
/// "U+FFFD Substitution of Maximal Subparts", over the well-formed sequences of its
/// Table 3-7).
pub fn decode_utf8(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let mut i = 0;
    core::iter::from_fn(move || {
        if i >= bytes.len() {
            return None;
        }
        work::add(1);
        let b0 = bytes[i];
        // (sequence length, the range of the second byte) for each valid lead byte.
        let (len, lo, hi) = match b0 {
            0x00..=0x7F => {
                i += 1;
                return Some(b0 as u32);
            }
            0xC2..=0xDF => (2, 0x80, 0xBF),
            0xE0 => (3, 0xA0, 0xBF),
            0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
            0xED => (3, 0x80, 0x9F),
            0xF0 => (4, 0x90, 0xBF),
            0xF1..=0xF3 => (4, 0x80, 0xBF),
            0xF4 => (4, 0x80, 0x8F),
            _ => {
                i += 1;
                return Some(REPLACEMENT);
            }
        };
        let mut cp = (b0 as u32) & (0x7F >> len);
        for k in 1..len {
            let (lo, hi) = if k == 1 { (lo, hi) } else { (0x80, 0xBF) };
            match bytes.get(i + k) {
                Some(&b) if (lo..=hi).contains(&b) => cp = (cp << 6) | (b & 0x3F) as u32,
                _ => {
                    // The maximal subpart is the lead and the k − 1 bytes that fit.
                    i += k;
                    return Some(REPLACEMENT);
                }
            }
        }
        i += len;
        Some(cp)
    })
}

/// `floor((pen + 32) / 64)`: a 26.6 pen position rounded once to whole pixels.
fn pixels(pen: i64) -> i64 {
    (pen + 32).div_euclid(64)
}

/// Lay out `glyphs` from pen 0 at `size`, calling `draw(font, glyph, x)` with each glyph's
/// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
/// pixel origin offset; returns the width, `floor((pen_end + 32) / 64)`. A size outside
/// 1 to 1,024 lays out nothing.
fn layout(font: &mut CachedFont, glyphs: impl Iterator<Item = u16>, size: f32,
    mut draw: impl FnMut(&mut CachedFont, u16, i64)) -> i32 {
    let Ok(size26) = Font::size26(size) else { return 0 };
    let (mut pen, mut prev) = (0i64, None);
    for g in glyphs {
        work::add(1);
        if let Some(p) = prev {
            pen += font.font().kern26(p, g, size26) as i64;
        }
        draw(font, g, pixels(pen));
        pen += font.font().advance26(g, size26) as i64;
        prev = Some(g);
    }
    pixels(pen) as i32
}

fn glyphs_of<'a>(font: &'a CachedFont, cps: impl Iterator<Item = u32> + 'a) -> impl Iterator<Item = u16> + 'a {
    cps.map(move |cp| font.font().glyph_for(cp))
}

/// Whether glyph `g` at `size26` with its pen at (x, y) can draw inside the clip. A glyph
/// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
/// without contours, or whose record is bad, draws nothing, so it cannot.
fn reaches_clip(fb: &Surface, font: &CachedFont, g: u16, size26: i32, x: i64, y: i64) -> bool {
    match font.font().glyph_box(g, size26) {
        Ok(Some(b)) => {
            let (x0, y0) = (x + b.x_offset as i64, y + b.y_offset as i64);
            let rect = Box64 { x0, y0, x1: x0 + b.width as i64, y1: y0 + b.height as i64 };
            !rect.intersect(&fb.clip_box()).is_empty()
        }
        _ => false,
    }
}

/// The glyphs of `glyphs` laid out at `size` whose rectangles reach `fb`'s clip with the pen
/// at (x, y), with their pen offsets, and the run's width.
fn visible(fb: &Surface, font: &mut CachedFont, glyphs: Vec<u16>, x: i32, y: i32, size: f32) -> (Vec<(u16, i64)>, i32) {
    let size26 = Font::size26(size).unwrap_or(0);
    let mut placed = Vec::new();
    let width = layout(font, glyphs.into_iter(), size, |font, g, dx| {
        if reaches_clip(fb, font, g, size26, x as i64 + dx, y as i64) { placed.push((g, dx)); }
    });
    (placed, width)
}

fn draw_glyphs(fb: &mut Surface, font: &mut CachedFont, glyphs: Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let (placed, width) = visible(fb, font, glyphs, x, y, size);
    font.run(&placed, Font::size26(size).unwrap_or(0), |r| match r {
        Ok((image, dx)) => draw_atlas(fb, &image, x as i64 + dx, y as i64, fg),
        Err(Error::Device(e)) => fb.fail(e),
        Err(_) => {},
    });
    width
}

/// Draw one character (glyph 0 when unmapped); returns its advance in whole pixels.
pub fn draw_char(fb: &mut Surface, font: &mut CachedFont, ch: char, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let g = font.glyph_index(ch);
    draw_glyphs(fb, font, alloc::vec![g], x, y, size, fg)
}

/// Draw `text` with its pen starting at (x, y); returns the width section 8.3 gives.
pub fn draw_text(fb: &mut Surface, font: &mut CachedFont, text: &str, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    draw_glyphs(fb, font, glyphs, x, y, size, fg)
}

/// Draw UTF-8 `bytes`, one U+FFFD for each maximal invalid subpart; never panics.
/// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
pub fn draw_text_bytes(fb: &mut Surface, font: &mut CachedFont, bytes: &[u8], x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, decode_utf8(bytes)).collect();
    draw_glyphs(fb, font, glyphs, x, y, size, fg)
}

/// One run of text for [`draw_runs`]: UTF-8 `bytes` with the pen at (x, y), at `size`, in
/// `fg`, under `clip`.
pub struct TextRun<'a> {
    pub bytes: &'a [u8],
    pub x: i32,
    pub y: i32,
    pub size: f32,
    pub fg: Color,
    pub clip: crate::surface::Rect,
}

/// Draw `runs`, each as `draw_text_bytes` draws it under its clip, for runs whose glyph boxes
/// do not meet one another's (the run stage's text): a glyph goes in the layer after the last
/// earlier glyph of its run that it meets, and a layer's glyphs share draws, by atlas page
/// and colour, so glyphs that meet still blend in their run's order and no draw holds two
/// glyphs that meet. When gathering the runs' glyphs evicted an atlas entry, an earlier run's
/// glyph may name a reused slot, so the runs are drawn one by one instead.
pub fn draw_runs(fb: &mut Surface, font: &mut CachedFont, runs: &[TextRun<'_>]) {
    type Placed = (Arc<AtlasGlyph>, i64, i64, Box64, Color);
    let before = font.cache_stats();
    let mut layers: Vec<Vec<Placed>> = Vec::new();
    for r in runs {
        fb.push_clip(r.clip);
        let glyphs = glyphs_of(font, decode_utf8(r.bytes)).collect();
        let (placed, _) = visible(fb, font, glyphs, r.x, r.y, r.size);
        let clip = fb.clip_box();
        // Each earlier glyph of this run that draws: its box and layer.
        let mut drawn: Vec<(Box64, usize)> = Vec::new();
        let mut failed = None;
        font.run(&placed, Font::size26(r.size).unwrap_or(0), |got| match got {
            Ok((image, dx)) => {
                let (x, y) = (r.x as i64 + dx, r.y as i64);
                let b = Box64::wide(x + image.x_offset as i64, y + image.y_offset as i64, image.width as i64, image.height as i64).intersect(&clip);
                let layer = drawn.iter().filter(|(o, _)| !o.intersect(&b).is_empty()).map(|(_, l)| l + 1).max().unwrap_or(0);
                if !b.is_empty() {
                    drawn.push((b, layer));
                }
                if layers.len() <= layer {
                    layers.resize_with(layer + 1, Vec::new);
                }
                layers[layer].push((image, x, y, clip, r.fg));
            }
            Err(Error::Device(e)) => failed = Some(e),
            Err(_) => {}
        });
        if let Some(e) = failed {
            fb.fail(e);
        }
        fb.pop_clip();
    }
    let after = font.cache_stats();
    if before.entries + (after.misses - before.misses) as usize > after.entries {
        for r in runs {
            fb.push_clip(r.clip);
            draw_text_bytes(fb, font, r.bytes, r.x, r.y, r.size, r.fg);
            fb.pop_clip();
        }
        return;
    }
    for layer in &layers {
        // One draw per atlas page and colour, each within half the work cap.
        let mut done = vec![false; layer.len()];
        for first in 0..layer.len() {
            if done[first] {
                continue;
            }
            let Some(page) = layer[first].0.region.as_ref().map(|r| r.target.handle()) else {
                done[first] = true;
                continue;
            };
            let fg = layer[first].4;
            let mut instances = Vec::new();
            let mut work = 0u64;
            for (i, (image, x, y, clip, colour)) in layer.iter().enumerate().skip(first) {
                let Some(atlas) = &image.region else {
                    done[i] = true;
                    continue;
                };
                if done[i] || atlas.target.handle() != page || *colour != fg {
                    continue;
                }
                let whole = Box64::wide(x + image.x_offset as i64, y + image.y_offset as i64, image.width as i64, image.height as i64);
                let visible = whole.intersect(clip);
                done[i] = true;
                if visible.is_empty() {
                    continue;
                }
                let [x0, y0, x1, y1] = visible.corners();
                let cost = 257 * ((x1 - x0) as u64) * ((y1 - y0) as u64);
                if !instances.is_empty() && work + cost > abi::limits::WORK / 2 {
                    let batch = core::mem::take(&mut instances);
                    fb.submit(|c| c.draw(pipeline::COVERAGE, word(fg), page, 0, 0, &batch));
                    work = 0;
                }
                work::add(1);
                let params = [x0, y0, atlas.x as i32 + (visible.x0 - whole.x0) as i32, atlas.y as i32 + (visible.y0 - whole.y0) as i32, 1];
                instances.push(instance([x0, y0, x1, y1], &params));
                work += cost;
                fb.mark(whole);
            }
            if !instances.is_empty() {
                fb.submit(|c| c.draw(pipeline::COVERAGE, word(fg), page, 0, 0, &instances));
            }
        }
    }
}

/// The width `draw_text` returns, without drawing.
pub fn text_width(font: &mut CachedFont, text: &str, size: f32) -> i32 {
    let glyphs: Vec<u16> = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

/// The width `draw_text_bytes` returns, without drawing.
pub fn text_width_bytes(font: &mut CachedFont, bytes: &[u8], size: f32) -> i32 {
    let glyphs: Vec<u16> = glyphs_of(font, decode_utf8(bytes)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

/// LCD glyphs are not cached: the run's glyphs are rasterized into scratch in one run and
/// drawn after it in the same context, which runs them in order, so drawing does not wait.
fn draw_glyphs_subpixel(fb: &mut Surface, font: &mut CachedFont, glyphs: Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let (placed, width) = visible(fb, font, glyphs, x, y, size);
    let size26 = Font::size26(size).unwrap_or(0);
    let mut run = Run::default();
    let mut draws = Vec::new();
    for (g, dx) in placed {
        match font.font().plan(g, size26, true, &mut run) {
            Ok(image) => draws.push((image, dx)),
            Err(Error::Device(e)) => fb.fail(e),
            Err(_) => {},
        }
    }
    match run.send() {
        Ok(_) => for (image, dx) in draws { draw_atlas(fb, &image, x as i64 + dx, y as i64, fg); },
        Err(Error::Device(e)) => fb.fail(e),
        Err(_) => {},
    }
    width
}

/// Draw one character with LCD subpixel rendering; returns its advance in whole pixels.
pub fn draw_char_subpixel(fb: &mut Surface, font: &mut CachedFont, ch: char, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let g = font.glyph_index(ch);
    draw_glyphs_subpixel(fb, font, alloc::vec![g], x, y, size, fg)
}

/// Draw `text` with LCD subpixel rendering; returns its width.
pub fn draw_text_subpixel(fb: &mut Surface, font: &mut CachedFont, text: &str, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let glyphs = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    draw_glyphs_subpixel(fb, font, glyphs, x, y, size, fg)
}

fn draw_atlas(fb: &mut Surface, image: &AtlasGlyph, x: i64, y: i64, fg: Color) {
    let Some(atlas) = &image.region else { return };
    let whole = Box64::wide(x + image.x_offset as i64, y + image.y_offset as i64, image.width as i64, image.height as i64);
    let visible = whole.intersect(&fb.clip_box());
    if visible.is_empty() { return; }
    work::add(1);
    let params = [visible.x0 as i32, visible.y0 as i32,
        atlas.x as i32 + (visible.x0 - whole.x0) as i32, atlas.y as i32 + (visible.y0 - whole.y0) as i32, 1];
    // coverage costs 256 per pixel plus its blend copy: a glyph too large for half the work
    // cap is drawn in row bands, the same pixels, since the parameters name origins.
    let [x0, y0, x1, y1] = visible.corners();
    let rows = ((abi::limits::WORK / 2 / (257 * (x1 - x0) as u64)) as i32).max(1);
    for y in (y0..y1).step_by(rows as usize) {
        let i = instance([x0, y, x1, (y + rows).min(y1)], &params);
        fb.submit(|c| c.draw(pipeline::COVERAGE, word(fg), atlas.target.handle(), 0, 0, &[i]));
    }
    fb.mark(whole);
}
