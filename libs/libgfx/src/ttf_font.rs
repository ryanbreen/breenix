//! TrueType geometry on the CPU, coverage and glyph storage on the GPU.
//! Each atlas entry is an exact-size B8G8R8A8 target (B15/B19).
use alloc::{rc::Rc, vec, vec::Vec};
use core::ops::Deref;
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
        let glyph = self.glyph(g, Self::size26(size)?, true)?;
        let pixels = glyph.pixels()?;
        let coverage = pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
        Ok(SubpixelBitmap { width: glyph.width, height: glyph.height, x_offset: glyph.x_offset, y_offset: glyph.y_offset, coverage })
    }
    fn glyph(&self, g: u16, size26: i32, lcd: bool) -> Result<AtlasGlyph, Error> {
        let outline = self.0.glyph_outline(g)?;
        match self.0.layout_glyph(&outline, g, size26)? {
            GlyphLayout::Empty { advance } => Ok(AtlasGlyph { surface: None, width: advance, height: 0, x_offset: 0, y_offset: 0 }),
            GlyphLayout::Outline { segments, width, height, x_offset, y_offset } => {
                let surface = if lcd { render_lcd(&segments, width, height)? } else { render(&segments, width, height)? };
                Ok(AtlasGlyph { surface: Some(surface), width, height, x_offset, y_offset })
            }
        }
    }
}

/// The crossing wire layout, useful for independent host inspection.
pub fn crossing_table(segments: &[LineSegment], first_row: usize, rows: usize) -> Vec<u8> {
    libfont::crossings::CrossingTable::new(segments, first_row, rows).bytes().to_vec()
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
fn finish(surface: &Surface) -> Result<(), Error> {
    surface.error().map_or(Ok(()), |e| Err(Error::Device(e)))
}

/// Render batches admitted against the original total bound before any crossings or device work.
/// Each batch stays below the unchanged submission work quota; no pixel values reach the CPU.
fn render(segments: &[LineSegment], width: usize, height: usize) -> Result<Surface, Error> {
    use libfont::rasterizer::{raster_bound, MAX_RASTER_STEPS};
    if raster_bound(segments.len(), height, width) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
    let surface = target(width, height)?;
    let row_bound = 8192u64 * width as u64 * (1 + 8 * segments.len() as u64) + width as u64;
    // A row can itself exceed the quota: split its rectangle in x, retaining the original
    // width in the pipeline parameters, so span accumulation and clipping stay exact.
    let rows = ((abi::limits::WORK / row_bound.max(1)) as usize).max(1).min(height);
    for row in (0..height).step_by(rows) {
        let count = rows.min(height - row);
        let table = libfont::crossings::CrossingTable::new(segments, row, count);
        let bytes = table.bytes().len() as u32;
        let entries = table.entries() as u64;
        let cost_per_column = 8192 * (count as u64 + entries) + count as u64;
        let columns = ((abi::limits::WORK / cost_per_column.max(1)) as usize).max(1).min(width);
        let result = device::with(|d| {
            let buffer = gpu::buffer_create(bytes as u64)?;
            let result = (|| {
                bgl::geometry_upload(table, &d.context, buffer)?;
                for x in (0..width).step_by(columns) {
                    d.context.begin(surface.handle(), width as u32, height as u32);
                    let i = instance([x as i32, row as i32, (x + columns).min(width) as i32, (row + count) as i32],
                        &[0, row as i32, width as i32, count as i32, 0]);
                    d.context.draw(pipeline::COVERAGE_SPANS, 0xffffffff, 0, buffer, bytes, &[i]);
                    d.context.submit()?.wait(5_000_000_000)?;
                }
                Ok(())
            })();
            gpu::resource_destroy(buffer);
            if result.is_err() { d.context.discard(); }
            result
        });
        result.map_err(Error::Device)?;
    }
    Ok(surface)
}

/// LCD uses the exact same binary32 operation order at three columns per pixel.
/// Downsample each column class on the GPU, zero its other channels with a constant
/// channel mask, then use it as an LCD coverage mask to assemble the packed atlas.
fn render_lcd(segments: &[LineSegment], width: usize, height: usize) -> Result<Surface, Error> {
    if libfont::rasterizer::raster_bound(segments.len(), height, width * 3) > libfont::rasterizer::MAX_RASTER_STEPS {
        return Err(FontError::GlyphTooExpensive.into());
    }
    let scaled = libfont::crossings::lcd_segments(segments);
    let wide = render(&scaled, width * 3, height)?;
    let mut packed = target(width, height)?;
    packed.clear(Color::BLACK);
    // Padding permits the last column-class sampling rectangle to remain in bounds.
    let mut padded = target(width * 3 + 2, height)?;
    crate::blit::blit(&mut padded, &wide, 0, 0);
    let mut sample = target(width, height)?;
    let mut mask = target(width, height)?;
    let rect = [0, 0, width as i32, height as i32];
    for channel in 0..3 {
        sample.submit(|c| c.draw(pipeline::NEAREST_SCALED, 0, padded.handle(), 0, 0,
            &[instance(rect, &[0, 0, width as i32, height as i32, channel, 0, (width * 3) as i32, height as i32])]));
        let colour = match channel { 0 => Color::rgb(0,255,255), 1 => Color::rgb(255,0,255), _ => Color::rgb(255,255,0) };
        mask.clear(colour);
        sample.submit(|c| c.draw(pipeline::COVERAGE, 0xff000000, mask.handle(), 0, 0, &[instance(rect, &[0,0,0,0,1])]));
        packed.submit(|c| c.draw(pipeline::COVERAGE, 0xffffffff, sample.handle(), 0, 0, &[instance(rect, &[0,0,0,0,1])]));
        finish(&sample)?; finish(&mask)?; finish(&packed)?;
    }
    Ok(packed)
}

pub struct AtlasGlyph { surface: Option<Surface>, pub width: usize, pub height: usize, pub x_offset: i32, pub y_offset: i32 }
impl AtlasGlyph {
    fn bytes(&self) -> usize { self.surface.as_ref().map_or(0, |_| self.width * self.height * 4) }
    fn pixels(&self) -> Result<Vec<u8>, Error> {
        let mut pixels = vec![0; self.width * self.height * 4];
        if let Some(s) = &self.surface { s.read(&mut pixels, self.width * 4).map_err(Error::Device)?; }
        Ok(pixels)
    }
    fn read(&self) -> Result<GlyphBitmap, Error> {
        let coverage = self.pixels()?.chunks_exact(4).map(|p| p[0]).collect();
        Ok(GlyphBitmap { width: self.width, height: self.height, x_offset: self.x_offset, y_offset: self.y_offset, coverage })
    }
}
struct Entry { glyph: u16, size26: i32, image: Rc<AtlasGlyph>, used: u64 }
/// Exact-size atlas targets avoid unused page storage and charge every allocated texel.
pub struct CachedFont {
    font: Font, entries: Vec<Entry>, capacity: usize, max_bytes: usize, clock: u64,
    stats: CacheStats, readback: Option<GlyphBitmap>, owner: u64,
}
impl CachedFont {
    pub fn new(font: Font, capacity: usize) -> Self {
        Self { font, entries: Vec::new(), capacity: capacity.max(1), max_bytes: usize::MAX,
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
    fn glyph(&mut self, glyph: u16, size26: i32) -> Result<Rc<AtlasGlyph>, Error> {
        if self.owner != device::pid() { self.clear(); self.owner = device::pid(); }
        self.clock += 1;
        if let Some(e) = self.entries.iter_mut().find(|e| e.glyph == glyph && e.size26 == size26) {
            self.stats.hits += 1; e.used = self.clock; return Ok(e.image.clone());
        }
        self.stats.misses += 1;
        let image = Rc::new(self.font.glyph(glyph, size26, false)?);
        let bytes = image.bytes();
        if bytes > self.max_bytes { return Ok(image); }
        while !self.entries.is_empty() && (self.entries.len() >= self.capacity || self.stats.bytes + bytes > self.max_bytes) {
            let oldest = self.entries.iter().enumerate().min_by_key(|(_,e)| e.used).map(|(i,_)|i).unwrap();
            self.stats.bytes -= self.entries.swap_remove(oldest).image.bytes();
        }
        self.stats.bytes += bytes;
        self.entries.push(Entry { glyph, size26, image: image.clone(), used: self.clock });
        self.stats.entries = self.entries.len();
        Ok(image)
    }
    /// Explicit readback for checks and capture; drawing samples the atlas directly.
    pub fn rasterize_glyph(&mut self, glyph: u16, size: f32) -> Result<&GlyphBitmap, Error> {
        let image = self.glyph(glyph, Font::size26(size)?)?;
        self.readback = Some(image.read()?); Ok(self.readback.as_ref().unwrap())
    }
    pub fn cache_stats(&self) -> CacheStats { self.stats }
    pub fn clear(&mut self) { self.entries.clear(); self.readback = None; self.stats.entries = 0; self.stats.bytes = 0; }
    pub fn clear_cache(&mut self) { self.clear(); }
}

/// GPU raster entry points with the same readback shape as libfont's CPU reference.
pub mod rasterizer {
    use super::*;
    pub use libfont::rasterizer::{raster_bound, MAX_RASTER_STEPS};
    pub fn rasterize(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<GlyphBitmap, Error> {
        if raster_bound(s.len(), h, w) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let surface = if w == 0 || h == 0 { None } else { Some(render(s,w,h)?) };
        AtlasGlyph { surface, width:w, height:h, x_offset:x, y_offset:y }.read()
    }
    pub fn rasterize_subpixel(s: &[LineSegment], w: usize, h: usize, x: i32, y: i32) -> Result<SubpixelBitmap, Error> {
        if raster_bound(s.len(), h, w*3) > MAX_RASTER_STEPS { return Err(FontError::GlyphTooExpensive.into()); }
        let surface = if w == 0 || h == 0 { None } else { Some(render_lcd(s,w,h)?) };
        let glyph = AtlasGlyph { surface, width:w, height:h, x_offset:x, y_offset:y };
        let coverage = glyph.pixels()?.chunks_exact(4).flat_map(|p| [p[2],p[1],p[0]]).collect();
        Ok(SubpixelBitmap { width:w, height:h, x_offset:x, y_offset:y, coverage })
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
    let Some(atlas) = &image.surface else { return };
    let whole = Box64::wide(x + image.x_offset as i64, y + image.y_offset as i64, image.width as i64, image.height as i64);
    let visible = whole.intersect(&fb.clip_box());
    if visible.is_empty() { return; }
    work::add(1);
    let i = instance(visible.corners(), &[visible.x0 as i32, visible.y0 as i32,
        (visible.x0 - whole.x0) as i32, (visible.y0 - whole.y0) as i32, 1]);
    fb.submit(|c| c.draw(pipeline::COVERAGE, word(fg), atlas.handle(), 0, 0, &[i]));
    fb.mark(whole);
}
