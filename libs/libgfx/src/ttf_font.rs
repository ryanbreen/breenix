//! TrueType geometry on the CPU, coverage and glyph storage on the GPU.
//! Glyphs live in B8G8R8A8 targets (B15/B19): a cached glyph in a slot of an atlas page,
//! an uncached one in a never-used region of a scratch target.
use alloc::{sync::Arc, vec, vec::Vec};
use core::cell::UnsafeCell;
use core::ops::Deref;
use core::sync::atomic::{AtomicU64, Ordering};
use bgl::{Instance, pipeline};
use libbreenix::gpu::{self, abi};
use libfont::{FontError, GlyphLayout, GlyphBitmap, SubpixelBitmap, CacheStats};
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
        self.glyph(g, size26, false)?.read()
    }
    pub fn rasterize_glyph_subpixel(&self, g: u16, size: f32) -> Result<SubpixelBitmap, Error> {
        self.glyph(g, Self::size26(size)?, true)?.subpixel()
    }
    fn glyph(&self, g: u16, size26: i32, lcd: bool) -> Result<AtlasGlyph, Error> {
        let outline = self.0.glyph_outline(g)?;
        match self.0.layout_glyph(&outline, g, size26)? {
            GlyphLayout::Empty { advance } => Ok(AtlasGlyph::empty(advance)),
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => {
                let mut glyph = if lcd { scratch_lcd(&segments, width, height)? } else { scratch_render(&segments, width, height)? };
                (glyph.x_offset, glyph.y_offset) = (x_offset, y_offset);
                Ok(glyph)
            }
        }
    }
}

/// Uncached glyphs: rows of the process's scratch targets, each handed out once, so a
/// glyph's region is as the core zeroed it and its draws and work are a new target's.
struct ScratchSlot { owner: AtomicU64, targets: UnsafeCell<[Option<Scratch>; 2]> }
// SAFETY: `targets` is only reached while the PID-tagged owner lock is held.
unsafe impl Sync for ScratchSlot {}
static SCRATCH: ScratchSlot = ScratchSlot { owner: AtomicU64::new(0), targets: UnsafeCell::new([None, None]) };

/// A `width × height` region of scratch target `which`, its rows handed out once each.
fn region(which: usize, width: usize, height: usize) -> Result<Region, Error> {
    let me = device::pid();
    if me == 0 { return Err(Error::Device(-abi::errno::EIO)); }
    let inherited = device::acquire(&SCRATCH.owner, me);
    struct Held(u64);
    impl Drop for Held { fn drop(&mut self) { SCRATCH.owner.store(self.0 << 1, Ordering::Release); } }
    let _held = Held(me);
    // A fork may copy the slot mid-update; its handles are the parent's. Abandon them unread.
    if inherited { unsafe { SCRATCH.targets.get().write([None, None]); } }
    // SAFETY: the PID-tagged lock is held for the rest of this call.
    let slot = unsafe { &mut (*SCRATCH.targets.get())[which] };
    let fits = slot.as_ref().is_some_and(|s| s.target.owned_here() && width <= s.target.width()
        && s.next_row + height <= s.target.height());
    if !fits {
        *slot = Some(Scratch { target: Arc::new(target(width.max(SCRATCH_SIDE), height.max(SCRATCH_SIDE))?), next_row: 0 });
    }
    let s = slot.as_mut().expect("scratch made above");
    let y = s.next_row;
    s.next_row += height;
    Ok(Region { target: s.target.clone(), x: 0, y })
}
fn scratch_render(segments: &[LineSegment], width: usize, height: usize) -> Result<AtlasGlyph, Error> {
    admit(segments, width, height)?;
    let region = region(0, width, height)?;
    let fence = render(segments, width, height, &region)?;
    Ok(AtlasGlyph::at(region, width, height, fence))
}
fn scratch_lcd(segments: &[LineSegment], width: usize, height: usize) -> Result<AtlasGlyph, Error> {
    render_lcd(segments, width, height, &mut region)
}

/// The crossing wire layout, useful for independent host inspection.
pub fn crossing_table(segments: &[LineSegment], first_row: usize, rows: usize) -> Vec<u8> {
    libfont::crossings::CrossingTable::new(segments, first_row, rows).bytes().to_vec()
}

/// LCD uses the exact same binary32 operation order at three columns per pixel.
/// claim-lint:ok: docs/gui/breengel.md sections 7, 8 and 13 and docs/gui/gpu.md section 2 specify this contract.
/// Downsample each column class on the GPU, zero its other channels with a constant
/// channel mask, then use it as an LCD coverage mask to assemble the packed glyph. Each
/// stage is a fresh region from `region`, by scratch target 0 or 1; a stage samples only
/// regions of the other target.
fn render_lcd(segments: &[LineSegment], width: usize, height: usize,
    region: &mut dyn FnMut(usize, usize, usize) -> Result<Region, Error>) -> Result<AtlasGlyph, Error> {
    if libfont::bounds::raster_bound(segments.len(), height, width * 3) > libfont::bounds::MAX_RASTER_STEPS {
        return Err(FontError::GlyphTooExpensive.into());
    }
    let scaled = libfont::crossings::lcd_segments(segments);
    admit(&scaled, width * 3, height)?;
    let wide = region(0, width * 3, height)?;
    render(&scaled, width * 3, height, &wide)?;
    let sample = region(0, width, height)?;
    // Padding permits the last column-class sampling rectangle to remain in bounds.
    let padded = region(1, width * 3 + 2, height)?;
    let mask = region(1, width, height)?;
    let packed = region(1, width, height)?;
    let (w, h) = (width as i32, height as i32);
    let at = |r: &Region| [r.x as i32, r.y as i32];
    let rect = |r: &Region, rw: i32| [r.x as i32, r.y as i32, r.x as i32 + rw, r.y as i32 + h];
    // Per-pixel weights in gpu-abi.md: solid and copy 1, nearest_scaled 8192, coverage 256
    // plus its blend copy.
    packed.banded(rect(&packed, w), 1, |c, r| c.solid(word(Color::BLACK), &[r]))?;
    let [px, py] = at(&padded);
    let [wx, wy] = at(&wide);
    // The copy writes wide's columns with output X forced opaque, as blit does.
    padded.banded(rect(&padded, 3 * w), 1, |c, r| c.draw(pipeline::COPY, 0, wide.target.handle(), 0, 0,
        &[instance(r, &[wx - px, wy - py, 1])]))?;
    let mut fence = None;
    for channel in 0..3 {
        let [sx, sy] = at(&sample);
        sample.banded(rect(&sample, w), 8192, |c, r| c.draw(pipeline::NEAREST_SCALED, 0, padded.target.handle(), 0, 0,
            &[instance(r, &[sx, sy, w, h, px + channel, py, 3 * w, h])]))?;
        let colour = match channel { 0 => Color::rgb(0,255,255), 1 => Color::rgb(255,0,255), _ => Color::rgb(255,255,0) };
        mask.banded(rect(&mask, w), 1, |c, r| c.solid(word(colour), &[r]))?;
        let [mx, my] = at(&mask);
        sample.banded(rect(&sample, w), 257, |c, r| c.draw(pipeline::COVERAGE, 0xff000000, mask.target.handle(), 0, 0,
            &[instance(r, &[sx, sy, mx, my, 1])]))?;
        let [kx, ky] = at(&packed);
        fence = Some(packed.banded(rect(&packed, w), 257, |c, r| c.draw(pipeline::COVERAGE, 0xffffffff, sample.target.handle(), 0, 0,
            &[instance(r, &[kx, ky, sx, sy, 1])]))?);
    }
    Ok(AtlasGlyph::at(packed, width, height, fence))
}

/// A scratch target's side when no glyph needs more; its rows are handed out once each.
const SCRATCH_SIDE: usize = 256;
struct Scratch { target: Arc<Surface>, next_row: usize }

/// A glyph-sized rectangle of a target, at (x, y).
struct Region { target: Arc<Surface>, x: usize, y: usize }
impl Region {
    /// Encode `draw` after a `BEGIN` of this region's target and submit it.
    fn send(&self, draw: impl FnOnce(&mut bgl::Context)) -> Result<bgl::Fence, Error> {
        let t = &self.target;
        device::with(|d| {
            d.context.begin(t.handle(), t.width() as u32, t.height() as u32);
            draw(&mut d.context);
            d.context.submit().inspect_err(|_| d.context.discard())
        }).map_err(Error::Device)
    }
    /// Draw `rect` in bands, each its own submission with an estimate of at most half the
    /// work cap at `weight` steps per pixel. The instance parameters name origins, so each
    /// band draws the pixels the whole rectangle would there. Returns the last fence.
    fn banded(&self, rect: [i32; 4], weight: u64, mut draw: impl FnMut(&mut bgl::Context, [i32; 4])) -> Result<bgl::Fence, Error> {
        let budget = abi::limits::WORK / 2;
        let width = (rect[2] - rect[0]).max(1) as u64;
        let columns = ((budget / weight) as i32).max(1).min(width as i32);
        let rows = ((budget / (weight * width)) as i32).max(1);
        let mut fence = None;
        for y in (rect[1]..rect[3]).step_by(rows as usize) {
            for x in (rect[0]..rect[2]).step_by(columns as usize) {
                let band = [x, y, (x + columns).min(rect[2]), (y + rows).min(rect[3])];
                fence = Some(self.send(|c| draw(c, band))?);
            }
        }
        fence.ok_or(Error::Device(-abi::errno::EINVAL))
    }
}

fn target(width: usize, height: usize) -> Result<Surface, Error> {
    Surface::new(width, height, Format::BGRX).map_err(|e| match e {
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
    if raster_bound(segments.len(), height, width) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
    Ok(())
}

/// The bytes one crossing table may hold; a taller glyph's rows are split across tables.
const TABLE_BYTES: usize = 8 << 20;

/// Coverage of `segments` into the glyph-sized `region`, as the core cleared it. Each table's
/// rows are packed into submissions by their actual crossings, each submission's estimate at
/// most half the work cap; no pixel values reach the CPU. Returns the last fence.
fn render(segments: &[LineSegment], width: usize, height: usize, region: &Region) -> Result<Option<bgl::Fence>, Error> {
    if width == 0 || height == 0 { return Ok(None); }
    let budget = abi::limits::WORK / 2;
    let rows_per_table = (TABLE_BYTES / (32 + 64 * segments.len().max(1))).max(1);
    let (ox, oy) = (region.x as i32, region.y as i32);
    let mut fence = None;
    for row in (0..height).step_by(rows_per_table) {
        let count = rows_per_table.min(height - row);
        let table = libfont::crossings::CrossingTable::new(segments, row, count);
        let bytes = table.bytes().len() as u32;
        // Crossings on each row: the difference of its first sub-scanline offsets.
        let offset = |i: usize| u32::from_le_bytes(table.bytes()[4 * i..4 * i + 4].try_into().unwrap()) as u64;
        let crossings: Vec<u64> = (0..count).map(|r| offset(8 * (r + 1)) - offset(8 * r)).collect();
        // Each pixel costs 8192 per crossing on its row plus 8192 for accumulation, and the
        // blend copies it once.
        let pixel = |r: usize| 8192 * (1 + crossings[r]) + 1;
        let t = &region.target;
        let result = device::with(|d| {
            let buffer = gpu::buffer_create(bytes as u64)?;
            let sent = (|| {
                bgl::geometry_upload(table, &d.context, buffer)?;
                let mut draw = |x0: usize, x1: usize, r0: usize, r1: usize| -> Result<(), i64> {
                    d.context.begin(t.handle(), t.width() as u32, t.height() as u32);
                    let rect = [ox + x0 as i32, oy + (row + r0) as i32, ox + x1 as i32, oy + (row + r1) as i32];
                    d.context.draw(pipeline::COVERAGE_SPANS, 0xffffffff, 0, buffer, bytes,
                        &[instance(rect, &[ox, oy + row as i32, width as i32, count as i32, 0])]);
                    fence = Some(d.context.submit()?);
                    Ok(())
                };
                let mut r0 = 0;
                while r0 < count {
                    let row_cost = width as u64 * pixel(r0);
                    if row_cost > budget {
                        // A row can itself exceed the budget: split its rectangle in x; the
                        // parameters keep the glyph's width, so accumulation stays exact.
                        let columns = ((budget / pixel(r0)) as usize).max(1);
                        for x in (0..width).step_by(columns) { draw(x, (x + columns).min(width), r0, r0 + 1)?; }
                        r0 += 1;
                        continue;
                    }
                    let (mut r1, mut cost) = (r0, 0);
                    while r1 < count && cost + width as u64 * pixel(r1) <= budget {
                        cost += width as u64 * pixel(r1);
                        r1 += 1;
                    }
                    draw(0, width, r0, r1)?;
                    r0 = r1;
                }
                Ok(())
            })();
            gpu::resource_destroy(buffer);
            if sent.is_err() { d.context.discard(); }
            sent
        });
        result.map_err(Error::Device)?;
    }
    Ok(fence)
}

/// A glyph's coverage: its `width × height` region of a target (absent for an empty glyph)
/// and the fence of the draw that wrote it.
pub struct AtlasGlyph {
    region: Option<Region>, fence: Option<bgl::Fence>,
    pub width: usize, pub height: usize, pub x_offset: i32, pub y_offset: i32,
}
impl AtlasGlyph {
    fn empty(advance: usize) -> Self {
        AtlasGlyph { region: None, fence: None, width: advance, height: 0, x_offset: 0, y_offset: 0 }
    }
    fn at(region: Region, width: usize, height: usize, fence: Option<bgl::Fence>) -> Self {
        AtlasGlyph { region: Some(region), fence, width, height, x_offset: 0, y_offset: 0 }
    }
    /// Wait for the draw that wrote this glyph: a device failure is an error here.
    fn ready(&self) -> Result<(), Error> {
        self.fence.map_or(Ok(()), |f| f.wait(u64::MAX)).map_err(Error::Device)
    }
    fn pixels(&self) -> Result<Vec<u8>, Error> {
        let mut pixels = vec![0; self.width * self.height * 4];
        if let Some(r) = &self.region {
            self.ready()?;
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
    fn glyph(&mut self, glyph: u16, size26: i32) -> Result<Arc<AtlasGlyph>, Error> {
        if self.owner != device::pid() { self.clear(); self.owner = device::pid(); }
        self.clock += 1;
        if let Some(e) = self.entries.iter_mut().find(|e| e.glyph == glyph && e.size26 == size26) {
            self.stats.hits += 1; e.used = self.clock; return Ok(e.image.clone());
        }
        self.stats.misses += 1;
        let outline = self.font.glyph_outline(glyph)?;
        let (segments, width, height, x_offset, y_offset) = match self.font.layout_glyph(&outline, glyph, size26)? {
            GlyphLayout::Empty { advance } => {
                while self.entries.len() >= self.capacity { self.evict(); }
                return Ok(self.insert(glyph, size26, AtlasGlyph::empty(advance), None));
            }
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => (segments, width, height, x_offset, y_offset),
        };
        admit(&segments, width, height)?;
        let (pitch, rows) = class(width, height);
        if charge(pitch / 4, rows) > self.max_bytes {
            // Larger than the whole budget: rasterized, not kept.
            let mut image = scratch_render(&segments, width, height)?;
            (image.x_offset, image.y_offset) = (x_offset, y_offset);
            image.ready()?;
            return Ok(Arc::new(image));
        }
        while self.entries.len() >= self.capacity { self.evict(); }
        let (page, slot) = self.slot(pitch, rows)?;
        let p = self.pages.iter_mut().find(|p| p.id == page).expect("slot's page is live");
        p.used[slot] = true;
        let region = Region { target: p.target.clone(), x: 0, y: slot * rows };
        let reused = core::mem::replace(&mut p.drawn[slot], true);
        let drawn = (|| {
            // A slot drawn before is zeroed again first, so it is as a new target is.
            if reused { region.send(|c| c.solid(0, &[[0, region.y as i32, width as i32, (region.y + height) as i32]]))?; }
            let fence = render(&segments, width, height, &region)?;
            let mut image = AtlasGlyph::at(region, width, height, fence);
            (image.x_offset, image.y_offset) = (x_offset, y_offset);
            image.ready()?;
            Ok(image)
        })();
        match drawn {
            Ok(image) => Ok(self.insert(glyph, size26, image, Some((page, slot)))),
            Err(e) => { self.free(page, slot); Err(e) }
        }
    }
    fn insert(&mut self, glyph: u16, size26: i32, image: AtlasGlyph, slot: Option<(u64, usize)>) -> Arc<AtlasGlyph> {
        let image = Arc::new(image);
        self.entries.push(Entry { glyph, size26, image: image.clone(), used: self.clock, slot });
        self.stats.entries = self.entries.len();
        image
    }
    /// A free slot of class (pitch, rows), evicting least recently used entries until a
    /// page with one fits the byte budget.
    fn slot(&mut self, pitch: usize, rows: usize) -> Result<(u64, usize), Error> {
        loop {
            if let Some((id, slot)) = self.pages.iter().filter(|p| (p.pitch, p.rows) == (pitch, rows))
                .find_map(|p| p.used.iter().position(|u| !u).map(|s| (p.id, s))) {
                return Ok((id, slot));
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
            self.evict();
        }
    }
    fn evict(&mut self) {
        let oldest = self.entries.iter().enumerate().min_by_key(|(_, e)| e.used).map(|(i, _)| i).unwrap();
        if let Some((page, slot)) = self.entries.swap_remove(oldest).slot { self.free(page, slot); }
        self.stats.entries = self.entries.len();
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
        if raster_bound(s.len(), h, w) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let mut glyph = if w == 0 || h == 0 { AtlasGlyph::empty(0) } else {
            let region = region(0, w, h)?;
            let fence = render(s, w, h, &region)?;
            AtlasGlyph::at(region, w, h, fence)
        };
        (glyph.width, glyph.height, glyph.x_offset, glyph.y_offset) = (w, h, x, y);
        glyph.read()
    }
    pub fn rasterize_subpixel(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<SubpixelBitmap, Error> {
        if raster_bound(s.len(), h, w*3) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let mut glyph = if w == 0 || h == 0 { AtlasGlyph::empty(0) } else {
            scratch_lcd(s, w, h)?
        };
        (glyph.width, glyph.height, glyph.x_offset, glyph.y_offset) = (w, h, x, y);
        glyph.subpixel()
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

fn draw_glyphs(fb: &mut Surface, font: &mut CachedFont, glyphs: alloc::vec::Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let size26 = Font::size26(size).unwrap_or(0);
    layout(font, glyphs.into_iter(), size, |font, g, dx| {
        if !reaches_clip(fb, font, g, size26, x as i64 + dx, y as i64) {
            return;
        }
        match font.glyph(g, size26) {
            Ok(bitmap) => draw_atlas(fb, &bitmap, x as i64 + dx, y as i64, fg),
            Err(Error::Device(e)) => fb.fail(e),
            Err(_) => {},
        }
    })
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

/// The width `draw_text` returns, without drawing.
pub fn text_width(font: &mut CachedFont, text: &str, size: f32) -> i32 {
    let glyphs: alloc::vec::Vec<u16> = glyphs_of(font, text.chars().map(|c| c as u32)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

/// The width `draw_text_bytes` returns, without drawing.
pub fn text_width_bytes(font: &mut CachedFont, bytes: &[u8], size: f32) -> i32 {
    let glyphs: alloc::vec::Vec<u16> = glyphs_of(font, decode_utf8(bytes)).collect();
    layout(font, glyphs.into_iter(), size, |_, _, _| {})
}

fn draw_glyphs_subpixel(fb: &mut Surface, font: &mut CachedFont, glyphs: alloc::vec::Vec<u16>, x: i32, y: i32, size: f32, fg: Color) -> i32 {
    let size26 = Font::size26(size).unwrap_or(0);
    layout(font, glyphs.into_iter(), size, |font, g, dx| {
        if !reaches_clip(fb, font, g, size26, x as i64 + dx, y as i64) {
            return;
        }
        match font.font().glyph(g, size26, true) {
            Ok(bitmap) => draw_atlas(fb, &bitmap, x as i64 + dx, y as i64, fg),
            Err(Error::Device(e)) => fb.fail(e),
            Err(_) => {},
        }
    })
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
