//! Host checks of libfont's section 7 and 10 rules (docs/gui/breengel.md) on fonts built
//! here from literal tables. The guest suite (/sbin/suite-breengel) gives the verdict.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use libfont::rasterizer::{rasterize, raster_bound};
use libfont::outline::LineSegment;
use libfont::{work, Font, FontError};

use ttf::*;

/// Counts the bytes every allocation asks for; the tests run one at a time.
struct Counting;
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call goes to the system allocator unchanged; only a counter is added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size as u64, Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes allocated while `f` runs.
fn allocated<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let before = ALLOCATED.load(Relaxed);
    let out = f();
    (out, ALLOCATED.load(Relaxed) - before)
}

/// libfont steps taken while `f` runs.
fn steps<T>(f: impl FnOnce() -> T) -> (T, u64) {
    work::reset();
    let out = f();
    (out, work::steps())
}

/// A TrueType file built from literal tables (OpenType specification, "Organization of an
/// OpenType font" and the head, hhea, maxp, cmap, hmtx, loca, glyf and kern chapters).
mod ttf {
    pub fn be16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    pub fn be32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    pub struct Ttf {
        pub units_per_em: u16,
        pub glyphs: Vec<Vec<u8>>,
        /// Explicit long loca entries; computed from `glyphs` when `None`.
        pub loca: Option<Vec<u32>>,
        pub max_points: u16,
        pub max_composite_points: u16,
        pub cmap: Vec<u8>,
        pub kern: Option<Vec<u8>>,
        /// Further directory records (tag, offset, length), pointing anywhere in the file.
        pub records: Vec<([u8; 4], u32, u32)>,
    }

    impl Ttf {
        /// A font of `glyphs` with a cmap that maps nothing and generous maxp limits.
        pub fn new(glyphs: Vec<Vec<u8>>) -> Ttf {
            Ttf {
                units_per_em: 1000,
                glyphs,
                loca: None,
                max_points: 65_535,
                max_composite_points: 65_535,
                cmap: cmap_with(&[(3, 1, 12)], &cmap4_empty()),
                kern: None,
                records: Vec::new(),
            }
        }

        pub fn build(&self) -> Vec<u8> {
            let n = self.glyphs.len() as u16;
            let mut head = vec![0u8; 54];
            head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
            head[18..20].copy_from_slice(&self.units_per_em.to_be_bytes());
            head[50..52].copy_from_slice(&1i16.to_be_bytes());
            let mut hhea = vec![0u8; 36];
            hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
            hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
            hhea[34..36].copy_from_slice(&n.to_be_bytes());
            let mut maxp = vec![0u8; 32];
            maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            maxp[4..6].copy_from_slice(&n.to_be_bytes());
            maxp[6..8].copy_from_slice(&self.max_points.to_be_bytes());
            maxp[10..12].copy_from_slice(&self.max_composite_points.to_be_bytes());
            let mut hmtx = Vec::new();
            for _ in 0..n {
                be16(&mut hmtx, 500);
                be16(&mut hmtx, 0);
            }
            let (mut glyf, mut loca) = (Vec::new(), Vec::new());
            for g in &self.glyphs {
                be32(&mut loca, glyf.len() as u32);
                glyf.extend_from_slice(g);
            }
            be32(&mut loca, glyf.len() as u32);
            if let Some(entries) = &self.loca {
                loca.clear();
                for &e in entries {
                    be32(&mut loca, e);
                }
            }
            let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
                (*b"cmap", self.cmap.clone()),
                (*b"glyf", glyf),
                (*b"head", head),
                (*b"hhea", hhea),
                (*b"hmtx", hmtx),
                (*b"loca", loca),
                (*b"maxp", maxp),
            ];
            if let Some(k) = &self.kern {
                tables.push((*b"kern", k.clone()));
            }
            let count = tables.len() + self.records.len();
            let mut out = Vec::new();
            be32(&mut out, 0x0001_0000);
            be16(&mut out, count as u16);
            out.extend_from_slice(&[0; 6]);
            let offset = 12 + 16 * count;
            let mut body = Vec::new();
            for (tag, data) in &tables {
                out.extend_from_slice(tag);
                be32(&mut out, 0);
                be32(&mut out, (offset + body.len()) as u32);
                be32(&mut out, data.len() as u32);
                body.extend_from_slice(data);
                while body.len() % 4 != 0 {
                    body.push(0);
                }
            }
            for (tag, at, len) in &self.records {
                out.extend_from_slice(tag);
                be32(&mut out, 0);
                be32(&mut out, *at);
                be32(&mut out, *len);
            }
            assert_eq!(out.len(), offset);
            out.extend_from_slice(&body);
            out
        }
    }

    /// A cmap table of `records` (platform, encoding, offset from the table start), followed by
    /// `subtables` at offset `4 + 8 × records`.
    pub fn cmap_with(records: &[(u16, u16, u32)], subtables: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        be16(&mut v, 0);
        be16(&mut v, records.len() as u16);
        for &(p, e, o) in records {
            be16(&mut v, p);
            be16(&mut v, e);
            be32(&mut v, o);
        }
        v.extend_from_slice(subtables);
        v
    }

    /// A format 4 subtable holding only the final 0xFFFF segment.
    pub fn cmap4_empty() -> Vec<u8> {
        let mut v = Vec::new();
        for x in [4u16, 24, 0, 2, 2, 0, 0, 0xFFFF, 0, 0xFFFF, 1, 0] {
            be16(&mut v, x);
        }
        v
    }

    /// A format 12 subtable of `groups` (start, end, start glyph).
    pub fn cmap12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut v = Vec::new();
        be16(&mut v, 12);
        be16(&mut v, 0);
        be32(&mut v, 16 + 12 * groups.len() as u32);
        be32(&mut v, 0);
        be32(&mut v, groups.len() as u32);
        for &(s, e, g) in groups {
            be32(&mut v, s);
            be32(&mut v, e);
            be32(&mut v, g);
        }
        v
    }

    /// A simple glyph of on-curve contours, every coordinate a two-byte delta.
    pub fn simple(contours: &[&[(i16, i16)]]) -> Vec<u8> {
        let pts: Vec<(i16, i16)> = contours.iter().flat_map(|c| c.iter().copied()).collect();
        let (xs, ys): (Vec<i16>, Vec<i16>) = pts.iter().copied().unzip();
        let mut v = Vec::new();
        be16(&mut v, contours.len() as u16);
        for b in [*xs.iter().min().unwrap_or(&0), *ys.iter().min().unwrap_or(&0), *xs.iter().max().unwrap_or(&0), *ys.iter().max().unwrap_or(&0)] {
            be16(&mut v, b as u16);
        }
        let mut end = 0usize;
        for c in contours {
            end += c.len();
            be16(&mut v, (end - 1) as u16);
        }
        be16(&mut v, 0);
        v.extend(std::iter::repeat_n(0x01u8, pts.len()));
        let (mut px, mut py) = (0i16, 0i16);
        for &(x, _) in &pts {
            be16(&mut v, x.wrapping_sub(px) as u16);
            px = x;
        }
        for &(_, y) in &pts {
            be16(&mut v, y.wrapping_sub(py) as u16);
            py = y;
        }
        v
    }

    /// One component: glyph, flags (MORE_COMPONENTS is set by `compound`), the two arguments as
    /// words, and the transform's F2Dot14 words.
    pub struct Component {
        pub glyph: u16,
        pub flags: u16,
        pub args: (i16, i16),
        pub transform: Vec<i16>,
    }

    pub const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
    pub const ARGS_ARE_XY_VALUES: u16 = 0x0002;
    pub const WE_HAVE_A_SCALE: u16 = 0x0008;
    pub const MORE_COMPONENTS: u16 = 0x0020;
    pub const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
    pub const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
    pub const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

    /// A compound glyph of `parts`, then `tail` (instructions, or nothing).
    pub fn compound(parts: &[Component], tail: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        be16(&mut v, (-1i16) as u16);
        v.extend_from_slice(&[0; 8]);
        for (i, c) in parts.iter().enumerate() {
            let more = if i + 1 < parts.len() { MORE_COMPONENTS } else { 0 };
            be16(&mut v, c.flags | more | ARG_1_AND_2_ARE_WORDS);
            be16(&mut v, c.glyph);
            be16(&mut v, c.args.0 as u16);
            be16(&mut v, c.args.1 as u16);
            for &t in &c.transform {
                be16(&mut v, t as u16);
            }
        }
        v.extend_from_slice(tail);
        v
    }

    /// A version 0 kern table of `subtables` (coverage, pairs).
    pub fn kern(subtables: &[(u16, &[(u16, u16, i16)])]) -> Vec<u8> {
        let mut v = Vec::new();
        be16(&mut v, 0);
        be16(&mut v, subtables.len() as u16);
        for &(coverage, pairs) in subtables {
            be16(&mut v, 0);
            be16(&mut v, (14 + 6 * pairs.len()) as u16);
            be16(&mut v, coverage);
            be16(&mut v, pairs.len() as u16);
            v.extend_from_slice(&[0; 6]);
            for &(l, r, k) in pairs {
                be16(&mut v, l);
                be16(&mut v, r);
                be16(&mut v, k as u16);
            }
        }
        v
    }
}

/// A square of side 100 at the origin, glyph 1 of the fonts below.
fn square() -> Vec<u8> {
    simple(&[&[(0, 0), (0, 100), (100, 100), (100, 0)]])
}

#[test]
fn records_sharing_one_cmap_subtable_load_within_the_step_bound() {
    // 4,096 Unicode records all naming one 4,096-group format 12 subtable.
    let groups: Vec<(u32, u32, u32)> = (0..4_096u32).map(|i| (2 * i, 2 * i, 1)).collect();
    let records: Vec<(u16, u16, u32)> = (0..4_096).map(|_| (3, 10, 4 + 8 * 4_096)).collect();
    let mut t = Ttf::new(vec![Vec::new(), square()]);
    t.cmap = cmap_with(&records, &cmap12(&groups));
    let file = t.build();
    let (font, n) = steps(|| Font::parse(&file));
    let font = font.unwrap();
    assert_eq!((font.glyph_for(0), font.glyph_for(1), font.glyph_for(8_190)), (1, 0, 1));
    let bound = 16 * file.len() as u64 + 65_536;
    assert!(n <= bound, "loading took {n} steps; section 7 allows {bound}");
}

#[test]
fn a_large_table_directory_loads_within_the_allocation_bound() {
    let mut t = Ttf::new(vec![Vec::new(), square()]);
    t.records = (0..20_000u32).map(|i| (*b"zz00", i % 64, 0)).collect();
    let file = t.build();
    let (font, bytes) = allocated(|| Font::parse(&file));
    font.unwrap();
    let bound = 2 * file.len() as u64 + 65_536;
    assert!(bytes <= bound, "loading allocated {bytes} bytes; section 7 allows {bound}");
}

#[test]
fn many_one_point_contours_stay_within_the_outline_allocation_bound() {
    // 32,767 one-point contours, every coordinate repeated (flags 0x31 with repeats).
    let n = 32_767usize;
    let mut g = Vec::new();
    be16(&mut g, n as u16);
    g.extend_from_slice(&[0; 8]);
    for i in 0..n {
        be16(&mut g, i as u16);
    }
    be16(&mut g, 0);
    let mut left = n;
    while left > 0 {
        let run = left.min(256);
        g.push(0x31 | 0x08);
        g.push((run - 1) as u8);
        left -= run;
    }
    let mut t = Ttf::new(vec![Vec::new(), g]);
    t.max_points = n as u16;
    let file = t.build();
    let font = Font::parse(&file).unwrap();
    let (outline, bytes) = allocated(|| font.glyph_outline(1));
    assert_eq!(outline.unwrap().point_count(), n);
    let bound = 64 * n as u64 + 65_536;
    assert!(bytes <= bound, "the outline allocated {bytes} bytes; section 7 allows {bound}");
}

#[test]
fn a_compound_is_held_to_max_composite_points() {
    let part = |dx| Component { glyph: 1, flags: ARGS_ARE_XY_VALUES, args: (dx, 0), transform: vec![] };
    let pair = compound(&[part(0), part(200)], &[]);
    for (limit, ok) in [(7u16, false), (8, true)] {
        let mut t = Ttf::new(vec![Vec::new(), square(), pair.clone()]);
        t.max_points = 4;
        t.max_composite_points = limit;
        let font = Font::parse(&t.build()).unwrap();
        match font.glyph_outline(2) {
            Ok(o) => assert!(ok, "8 points passed maxCompositePoints {limit} ({} points)", o.point_count()),
            Err(e) => assert!(!ok && e == FontError::TooManyPoints, "maxCompositePoints {limit}: {e}"),
        }
        assert!(font.glyph_outline(1).is_ok());
    }
}

#[test]
fn equal_loca_entries_past_glyf_are_out_of_range() {
    let mut t = Ttf::new(vec![Vec::new(), square(), Vec::new()]);
    let len = square().len() as u32;
    t.loca = Some(vec![0, 0, len, len + 100]);
    let mut entries = t.loca.clone().unwrap();
    entries.push(len + 100);
    t.loca = Some(entries);
    t.glyphs.push(Vec::new());
    let font = Font::parse(&t.build()).unwrap();
    assert_eq!(font.glyph_outline(3).err(), Some(FontError::GlyphOutOfRange));
    assert!(font.glyph_outline(1).is_ok());
    assert_eq!(font.glyph_outline(0).unwrap().point_count(), 0);
}

#[test]
fn scaled_component_offsets_follow_the_component_transform() {
    // Scale 0.5 (F2Dot14 0x2000) and offset (100, 40).
    let with = |flags| Component { glyph: 1, flags: ARGS_ARE_XY_VALUES | WE_HAVE_A_SCALE | flags, args: (100, 40), transform: vec![0x2000] };
    for (flags, (dx, dy)) in [(0, (100.0, 40.0)), (SCALED_COMPONENT_OFFSET, (50.0, 20.0)),
        (UNSCALED_COMPONENT_OFFSET, (100.0, 40.0)), (SCALED_COMPONENT_OFFSET | UNSCALED_COMPONENT_OFFSET, (100.0, 40.0))] {
        let t = Ttf::new(vec![Vec::new(), square(), compound(&[with(flags)], &[])]);
        let font = Font::parse(&t.build()).unwrap();
        let got: Vec<(f32, f32)> = font.glyph_outline(2).unwrap().points.iter().map(|p| (p.x, p.y)).collect();
        let want: Vec<(f32, f32)> = [(0.0, 0.0), (0.0, 100.0), (100.0, 100.0), (100.0, 0.0)].iter().map(|&(x, y)| (0.5 * x + dx, 0.5 * y + dy)).collect();
        assert_eq!(got, want, "flags {flags:#06x}");
    }
}

#[test]
fn compound_instructions_are_read_and_bounded() {
    let part = |flags| Component { glyph: 1, flags: ARGS_ARE_XY_VALUES | flags, args: (0, 0), transform: vec![] };
    for (tail, ok) in [(vec![], false), (vec![0], false), (vec![0, 3, 1, 2], false), (vec![0, 3, 1, 2, 3], true), (vec![0, 0], true)] {
        let t = Ttf::new(vec![Vec::new(), square(), compound(&[part(WE_HAVE_INSTRUCTIONS)], &tail)]);
        let font = Font::parse(&t.build()).unwrap();
        match font.glyph_outline(2) {
            Ok(_) => assert!(ok, "instructions {tail:?} were accepted"),
            Err(e) => assert!(!ok && e == FontError::BadGlyph, "instructions {tail:?}: {e}"),
        }
    }
}

#[test]
fn kern_pairs_stay_inside_their_subtable_and_every_subtable_applies() {
    let font = |kern_table: Vec<u8>| {
        let mut t = Ttf::new(vec![Vec::new(), square(), square()]);
        t.kern = Some(kern_table);
        Font::parse(&t.build())
    };
    // Two additive subtables: -40 and -10 give -50; an override subtable replaces the sum.
    let f = font(kern(&[(0x0001, &[(1, 2, -40)]), (0x0001, &[(1, 2, -10), (2, 1, 7)])])).unwrap();
    assert_eq!((f.kern_units(1, 2), f.kern_units(2, 1), f.kern_units(1, 1)), (-50, 7, 0));
    let f = font(kern(&[(0x0001, &[(1, 2, -40)]), (0x0009, &[(1, 2, -10)]), (0x0001, &[(1, 2, -5)])])).unwrap();
    assert_eq!(f.kern_units(1, 2), -15);
    // Minimum, cross-stream and vertical subtables give no kerning.
    let f = font(kern(&[(0x0003, &[(1, 2, -40)]), (0x0005, &[(1, 2, -40)]), (0x0000, &[(1, 2, -40)])])).unwrap();
    assert_eq!(f.kern_units(1, 2), 0);
    // A first subtable whose length stops before its pair, followed by one holding the
    // bytes of that pair: the font is refused, not kerned from the next subtable's bytes.
    let mut short = kern(&[(0x0001, &[(1, 2, -40)]), (0x0001, &[(1, 2, -10)])]);
    short[6..8].copy_from_slice(&14u16.to_be_bytes());
    assert_eq!(font(short).err(), Some(FontError::Truncated));
    // A table cut inside a subtable header, or inside its pairs.
    let whole = kern(&[(0x0001, &[(1, 2, -40)])]);
    for cut in [6, 10, whole.len() - 1] {
        assert_eq!(font(whole[..cut].to_vec()).err(), Some(FontError::Truncated), "cut at {cut}");
    }
}

#[test]
fn a_ten_byte_glyph_declaring_32767_contours_counts_what_it_reads() {
    let mut g = Vec::new();
    be16(&mut g, 32_767);
    g.extend_from_slice(&[0; 8]);
    let font = Font::parse(&Ttf::new(vec![Vec::new(), g]).build()).unwrap();
    let (outline, n) = steps(|| font.glyph_outline(1));
    assert_eq!(outline.err(), Some(FontError::BadGlyph));
    assert!(n <= 16 * 10 + 4_096, "refusing it took {n} steps; section 7 allows {}", 16 * 10 + 4_096);
}

#[test]
fn rasterizing_crossing_stripes_counts_the_sort_and_keeps_the_bound() {
    // 32 one-pixel stripes two pixels apart: 64 vertical edges cross every sub-scanline.
    let (h, w) = (16usize, 130usize);
    let mut segments = Vec::new();
    for i in 0..32 {
        let (l, r) = (1.0 + 4.0 * i as f32, 2.0 + 4.0 * i as f32);
        let y1 = h as f32;
        segments.push(LineSegment { x0: l, y0: 0.0, x1: l, y1 });
        segments.push(LineSegment { x0: r, y0: y1, x1: r, y1: 0.0 });
    }
    let (bitmap, n) = steps(|| rasterize(&segments, w, h, 0, 0));
    assert_eq!(bitmap.unwrap().coverage[w + 1], 255);
    // Each of the 8 × 16 sub-scanlines sorts 64 crossings in 6 passes of 1 + 64 steps.
    let sorting = 8 * h as u64 * 6 * 65;
    assert!(n >= sorting, "{n} steps cannot include the {sorting} steps of the merge sort");
    let bound = raster_bound(segments.len(), h, w) as u64;
    assert!(n <= bound, "{n} steps; section 7 allows {bound}");
}

#[test]
fn a_compressed_glyph_resolves_within_the_outline_step_bound() {
    // One contour of 65,535 points in 526 bytes: repeat flags and coordinates that repeat
    // the previous one take no bytes, so the points, not the bytes, set the work.
    let n = 65_535usize;
    let mut g = Vec::new();
    be16(&mut g, 1);
    g.extend_from_slice(&[0; 8]);
    be16(&mut g, (n - 1) as u16);
    be16(&mut g, 0);
    let mut left = n;
    while left > 0 {
        let run = left.min(256);
        g.push(0x31 | 0x08);
        g.push((run - 1) as u8);
        left -= run;
    }
    let bytes = g.len() as u64;
    let font = Font::parse(&Ttf::new(vec![Vec::new(), g]).build()).unwrap();
    let (outline, k) = steps(|| font.glyph_outline(1));
    let p = outline.unwrap().point_count() as u64;
    assert_eq!((bytes, p), (526, 65_535));
    let bound = 16 * bytes + 8 * p + 4_096;
    assert!(k <= bound, "{k} steps; section 7 allows 16 x {bytes} + 8 x {p} + 4,096 = {bound}");
}
