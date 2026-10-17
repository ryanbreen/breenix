//! G3 checks, derived from `docs/gui/gpu.md` sections 1.3, 1.4, 1.4.1 and 1.8 and from
//! `docs/gui/gpu-abi.md`'s `UPLOAD`, `READBACK`, `COPY`, `DRAW_SAMPLED` and points 29 and 30.
//! Each expectation is the suite's own model of the contract's rules, computed pixel by
//! pixel here, not taken from the kernel or from the shared ABI crate's helpers.

use libbreenix::gpu::{self, abi, abi::Plain};
use libbreenix::suite::{case_ms_left, value, wait_for, CaseError, CaseResult};
use libbreenix::{graphics, memory, time};
use libbreenix::signal::{self, Itimerval, Sigaction, Timeval, ITIMER_REAL, SIGALRM};
use super::common::*;
use abi::{purpose, upload_source as src};

type R<T> = Result<T, CaseError>;

/// Bytes per pixel a resource of `fmt` reads back as: one for R8, else canonical BGRA.
fn out_bpp(fmt: u32) -> u32 {
    if fmt == format::R8 { 1 } else { 4 }
}

/// The suite's picture of a texture or target in the form it reads back as.
#[derive(Clone)]
struct Image {
    w: u32,
    h: u32,
    bpp: u32,
    px: Vec<u8>,
}

impl Image {
    fn new(fmt: u32, w: u32, h: u32) -> Image {
        let bpp = out_bpp(fmt);
        Image { w, h, bpp, px: vec![0; (w * h * bpp) as usize] }
    }

    fn filled(fmt: u32, w: u32, h: u32, value: &[u8]) -> Image {
        let mut i = Image::new(fmt, w, h);
        for p in i.px.chunks_exact_mut(i.bpp as usize) {
            p.copy_from_slice(value);
        }
        i
    }

    /// A pixel value that depends on position and seed, so a pixel moved by one place, or
    /// taken from another image, reads differently.
    fn pattern(fmt: u32, w: u32, h: u32, seed: u32) -> Image {
        let mut i = Image::new(fmt, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
                v ^= v >> 15;
                v = v.wrapping_mul(0x2C1B_3C6D);
                v ^= v >> 12;
                let b = v.to_le_bytes();
                i.set(x, y, &b[..i.bpp as usize]);
            }
        }
        i
    }

    fn at(&self, x: u32, y: u32) -> &[u8] {
        let at = ((y * self.w + x) * self.bpp) as usize;
        &self.px[at..at + self.bpp as usize]
    }

    fn set(&mut self, x: u32, y: u32, v: &[u8]) {
        let at = ((y * self.w + x) * self.bpp) as usize;
        self.px[at..at + self.bpp as usize].copy_from_slice(v);
    }

    fn contains(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && x < self.w as i64 && y < self.h as i64
    }

    /// `Ok` when `got` equals the picture byte for byte, or the first pixel that differs.
    fn matches(&self, got: &[u8], what: &str) -> R<()> {
        if got.len() != self.px.len() {
            return Err(fail_msg(format!("{what}: read {} bytes, expected {}", got.len(), self.px.len())));
        }
        let n = self.bpp as usize;
        match got.chunks_exact(n).zip(self.px.chunks_exact(n)).position(|(a, b)| a != b) {
            None => Ok(()),
            Some(i) => {
                let (x, y) = (i as u32 % self.w, i as u32 / self.w);
                Err(fail_msg(format!("{what}: pixel ({x},{y}) read {:02x?}, expected {:02x?}", &got[i * n..i * n + n], &self.px[i * n..i * n + n])))
            }
        }
    }
}

/// The bytes of canonical pixel `p` in source layout `source` (gpu-abi.md, `UPLOAD`).
fn source_bytes(source: u32, p: &[u8]) -> Vec<u8> {
    match source {
        src::B8G8R8A8 => vec![p[0], p[1], p[2], p[3]],
        src::R8G8B8A8 => vec![p[2], p[1], p[0], p[3]],
        src::B8G8R8 => vec![p[0], p[1], p[2]],
        src::R8G8B8 => vec![p[2], p[1], p[0]],
        _ => vec![p[0]],
    }
}

fn source_bpp(source: u32) -> usize {
    match source {
        src::B8G8R8A8 | src::R8G8B8A8 => 4,
        src::B8G8R8 | src::R8G8B8 => 3,
        _ => 1,
    }
}

/// The caller's buffer for `img` in layout `source` at `stride`: each row's padding, and the
/// pixels past `cols` x `rows`, hold 0xEE, so a read past them is visible as 0xEE.
fn encode_pixels(img: &Image, cols: u32, rows: u32, source: u32, stride: usize) -> Vec<u8> {
    let bpp = source_bpp(source);
    let len = if rows == 0 { 0 } else { (rows as usize - 1) * stride + cols as usize * bpp };
    let mut out = vec![0xEE; len];
    for y in 0..rows {
        for x in 0..cols {
            let at = y as usize * stride + x as usize * bpp;
            out[at..at + bpp].copy_from_slice(&source_bytes(source, img.at(x, y)));
        }
    }
    out
}

/// Place `img` at `(x, y)` of `model`, clipped, as an upload of `source` writes it: 3-byte
/// sources give alpha 255.
fn place(model: &mut Image, img: &Image, x: u32, y: u32, source: u32) {
    for j in 0..img.h {
        for i in 0..img.w {
            if model.contains((x + i) as i64, (y + j) as i64) {
                let mut v = img.at(i, j).to_vec();
                if source_bpp(source) == 3 {
                    v[3] = 255;
                }
                model.set(x + i, y + j, &v);
            }
        }
    }
}

fn texture(fmt: u32, w: u32, h: u32) -> R<u64> {
    gpu::texture_create(fmt, w, h).map_err(|e| fail_msg(format!("RESOURCE_CREATE of a {w}x{h} texture in format {fmt} returned {}", errno(e))))
}

/// Upload the whole of `img` at `(x, y)` in `source` layout at `stride`, requiring success.
fn upload_at(ctx: u64, t: u64, img: &Image, x: u32, y: u32, source: u32, stride: usize) -> R<()> {
    let px = encode_pixels(img, img.w, img.h, source, stride);
    let (r, _) = gpu::upload(ctx, t, [x, y, img.w, img.h], &px, stride as u32, source, purpose::IMAGE);
    ensure(r == 0, || format!("UPLOAD of {}x{} at ({x},{y}) in layout {source} at stride {stride} returned {}", img.w, img.h, errno(r)))
}

/// Upload the whole of `img` at the origin in the layout matching its format, at its natural
/// stride.
fn upload_all(ctx: u64, t: u64, fmt: u32, img: &Image) -> R<()> {
    let source = natural(fmt);
    upload_at(ctx, t, img, 0, 0, source, img.w as usize * source_bpp(source))
}

fn natural(fmt: u32) -> u32 {
    match fmt {
        format::R8G8B8A8 => src::R8G8B8A8,
        format::R8 => src::R8,
        _ => src::B8G8R8A8,
    }
}

/// Read a whole texture or target back at the stride `w × bpp`.
fn read_back(t: u64, fmt: u32, w: u32, h: u32) -> R<(Vec<u8>, [u8; record::SIZE])> {
    let stride = w * out_bpp(fmt);
    let mut px = vec![0u8; (stride * h) as usize];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(t, [0, 0, w, h], &mut px, stride, &mut rec);
    ensure(r == 0, || format!("READBACK of {w}x{h} returned {} (unwritten {})", errno(r), u32_at(&rec, record::UNWRITTEN)))?;
    Ok((px, rec))
}

fn expect_image(t: u64, fmt: u32, want: &Image, what: &str) -> R<[u8; record::SIZE]> {
    let (px, rec) = read_back(t, fmt, want.w, want.h)?;
    want.matches(&px, what)?;
    Ok(rec)
}

/// Bind a target this case drew to the scanout (section 1.10: each rendering case binds its
/// target while it owns the display).
fn show(ctx: u64, t: u64) -> R<()> {
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0, || format!("PRESENT returned {}", errno(r)))?;
    wait_ok(ctx, p.fence, 2_000)
}

/// A frame that clears target `t` to `bg` and draws `instances` from `tex` with `copy`.
fn sampled_frame(t: u64, bg: u32, tex: u64, instances: &[[i32; 6]]) -> Vec<u32> {
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::clear(&mut s, bg);
    encode::draw_sampled(&mut s, pipeline::COPY, tex, instances);
    s
}

/// What `copy` writes: pixel `(x, y)` of each instance reads texel `(sx + x − x0, sy + y − y0)`
/// when that texel and the pixel exist, converted by channel (an R8 texel `r` as
/// B 0, G 0, R `r`, A 255).
fn model_sampled(model: &mut Image, tex: &Image, tex_fmt: u32, instances: &[[i32; 6]]) {
    for &[x0, y0, x1, y1, sx, sy] in instances {
        for y in y0 as i64..y1 as i64 {
            for x in x0 as i64..x1 as i64 {
                let (tx, ty) = (sx as i64 + x - x0 as i64, sy as i64 + y - y0 as i64);
                if !model.contains(x, y) || !tex.contains(tx, ty) {
                    continue;
                }
                let texel = tex.at(tx as u32, ty as u32);
                let v = if tex_fmt == format::R8 { [0, 0, texel[0], 255] } else { [texel[0], texel[1], texel[2], texel[3]] };
                model.set(x as u32, y as u32, &v);
            }
        }
    }
}

/// What a region copy writes: each pixel of the source rectangle that exists in the source
/// and whose place in the destination exists, taken from the source before the copy.
fn model_copy(dst: &mut Image, before: &Image, r: [u32; 4], to: [u32; 2]) {
    for j in 0..r[3] as i64 {
        for i in 0..r[2] as i64 {
            let (sx, sy) = (r[0] as i64 + i, r[1] as i64 + j);
            let (dx, dy) = (to[0] as i64 + i, to[1] as i64 + j);
            if before.contains(sx, sy) && dst.contains(dx, dy) {
                let v = before.at(sx as u32, sy as u32).to_vec();
                dst.set(dx as u32, dy as u32, &v);
            }
        }
    }
}

fn colour_bytes(c: u32) -> [u8; 4] {
    c.to_le_bytes()
}

const BG: u32 = 0xFF20_3040;

/// RESOURCE_CREATE through its literal wire layout (gpu-abi.md), returning the raw result.
fn create_raw(kind: u32, fmt: u32, w: u32, h: u32) -> (i64, u64) {
    let mut a = [0u8; 48];
    for (offset, value) in [(0, kind), (4, fmt), (8, w), (12, h)] {
        a[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let r = resource_create(&mut a);
    (r, u64_at(&a, 24))
}

pub fn create_formats() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let before = counters(0)?.resources;
    for (seed, fmt) in [format::B8G8R8A8, format::R8G8B8A8, format::R8].into_iter().enumerate() {
        let (r, t) = create_raw(abi::kind::TEXTURE, fmt, 7, 5);
        ensure(r == 0 && t != 0, || format!("a 7x5 texture in format {fmt}: {}", errno(r)))?;
        let img = Image::pattern(fmt, 7, 5, seed as u32 + 1);
        upload_all(ctx, t, fmt, &img)?;
        let rec = expect_image(t, fmt, &img, "a texture in each format")?;
        ensure(u32_at(&rec, record::FORMAT) == fmt, || format!("record format {} for format {fmt}", u32_at(&rec, record::FORMAT)))?;
    }
    ensure(counters(0)?.resources == before + 3, || "three textures are not three resources".to_string())?;
    for fmt in [3, 4, 64, 67, 0xFFFF_FFFF] {
        let (r, t) = create_raw(abi::kind::TEXTURE, fmt, 7, 5);
        ensure(r == -EINVAL && t == 0, || format!("a texture in format {fmt} returned {} handle {t}", errno(r)))?;
    }
    // R8 is a texture format only: no R8 target.
    let (r, _) = create_raw(abi::kind::TARGET, format::R8, 7, 5);
    ensure(r == -EINVAL, || format!("an R8 target returned {}", errno(r)))?;
    ensure(counters(0)?.resources == before + 3, || "a refused creation made a resource".to_string())
}

pub fn create_limits() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let c = caps()?;
    let (mw, mh) = (c.max_width, c.max_height);
    ensure(mw >= 2560 && mh >= 2560, || format!("CAPS maximum {mw}x{mh}"))?;
    for fmt in [format::B8G8R8A8, format::R8] {
        for (w, h) in [(1, 1), (mw, 1), (1, mh)] {
            let (r, t) = create_raw(abi::kind::TEXTURE, fmt, w, h);
            ensure(r == 0, || format!("a {w}x{h} texture in format {fmt}: {}", errno(r)))?;
            // The far corner exists: upload it and read the texture back.
            let last = Image::pattern(fmt, 1, 1, w ^ h);
            upload_at(ctx, t, &last, w - 1, h - 1, natural(fmt), out_bpp(fmt) as usize)?;
            let mut px = vec![0u8; out_bpp(fmt) as usize];
            let mut rec = [0u8; record::SIZE];
            let r = gpu::readback(t, [w - 1, h - 1, 1, 1], &mut px, out_bpp(fmt), &mut rec);
            ensure(r == 0 && px == last.px, || format!("the last texel of {w}x{h}: {} read {px:02x?}", errno(r)))?;
            ensure(gpu::resource_destroy(t) == 0, || "RESOURCE_DESTROY".to_string())?;
        }
        for (w, h) in [(0, 1), (1, 0), (0, 0), (mw + 1, 1), (1, mh + 1)] {
            let (r, t) = create_raw(abi::kind::TEXTURE, fmt, w, h);
            ensure(r == -EINVAL && t == 0, || format!("a {w}x{h} texture returned {}", errno(r)))?;
        }
    }
    pass()
}

pub fn upload_full() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let other = context()?;
    let mut bytes = 0u64;
    for (seed, fmt) in [format::B8G8R8A8, format::R8G8B8A8, format::R8].into_iter().enumerate() {
        let t = texture(fmt, 37, 11)?;
        let img = Image::pattern(fmt, 37, 11, 10 + seed as u32);
        upload_all(ctx, t, fmt, &img)?;
        bytes += 37 * 11 * out_bpp(fmt) as u64;
        let rec = expect_image(t, fmt, &img, "a full upload")?;
        let canonical = if fmt == format::R8 { record::CANONICAL_R8 } else { record::CANONICAL_BGRA };
        for (at, v) in [(record::FORMAT, fmt), (record::CANONICAL, canonical), (record::RENDER_KIND, 0x205), (record::RENDER_STATUS, 0x1100), (record::READBACK_KIND, 0x206)] {
            ensure(u32_at(&rec, at) == v, || format!("record[{at}] is {}, expected {v}", u32_at(&rec, at)))?;
        }
        ensure(u64_at(&rec, record::GENERATION) == 1 && u64_at(&rec, record::RENDER_SERIAL) != 0 && u64_at(&rec, record::CONTEXT) != 0,
            || "the record does not name the texture's one upload".to_string())?;
    }
    // A fixture upload is counted under its own purpose.
    let t = texture(format::R8, 5, 3)?;
    let img = Image::pattern(format::R8, 5, 3, 3);
    let px = encode_pixels(&img, 5, 3, src::R8, 5);
    let (r, _) = gpu::upload(ctx, t, [0, 0, 5, 3], &px, 5, src::R8, purpose::FIXTURE);
    ensure(r == 0, || format!("a fixture UPLOAD returned {}", errno(r)))?;
    for (scope, k) in [("the context", counters(ctx)?), ("the process", counters(0)?)] {
        ensure(k.uploads_image == bytes && k.uploads_fixture == 15, || {
            format!("{scope} counts {} image and {} fixture upload bytes, expected {bytes} and 15", k.uploads_image, k.uploads_fixture)
        })?;
    }
    let k = counters(other)?;
    ensure(k.uploads_image == 0 && k.uploads_fixture == 0, || "another context counted the uploads".to_string())?;
    // Refusals: an unknown purpose, a nonzero reserved word, a foreign context.
    for p in [0, 3] {
        let (r, _) = gpu::upload(ctx, t, [0, 0, 5, 3], &px, 5, src::R8, p);
        ensure(r == -EINVAL, || format!("purpose {p} returned {}", errno(r)))?;
    }
    let mut a = abi::Upload { context: ctx, resource: t, width: 5, height: 3, pixels: px.as_ptr() as u64, pixels_len: 15, stride: 5,
        source: src::R8, purpose: purpose::IMAGE, reserved: 1, ..abi::Upload::ZERO };
    ensure(gpu::call_with(abi::op::UPLOAD, &mut a) == -EINVAL, || "a nonzero reserved word was accepted".to_string())?;
    let (r, _) = gpu::upload(ctx ^ 1, t, [0, 0, 5, 3], &px, 5, src::R8, purpose::IMAGE);
    ensure(r == -EINVAL, || format!("an unknown context returned {}", errno(r)))?;
    ensure(counters(0)?.uploads_fixture == 15, || "a refused upload was counted".to_string())
}

pub fn upload_subrect() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for fmt in [format::B8G8R8A8, format::R8] {
        let t = texture(fmt, 20, 10)?;
        let mut model = Image::pattern(fmt, 20, 10, 21);
        upload_all(ctx, t, fmt, &model)?;
        let patch = Image::pattern(fmt, 5, 4, 22);
        let source = natural(fmt);
        upload_at(ctx, t, &patch, 3, 2, source, 5 * source_bpp(source))?;
        place(&mut model, &patch, 3, 2, source);
        let rec = expect_image(t, fmt, &model, "a sub-rectangle upload")?;
        ensure(u64_at(&rec, record::GENERATION) == 2, || "two uploads did not give generation 2".to_string())?;
    }
    pass()
}

pub fn upload_clip() -> CaseResult {
    let _case = begin()?;
    for fmt in [format::B8G8R8A8, format::R8] {
        let ctx = context()?;
        let bpp = out_bpp(fmt) as usize;
        let source = natural(fmt);
        let t = texture(fmt, 10, 6)?;
        let mut model = Image::pattern(fmt, 10, 6, 31);
        upload_all(ctx, t, fmt, &model)?;
        // 6x5 at (7, 4) keeps 3x2; the caller's buffer holds exactly that, at a 3-pixel stride.
        let patch = Image::pattern(fmt, 6, 5, 32);
        let px = encode_pixels(&patch, 3, 2, source, 3 * bpp);
        ensure(px.len() == 3 * bpp + 3 * bpp, || "buffer arithmetic".to_string())?;
        let (r, _) = gpu::upload(ctx, t, [7, 4, 6, 5], &px, (3 * bpp) as u32, source, purpose::IMAGE);
        ensure(r == 0, || format!("an upload past the edge returned {}", errno(r)))?;
        place(&mut model, &patch, 7, 4, source);
        expect_image(t, fmt, &model, "a clipped upload")?;
        ensure(counters(ctx)?.uploads_image == (60 + 6) * bpp as u64, || format!("a clipped upload counted {} bytes", counters(ctx).map_or(0, |k| k.uploads_image)))?;
        // Refusals, each leaving the texture as it was.
        for (rect, len, stride, what) in [
            ([10, 0, 1, 1], 64, 64, "a rectangle wholly past the edge"),
            ([0, 6, 1, 1], 64, 64, "a rectangle wholly below the texture"),
            ([0, 0, 0, 1], 64, 64, "a zero width"),
            ([0, 0, 1, 0], 64, 64, "a zero height"),
            ([7, 4, 6, 5], 6 * bpp - 1, 3 * bpp, "a buffer one byte short of the clipped rows"),
            ([7, 4, 6, 5], 64, 3 * bpp - 1, "a stride shorter than the clipped row"),
        ] {
            let big = vec![0x5Au8; 64];
            let (r, _) = gpu::upload_raw(ctx, t, rect, big.as_ptr() as u64, len as u64, stride as u32, source, purpose::IMAGE);
            ensure(r == -EINVAL, || format!("{what} returned {}", errno(r)))?;
        }
        expect_image(t, fmt, &model, "refused uploads changed nothing")?;
    }
    pass()
}

pub fn upload_stride() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for (fmt, w, strides) in [(format::B8G8R8A8, 48u32, [192usize, 200, 260, 1000]), (format::R8, 64, [64, 65, 300, 513])] {
        let source = natural(fmt);
        for (seed, stride) in strides.into_iter().enumerate() {
            let t = texture(fmt, w, 7)?;
            let img = Image::pattern(fmt, w, 7, 40 + seed as u32);
            upload_at(ctx, t, &img, 0, 0, source, stride)?;
            expect_image(t, fmt, &img, &format!("format {fmt} at stride {stride}"))?;
        }
    }
    // An RGBA texture from its own layout and from BGRA bytes, converted by channel.
    for (seed, source) in [src::R8G8B8A8, src::B8G8R8A8].into_iter().enumerate() {
        let t = texture(format::R8G8B8A8, 19, 4)?;
        let img = Image::pattern(format::R8G8B8A8, 19, 4, 50 + seed as u32);
        upload_at(ctx, t, &img, 0, 0, source, 19 * 4 + 9)?;
        expect_image(t, format::R8G8B8A8, &img, "an RGBA texture at a padded stride")?;
    }
    pass()
}

pub fn upload_3byte() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
        for (seed, source) in [src::B8G8R8, src::R8G8B8].into_iter().enumerate() {
            for stride in [27usize, 31] {
                let t = texture(fmt, 9, 4)?;
                let mut model = Image::filled(fmt, 9, 4, &[1, 2, 3, 4]);
                upload_all(ctx, t, fmt, &model)?;
                let img = Image::pattern(fmt, 9, 4, 60 + seed as u32);
                upload_at(ctx, t, &img, 0, 0, source, stride)?;
                place(&mut model, &img, 0, 0, source);
                ensure(model.px.chunks_exact(4).all(|p| p[3] == 255), || "model alpha".to_string())?;
                expect_image(t, fmt, &model, &format!("3-byte layout {source} into format {fmt} at stride {stride}"))?;
            }
        }
    }
    // A 3-byte source fits only the 4-byte formats.
    let t = texture(format::R8, 4, 4)?;
    let px = vec![0u8; 48];
    let (r, _) = gpu::upload(ctx, t, [0, 0, 4, 4], &px, 12, src::B8G8R8, purpose::IMAGE);
    ensure(r == -EINVAL, || format!("a 3-byte upload into an R8 texture returned {}", errno(r)))
}

pub fn upload_visibility() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let second = context()?;
    let tex = texture(format::B8G8R8A8, 16, 8)?;
    let x = scanout_target(format::B8G8R8A8, 24, 12)?;
    for (round, seed) in [70u32, 71].into_iter().enumerate() {
        let img = Image::pattern(format::B8G8R8A8, 16, 8, seed);
        upload_all(ctx, tex, format::B8G8R8A8, &img)?;
        // Read back, so a dropped upload cannot pass as a texture of zeros.
        expect_image(tex, format::B8G8R8A8, &img, "the uploaded texture")?;
        // Sampled by the next submission, of this context and then of another.
        let inst = [[4, 2, 20, 10, 0, 0]];
        for (who, c) in [("this context", ctx), ("another context", second)] {
            frame(c, &sampled_frame(x, BG, tex, &inst))?;
            show(c, x)?;
            let mut model = Image::filled(format::B8G8R8A8, 24, 12, &colour_bytes(BG));
            model_sampled(&mut model, &img, format::B8G8R8A8, &inst);
            expect_image(x, format::B8G8R8A8, &model, &format!("upload {round} sampled by {who}"))?;
        }
    }
    // A buffer upload is visible to the next submission's DRAW_BUFFER.
    let buf = gpu::buffer_create(4096).map_err(|e| fail_msg(format!("a buffer: {}", errno(e))))?;
    let rect: [i32; 4] = [3, 1, 9, 5];
    let bytes: Vec<u8> = rect.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (r, _) = gpu::upload(ctx, buf, [32, 0, 16, 1], &bytes, 16, src::R8, purpose::IMAGE);
    ensure(r == 0, || format!("a buffer UPLOAD returned {}", errno(r)))?;
    let mut s = Vec::new();
    encode::begin(&mut s, x);
    encode::clear(&mut s, BG);
    encode::draw_buffer(&mut s, pipeline::SOLID, 0xFF00_FF00, buf, 32, 1);
    frame(ctx, &s)?;
    show(ctx, x)?;
    expect(x, &scene_canvas(24, 12, BG, &[(rect, 0xFF00_FF00)]), "the uploaded buffer's rectangle")?;
    // Keep another context's submission unpublished. Upload must wait, and a signal
    // must interrupt that pre-transfer wait without changing generation or counters.
    signal::sigaction(SIGALRM, Some(&Sigaction::new(on_upload_alarm)), None)
        .map_err(|e| fail_msg(format!("sigaction: {e}")))?;
    let filler = target(format::B8G8R8A8, 4, 4)?;
    for buffer in [false, true] {
        arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_DEPTH)?;
        let s = if buffer {
            let mut s = scene(x, BG, &[]);
            encode::draw_buffer(&mut s, pipeline::SOLID, 0xFF00_FF00, buf, 32, 1);
            s
        } else {
            sampled_frame(x, BG, tex, &[[0, 0, 16, 8, 0, 0]])
        };
        let f = submit(second, &s)?;
        ensure(gpu::wait(second, f, 0).0 == -ETIMEDOUT, || "other context's fence was not held".to_string())?;
        let before = counters(ctx)?.uploads_image;
        let timer = Itimerval { it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
            it_value: Timeval { tv_sec: 0, tv_usec: 30_000 } };
        signal::setitimer(ITIMER_REAL, &timer, None).map_err(|e| fail_msg(format!("setitimer: {e}")))?;
        let (r, _) = if buffer {
            gpu::upload(ctx, buf, [32, 0, 16, 1], &bytes, 16, src::R8, purpose::IMAGE)
        } else {
            gpu::upload(ctx, tex, [0, 0, 1, 1], &[1, 2, 3, 4], 4, src::B8G8R8A8, purpose::IMAGE)
        };
        // Cancel the timer even if the call incorrectly returned before the signal.
        signal::setitimer(ITIMER_REAL, &Itimerval { it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
            it_value: Timeval { tv_sec: 0, tv_usec: 0 } }, None).map_err(|e| fail_msg(format!("cancel timer: {e}")))?;
        ensure(r == -EINTR && counters(ctx)?.uploads_image == before,
            || format!("upload before other-context completion (buffer={buffer}): {}", errno(r)))?;
        release_depth(second, filler, 1)?;
        wait_ok(second, f, 2_000)?;
    }
    let img = Image::pattern(format::B8G8R8A8, 16, 8, 71);
    expect_image(tex, format::B8G8R8A8, &img, "texture unchanged after pre-transfer EINTR")?;
    failed_uploads_invalidate(ctx, second, x)
}

/// A texture upload whose transfer was sent and then failed leaves the texture invalid
/// (`docs/gui/gpu.md` section 1.4.1): point 31's stall keeps the device's answers from being
/// taken, so each transfer is still outstanding when a signal interrupts its wait (EINTR) and
/// when the watchdog ends it (EIO). Sampling either texture is then refused. The stall ends
/// in the watchdog's device reset, which the case waits out.
fn failed_uploads_invalidate(ctx: u64, second: u64, x: u64) -> CaseResult {
    let checker = context()?;
    let img = Image::pattern(format::B8G8R8A8, 8, 4, 72);
    let interrupted = texture(format::B8G8R8A8, 8, 4)?;
    let hung = texture(format::B8G8R8A8, 8, 4)?;
    for t in [interrupted, hung] {
        upload_all(ctx, t, format::B8G8R8A8, &img)?;
    }
    let refused = |t: u64, what: &str| -> CaseResult {
        for sampled in [false, true] {
            let mut s = Vec::new();
            if sampled {
                encode::begin(&mut s, x);
                encode::draw_sampled(&mut s, pipeline::COPY, t, &[[0, 0, 8, 4, 0, 0]]);
            } else {
                encode::copy(&mut s, t, [0, 0, 8, 4], x, [0, 0]);
            }
            let (r, sub) = gpu::submit(checker, &s);
            ensure(r == -EINVAL && sub.error == stream::error::INVALID_TARGET,
                || format!("{what} (sampled={sampled}): {} code {}", errno(r), sub.error))?;
        }
        Ok(())
    };
    // The stall starts when the device answers this submission.
    let stall = target(format::B8G8R8A8, 4, 4)?;
    let before = fired(test_point::WITHHOLD_FENCE);
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ALL)?;
    let start = now_ns();
    submit(second, &scene(stall, BG, &[]))?;
    while fired(test_point::WITHHOLD_FENCE) == before {
        ensure(case_ms_left() > 1_000, || "the stalling submission's answer did not arrive".to_string())?;
        let _ = time::sleep_ms(1);
    }
    let pixels = encode_pixels(&img, 8, 4, src::B8G8R8A8, 32);
    let timer = Itimerval { it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
        it_value: Timeval { tv_sec: 0, tv_usec: 30_000 } };
    signal::setitimer(ITIMER_REAL, &timer, None).map_err(|e| fail_msg(format!("setitimer: {e}")))?;
    let (r, _) = gpu::upload(ctx, interrupted, [0, 0, 8, 4], &pixels, 32, src::B8G8R8A8, purpose::IMAGE);
    signal::setitimer(ITIMER_REAL, &Itimerval { it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
        it_value: Timeval { tv_sec: 0, tv_usec: 0 } }, None).map_err(|e| fail_msg(format!("cancel timer: {e}")))?;
    ensure(r == -EINTR, || format!("an upload interrupted after its transfer was sent returned {}", errno(r)))?;
    refused(interrupted, "sampling a texture whose upload was interrupted after sending")?;
    // The watchdog ends this transfer 2 s after it was sent, about 2 s before the reset.
    wait_for(2_000, "hung-upload-ms");
    let sent = now_ns();
    let (r, _) = gpu::upload(ctx, hung, [0, 0, 8, 4], &pixels, 32, src::B8G8R8A8, purpose::IMAGE);
    value("hung-upload-ms", ms_since(sent) as i64, "ms", None);
    ensure(r == -EIO, || format!("an upload the watchdog ended returned {}", errno(r)))?;
    refused(hung, "sampling a texture whose upload the watchdog ended")?;
    // Wait out the device reset the stall ends in, so later cases start on a working device.
    wait_for(3_000, "reset-ms");
    while counters(checker)?.state != context_state::RESET {
        ensure(case_ms_left() > 1_000, || "the stalled device was not reset".to_string())?;
        let _ = time::sleep_ms(10);
    }
    value("reset-ms", ms_since(start) as i64, "ms", None);
    let fresh = context()?;
    let after = target(format::B8G8R8A8, 4, 4)?;
    frame(fresh, &scene(after, BG, &[]))?;
    expect(after, &scene_canvas(4, 4, BG, &[]), "new work after the reset")?;
    pass()
}

extern "C" fn on_upload_alarm(_sig: i32) {}

pub fn unaligned_pitch() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for (fmt, w, h) in [(format::R8, 64u32, 9u32), (format::B8G8R8A8, 48, 7)] {
        let t = texture(fmt, w, h)?;
        let mut model = Image::pattern(fmt, w, h, 80 + w);
        upload_all(ctx, t, fmt, &model)?;
        expect_image(t, fmt, &model, &format!("{w}-wide format {fmt}"))?;
        // A sub-rectangle away from the row start, and a partial readback at a tight stride.
        let patch = Image::pattern(fmt, 20, 3, 90 + w);
        let source = natural(fmt);
        upload_at(ctx, t, &patch, 13, 2, source, 20 * source_bpp(source))?;
        place(&mut model, &patch, 13, 2, source);
        expect_image(t, fmt, &model, &format!("{w}-wide format {fmt} after a sub-rectangle"))?;
        let bpp = out_bpp(fmt) as usize;
        let mut px = vec![0u8; 20 * 3 * bpp];
        let mut rec = [0u8; record::SIZE];
        let r = gpu::readback(t, [13, 2, 20, 3], &mut px, (20 * bpp) as u32, &mut rec);
        ensure(r == 0 && px == patch.px, || format!("the sub-rectangle read back alone: {}", errno(r)))?;
    }
    pass()
}

pub fn copy_region() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let tex = texture(format::B8G8R8A8, 30, 20)?;
    let img = Image::pattern(format::B8G8R8A8, 30, 20, 100);
    upload_all(ctx, tex, format::B8G8R8A8, &img)?;
    let a = scanout_target(format::B8G8R8A8, 30, 20)?;
    let b = scanout_target(format::B8G8R8A8, 30, 20)?;
    // Texture to target, then target to target, then one clipped by both edges.
    let mut model_a = Image::filled(format::B8G8R8A8, 30, 20, &colour_bytes(BG));
    let mut model_b = model_a.clone();
    let mut s = Vec::new();
    encode::begin(&mut s, a);
    encode::clear(&mut s, BG);
    encode::begin(&mut s, b);
    encode::clear(&mut s, BG);
    encode::copy(&mut s, tex, [0, 0, 30, 20], a, [0, 0]);
    model_copy(&mut model_a, &img, [0, 0, 30, 20], [0, 0]);
    encode::copy(&mut s, a, [3, 4, 10, 7], b, [12, 9]);
    model_copy(&mut model_b, &model_a.clone(), [3, 4, 10, 7], [12, 9]);
    encode::copy(&mut s, tex, [25, 15, 9, 9], b, [0, 16]);
    model_copy(&mut model_b, &img, [25, 15, 9, 9], [0, 16]);
    frame(ctx, &s)?;
    show(ctx, b)?;
    expect_image(a, format::B8G8R8A8, &model_a, "a whole texture copied to a target")?;
    expect_image(b, format::B8G8R8A8, &model_b, "region copies")?;
    // A copy between formats is refused naming its command, and the target keeps its pixels.
    let rgba = scanout_target(format::R8G8B8A8, 30, 20)?;
    let mut s = Vec::new();
    encode::copy(&mut s, a, [0, 0, 4, 4], rgba, [0, 0]);
    let (r, sub) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && sub.error_index == 0 && sub.error == stream::error::FORMAT_MISMATCH,
        || format!("a copy between formats returned {} at {} code {}", errno(r), sub.error_index, sub.error))?;
    // So is a copy into a texture.
    let mut s = Vec::new();
    encode::copy(&mut s, a, [0, 0, 4, 4], tex, [0, 0]);
    let (r, sub) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && sub.error == stream::error::BAD_HANDLE, || format!("a copy into a texture returned {} code {}", errno(r), sub.error))?;
    expect_image(tex, format::B8G8R8A8, &img, "the texture after the refused copy")?;
    // The copied area is counted once per draw.
    let k = counters(ctx)?;
    // Two clears, the whole copy, the 10x7 copy and the copy clipped to 5x4.
    ensure(k.draws == 5 && k.fragment_area == 2 * 600 + 600 + 70 + 20, || format!("copies counted {} draws over {} pixels", k.draws, k.fragment_area))?;
    // A real device refusal invalidates its written target; neither source command may
    // then sample it. A later whole-target COPY recovers it, with exact pixels checked.
    arm(test_point::DEVICE_ERROR, 0)?;
    let failed = submit(ctx, &scene(a, BG, &[]))?;
    ensure(gpu::wait(ctx, failed, 2_000_000_000).0 == -EIO, || "device error did not invalidate the copy source".to_string())?;
    for sampled in [false, true] {
        let mut refused = Vec::new();
        if sampled {
            encode::begin(&mut refused, b);
            encode::draw_sampled(&mut refused, pipeline::COPY, a, &[[0, 0, 30, 20, 0, 0]]);
        } else {
            encode::copy(&mut refused, a, [0, 0, 30, 20], b, [0, 0]);
        }
        let (r, sub) = gpu::submit(ctx, &refused);
        ensure(r == -EINVAL && sub.error == stream::error::INVALID_TARGET,
            || format!("invalid source (sampled={sampled}): {} code {}", errno(r), sub.error))?;
    }
    let mut recover = Vec::new();
    encode::copy(&mut recover, tex, [0, 0, 30, 20], a, [0, 0]);
    frame(ctx, &recover)?;
    expect_image(a, format::B8G8R8A8, &img, "whole COPY recovers the invalid target")?;
    pass()
}

pub fn copy_overlap() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let tex = texture(format::B8G8R8A8, 24, 16)?;
    let img = Image::pattern(format::B8G8R8A8, 24, 16, 110);
    upload_all(ctx, tex, format::B8G8R8A8, &img)?;
    let t = scanout_target(format::B8G8R8A8, 24, 16)?;
    let last = [([2u32, 2, 12, 8], [5u32, 4]), ([3, 3, 15, 11], [0, 0]), ([0, 5, 24, 1], [1, 5]), ([4, 0, 6, 16], [4, 2])];
    for (rect, to) in last {
        let mut s = Vec::new();
        encode::copy(&mut s, tex, [0, 0, 24, 16], t, [0, 0]);
        encode::copy(&mut s, t, rect, t, to);
        frame(ctx, &s)?;
        show(ctx, t)?;
        let mut want = img.clone();
        model_copy(&mut want, &img, rect, to);
        expect_image(t, format::B8G8R8A8, &want, &format!("an overlapping copy of {rect:?} to {to:?}"))?;
    }
    let mut model = img.clone();
    model_copy(&mut model, &img, [4, 0, 6, 16], [4, 2]);
    // A staged copy costs two device copies in the work estimate.
    let mut s = Vec::new();
    encode::copy(&mut s, t, [0, 0, 10, 10], t, [5, 5]);
    let (r, sub) = gpu::submit(ctx, &s);
    ensure(r == 0, || format!("SUBMIT of an overlapping copy returned {}", errno(r)))?;
    ensure(sub.estimate == 200, || format!("an overlapping 10x10 copy estimated {} steps, expected 200", sub.estimate))?;
    wait_ok(ctx, sub.fence, 2_000)?;
    let before = model.clone();
    model_copy(&mut model, &before, [0, 0, 10, 10], [5, 5]);
    expect_image(t, format::B8G8R8A8, &model, "a copy of the copy")?;
    pass()
}

/// Submissions that keep test point 31's fence-depth mode going: after `kept` submissions
/// of `ctx` are held unpublished, three more fill the four, and a fifth releases them.
fn release_depth(ctx: u64, filler: u64, kept: usize) -> R<()> {
    for _ in kept..5 {
        submit(ctx, &scene(filler, BG, &[]))?;
    }
    Ok(())
}

pub fn immutable_while_pinned() -> CaseResult {
    let _case = begin()?;
    same_context_ordering()?;
    let ctx = context()?;
    let tex = texture(format::B8G8R8A8, 12, 6)?;
    let old = Image::pattern(format::B8G8R8A8, 12, 6, 120);
    let new = Image::pattern(format::B8G8R8A8, 12, 6, 121);
    upload_all(ctx, tex, format::B8G8R8A8, &old)?;
    let x = scanout_target(format::B8G8R8A8, 12, 6)?;
    let y = target(format::B8G8R8A8, 12, 6)?;
    let filler = target(format::B8G8R8A8, 4, 4)?;
    let buf = gpu::buffer_create(4096).map_err(|e| fail_msg(format!("a buffer: {}", errno(e))))?;
    let first: Vec<u8> = [1i32, 1, 4, 4].iter().flat_map(|v| v.to_le_bytes()).collect();
    let (r, _) = gpu::upload(ctx, buf, [0, 0, 16, 1], &first, 16, src::R8, purpose::IMAGE);
    ensure(r == 0, || format!("a buffer UPLOAD returned {}", errno(r)))?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_DEPTH)?;
    let inst = [[0, 0, 12, 6, 0, 0]];
    let mut s = sampled_frame(x, BG, tex, &inst);
    encode::draw_buffer(&mut s, pipeline::SOLID, 0xFF00_00FF, buf, 0, 1);
    let pinned = submit(ctx, &s)?;
    ensure(gpu::wait(ctx, pinned, 0).0 == -ETIMEDOUT, || "the sampling submission is not in flight".to_string())?;
    // Both uploads return while its fence is unpublished. The device may already have
    // completed it: this checks publication and snapshots, not device execution ordering.
    upload_all(ctx, tex, format::B8G8R8A8, &new)?;
    let second: Vec<u8> = [6i32, 2, 9, 5].iter().flat_map(|v| v.to_le_bytes()).collect();
    let (r, _) = gpu::upload(ctx, buf, [0, 0, 16, 1], &second, 16, src::R8, purpose::IMAGE);
    ensure(r == 0, || format!("a buffer UPLOAD during the submission returned {}", errno(r)))?;
    ensure(gpu::wait(ctx, pinned, 0).0 == -ETIMEDOUT, || "the uploads waited for the in-flight submission".to_string())?;
    let after = submit(ctx, &sampled_frame(y, BG, tex, &inst))?;
    release_depth(ctx, filler, 2)?;
    wait_ok(ctx, pinned, 2_000)?;
    wait_ok(ctx, after, 2_000)?;
    show(ctx, x)?;
    let mut model = Image::filled(format::B8G8R8A8, 12, 6, &colour_bytes(BG));
    model_sampled(&mut model, &old, format::B8G8R8A8, &inst);
    for yy in 1..4 {
        for xx in 1..4 {
            model.set(xx, yy, &colour_bytes(0xFF00_00FF));
        }
    }
    expect_image(x, format::B8G8R8A8, &model, "the in-flight submission's samples and buffer rectangle")?;
    let mut model = Image::filled(format::B8G8R8A8, 12, 6, &colour_bytes(BG));
    model_sampled(&mut model, &new, format::B8G8R8A8, &inst);
    expect_image(y, format::B8G8R8A8, &model, "the next submission's samples")?;
    pass()
}

/// The sampling draws of each run of the ordering experiment: each draw samples the 2^20
/// texels of a 1024×1024 texture, so the runs cost 2^30 and then 2^31 fragment-steps, the
/// contract's work cap for one submission (`docs/gui/gpu-abi.md`). The heavier run is sent
/// only when the first one finished too soon, so it cannot outlast the watchdog where the
/// first stayed inside it.
const ORDERING_DRAWS: [u32; 2] = [1024, 2048];

/// Same-context ordering through existing interfaces, with no point armed (ruling 26): a
/// sampling submission still unanswered when the UPLOAD is issued must read the texture as
/// it was before the upload. A run whose submission is already answered at that moment does
/// not show ordering: it is reported instead of passed, and the experiment repeats with a
/// heavier submission; the case fails when no run up to the work cap is still in flight.
fn same_context_ordering() -> CaseResult {
    let ctx = context()?;
    let tex = texture(format::B8G8R8A8, 1024, 1024)?;
    let x = target(format::B8G8R8A8, 1024, 1024)?;
    let old = Image::filled(format::B8G8R8A8, 1024, 1024, &[17, 43, 89, 255]);
    upload_all(ctx, tex, format::B8G8R8A8, &old)?;
    let inst = [[0, 0, 1024, 1024, 0, 0]];
    let patch = [101, 37, 211, 255];
    for draws in ORDERING_DRAWS {
        let mut s = Vec::new();
        encode::begin(&mut s, x);
        for _ in 0..draws {
            encode::draw_sampled(&mut s, pipeline::COPY, tex, &inst);
        }
        let f = submit(ctx, &s)?;
        let start = now_ns();
        // No test point is armed: ETIMEDOUT here cannot be an answer held by point 31.
        let (pending, _) = gpu::wait(ctx, f, 0);
        if pending == 0 {
            value("ordering-finished-early-draws", draws as i64, "", None);
            continue;
        }
        ensure(pending == -ETIMEDOUT,
            || format!("ordering experiment's {draws}-draw sampling submission answered {}", errno(pending)))?;
        let (r, _) = gpu::upload(ctx, tex, [0, 0, 1, 1], &patch, 4, src::B8G8R8A8, purpose::IMAGE);
        ensure(r == 0, || format!("ordering experiment's UPLOAD returned {}", errno(r)))?;
        value("ordering-upload-ms", ms_since(start) as i64, "ms", None);
        let still = gpu::wait(ctx, f, 0).0 == -ETIMEDOUT;
        value("ordering-in-flight-after-upload", still as i64, "", None);
        value("ordering-draws", draws as i64, "", None);
        wait_ok(ctx, f, 2_000)?;
        expect_image(x, format::B8G8R8A8, &old, "sampling in flight when the same-context UPLOAD was issued")?;
        frame(ctx, &sampled_frame(x, BG, tex, &inst))?;
        let mut new = old;
        new.set(0, 0, &patch);
        expect_image(x, format::B8G8R8A8, &new, "sampling sent after the same-context upload")?;
        // Release this experiment's resources before the publication/snapshot experiment.
        for resource in [tex, x] {
            ensure(gpu::resource_destroy(resource) == 0, || "ordering resource destroy".to_string())?;
        }
        ensure(gpu::context_destroy(ctx) == 0, || "ordering context destroy".to_string())?;
        return pass();
    }
    Err(fail_msg(format!(
        "no ordering run demonstrated ordering: each sampling submission, {} to {} draws, was answered before its UPLOAD was issued",
        ORDERING_DRAWS[0], ORDERING_DRAWS[ORDERING_DRAWS.len() - 1])))
}

pub fn destroy_deferred() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let x = scanout_target(format::B8G8R8A8, 12, 6)?;
    let filler = target(format::B8G8R8A8, 4, 4)?;
    let baseline = counters(0)?;
    let tex = texture(format::B8G8R8A8, 64, 64)?;
    let img = Image::pattern(format::B8G8R8A8, 64, 64, 130);
    upload_all(ctx, tex, format::B8G8R8A8, &img)?;
    let held = counters(0)?.resource_bytes;
    let size = held - baseline.resource_bytes;
    ensure(size == 64 * 256, || format!("a 64x64 texture holds {size} bytes, expected {}", 64 * 256))?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_DEPTH)?;
    let inst = [[0, 0, 12, 6, 40, 50]];
    let f = submit(ctx, &sampled_frame(x, BG, tex, &inst))?;
    ensure(gpu::resource_destroy(tex) == 0, || "RESOURCE_DESTROY of the held texture".to_string())?;
    ensure(gpu::wait(ctx, f, 0).0 == -ETIMEDOUT, || "the sampling submission is not in flight".to_string())?;
    let k = counters(0)?;
    ensure(k.resources == baseline.resources && k.resource_bytes == held, || {
        format!("with its handle gone and its fence pending the process holds {} resources and {} bytes, expected {} and {held}", k.resources, k.resource_bytes, baseline.resources)
    })?;
    let mut s = Vec::new();
    encode::begin(&mut s, x);
    encode::draw_sampled(&mut s, pipeline::COPY, tex, &inst);
    let (r, sub) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && sub.error == stream::error::BAD_HANDLE, || format!("sampling the destroyed handle returned {} code {}", errno(r), sub.error))?;
    release_depth(ctx, filler, 1)?;
    wait_ok(ctx, f, 2_000)?;
    ensure(counters(0)?.resource_bytes == baseline.resource_bytes, || {
        format!("after its fence the process holds {} bytes, expected {}", counters(0).map_or(0, |k| k.resource_bytes), baseline.resource_bytes)
    })?;
    show(ctx, x)?;
    let mut model = Image::filled(format::B8G8R8A8, 12, 6, &colour_bytes(BG));
    model_sampled(&mut model, &img, format::B8G8R8A8, &inst);
    expect_image(x, format::B8G8R8A8, &model, "the texture sampled after its handle was destroyed")?;
    pass()
}

/// Textures a case destroys when it ends, however it ends, so a failing case cannot leave
/// frames held for the cases after it while the reclamation sweep catches up.
struct Held(Vec<u64>);

impl Drop for Held {
    fn drop(&mut self) {
        for t in self.0.drain(..) {
            let _ = gpu::resource_destroy(t);
        }
    }
}

pub fn quota() -> CaseResult {
    let _case = begin()?;
    let base = counters(0)?.resource_bytes;
    // 4096x4096 B8G8R8A8 rows are 16,384 bytes, already a multiple of 256: 64 MiB each.
    const ONE: u64 = 64 << 20;
    const QUOTA: u64 = 192 << 20;
    let mut held = Held(Vec::new());
    while base + (held.0.len() as u64 + 1) * ONE <= QUOTA {
        match gpu::texture_create(format::B8G8R8A8, 4096, 4096) {
            Ok(t) => held.0.push(t),
            Err(e) => {
                return Err(fail_msg(format!(
                    "RESOURCE_CREATE of a 4096x4096 texture returned {} with {} held and {} bytes charged, below the {QUOTA}-byte quota",
                    errno(e),
                    held.0.len(),
                    counters(0).map_or(0, |k| k.resource_bytes)
                )))
            }
        }
    }
    let full = counters(0)?.resource_bytes;
    ensure(full == base + held.0.len() as u64 * ONE, || format!("{} textures hold {full} bytes", held.0.len()))?;
    let (r, t) = create_raw(abi::kind::TEXTURE, format::B8G8R8A8, 4096, 4096);
    ensure(r == -abi::errno::ENOMEM && t == 0, || format!("a texture past the quota returned {}", errno(r)))?;
    if full + 4096 > QUOTA {
        let (r, _) = create_raw(abi::kind::TEXTURE, format::R8, 1, 1);
        ensure(r == -abi::errno::ENOMEM, || format!("a one-page texture past the quota returned {}", errno(r)))?;
    }
    ensure(counters(0)?.resource_bytes == full, || "a refused texture changed the counter".to_string())?;
    let last = held.0.pop().ok_or_else(|| fail_msg("no texture fit the quota".to_string()))?;
    ensure(gpu::resource_destroy(last) == 0, || "RESOURCE_DESTROY".to_string())?;
    ensure(counters(0)?.resource_bytes == full - ONE, || "freeing a texture did not restore its bytes".to_string())?;
    held.0.push(texture(format::B8G8R8A8, 4096, 4096)?);
    for t in held.0.drain(..) {
        ensure(gpu::resource_destroy(t) == 0, || "RESOURCE_DESTROY".to_string())?;
    }
    ensure(counters(0)?.resource_bytes == base, || format!("after freeing the textures the counter is {}, expected {base}", counters(0).map_or(0, |k| k.resource_bytes)))
}

pub fn r8_atlas() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for (w, h) in [(64u32, 5u32), (37, 3)] {
        let atlas = texture(format::R8, w, h)?;
        let img = Image::pattern(format::R8, w, h, 140 + w);
        upload_all(ctx, atlas, format::R8, &img)?;
        for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
            let t = scanout_target(fmt, 40, 12)?;
            // The whole atlas, one cell of it, and a cell clipped by the atlas's edge.
            let inst = [[1, 1, 1 + w as i32, 1 + h as i32, 0, 0], [2, 7, 10, 10, 8, 1], [30, 6, 40, 12, w as i32 - 4, 0]];
            frame(ctx, &sampled_frame(t, BG, atlas, &inst))?;
            show(ctx, t)?;
            let mut model = Image::filled(fmt, 40, 12, &colour_bytes(BG));
            model_sampled(&mut model, &img, format::R8, &inst);
            expect_image(t, fmt, &model, &format!("a {w}x{h} R8 atlas sampled into format {fmt}"))?;
        }
    }
    pass()
}

pub fn readback_uploaded() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for (seed, fmt) in [format::B8G8R8A8, format::R8G8B8A8, format::R8].into_iter().enumerate() {
        let t = texture(fmt, 23, 9)?;
        let mut previous = 0;
        for round in 0..2u32 {
            let img = Image::pattern(fmt, 23, 9, 150 + seed as u32 * 2 + round);
            upload_all(ctx, t, fmt, &img)?;
            let rec = expect_image(t, fmt, &img, "an uploaded texture")?;
            let (generation, serial) = (u64_at(&rec, record::GENERATION), u64_at(&rec, record::RENDER_SERIAL));
            ensure(generation == round as u64 + 1 && serial > previous, || format!("upload {round}: generation {generation}, serial {serial}"))?;
            previous = serial;
            // A rectangle at a padded stride leaves the padding alone.
            let bpp = out_bpp(fmt) as usize;
            let stride = 5 * bpp + 3;
            let mut px = vec![0xD7u8; stride * 4];
            let mut rec = [0u8; record::SIZE];
            let r = gpu::readback(t, [17, 4, 5, 4], &mut px, stride as u32, &mut rec);
            ensure(r == 0, || format!("a partial texture readback returned {}", errno(r)))?;
            for row in 0..4 {
                for col in 0..5 {
                    let at = row * stride + col * bpp;
                    ensure(px[at..at + bpp] == *img.at(17 + col as u32, 4 + row as u32), || format!("partial readback pixel ({col},{row})"))?;
                }
                ensure(px[row * stride + 5 * bpp..(row + 1) * stride].iter().all(|&b| b == 0xD7), || "stride padding was written".to_string())?;
            }
        }
    }
    pass()
}

pub fn efault_upload() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = texture(format::B8G8R8A8, 32, 40)?;
    let img = Image::pattern(format::B8G8R8A8, 32, 40, 160);
    upload_all(ctx, t, format::B8G8R8A8, &img)?;
    let before = counters(ctx)?;
    let ptr = memory::mmap(core::ptr::null_mut(), 8192, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| fail_msg(format!("mmap: {e:?}")))?;
    // SAFETY: the mapping is 8192 bytes, readable and writable.
    unsafe { core::ptr::write_bytes(ptr, 0x33, 8192) };
    memory::munmap(unsafe { ptr.add(4096) }, 4096).map_err(|e| fail_msg(format!("munmap: {e:?}")))?;
    // 40 rows of 128 bytes from 4096 - 2048: the second half lies in the unmapped page.
    for (addr, what) in [(unsafe { ptr.add(2048) } as u64, "a source running into an unmapped page"), (0u64, "a null source")] {
        let (r, _) = gpu::upload_raw(ctx, t, [0, 0, 32, 40], addr, 5120, 128, src::B8G8R8A8, purpose::IMAGE);
        ensure(r == -EFAULT, || format!("{what} returned {}", errno(r)))?;
    }
    let after = counters(ctx)?;
    ensure(after.uploads_image == before.uploads_image, || "a faulting upload was counted".to_string())?;
    let rec = expect_image(t, format::B8G8R8A8, &img, "the texture after EFAULT")?;
    ensure(u64_at(&rec, record::GENERATION) == 1, || "a faulting upload advanced the generation".to_string())?;
    memory::munmap(ptr, 4096).map_err(|e| fail_msg(format!("munmap: {e:?}")))?;
    pass()
}

pub fn fault_wrong_texture() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let a = texture(format::B8G8R8A8, 10, 6)?;
    let b = texture(format::B8G8R8A8, 4, 3)?;
    let ia = Image::pattern(format::B8G8R8A8, 10, 6, 170);
    let ib = Image::pattern(format::B8G8R8A8, 4, 3, 171);
    upload_all(ctx, a, format::B8G8R8A8, &ia)?;
    upload_all(ctx, b, format::B8G8R8A8, &ib)?;
    let x = scanout_target(format::B8G8R8A8, 10, 6)?;
    let inst = [[0, 0, 10, 6, 0, 0]];
    let model_of = |img: &Image| {
        let mut m = Image::filled(format::B8G8R8A8, 10, 6, &colour_bytes(BG));
        model_sampled(&mut m, img, format::B8G8R8A8, &inst);
        m
    };
    let before = fired(test_point::WRONG_TEXTURE);
    let r = gpu::tc_arm(test_point::WRONG_TEXTURE, a, 0);
    ensure(r == 0, || format!("arming point 29 returned {}", errno(r)))?;
    // The fence completes OK: only the comparison shows the wrong texture.
    frame(ctx, &sampled_frame(x, BG, a, &inst))?;
    show(ctx, x)?;
    ensure(fired(test_point::WRONG_TEXTURE) == before + 1, || "point 29 did not fire".to_string())?;
    let (px, _) = read_back(x, format::B8G8R8A8, 10, 6)?;
    ensure(model_of(&ia).matches(&px, "a draw bound to another texture").is_err(), || "the exact comparison accepted the wrong texture".to_string())?;
    model_of(&ib).matches(&px, "the texture the draw was bound to")?;
    frame(ctx, &sampled_frame(x, BG, a, &inst))?;
    expect_image(x, format::B8G8R8A8, &model_of(&ia), "the draw once point 29 disarmed")?;
    ensure(fired(test_point::WRONG_TEXTURE) == before + 1, || "point 29 fired twice".to_string())
}

pub fn fault_skip_upload() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = texture(format::B8G8R8A8, 10, 6)?;
    let old = Image::pattern(format::B8G8R8A8, 10, 6, 180);
    let new = Image::pattern(format::B8G8R8A8, 10, 6, 181);
    upload_all(ctx, t, format::B8G8R8A8, &old)?;
    let before = fired(test_point::SKIP_UPLOAD_VISIBILITY);
    let r = gpu::tc_arm(test_point::SKIP_UPLOAD_VISIBILITY, t, 0);
    ensure(r == 0, || format!("arming point 30 returned {}", errno(r)))?;
    // The upload returns as a completed one does.
    upload_all(ctx, t, format::B8G8R8A8, &new)?;
    ensure(fired(test_point::SKIP_UPLOAD_VISIBILITY) == before + 1, || "point 30 did not fire".to_string())?;
    let (px, _) = read_back(t, format::B8G8R8A8, 10, 6)?;
    ensure(new.matches(&px, "an upload whose transfer was omitted").is_err(), || "reading the texture back did not catch the omitted transfer".to_string())?;
    old.matches(&px, "the texture the device kept")?;
    upload_all(ctx, t, format::B8G8R8A8, &new)?;
    expect_image(t, format::B8G8R8A8, &new, "the upload once point 30 disarmed")?;
    ensure(fired(test_point::SKIP_UPLOAD_VISIBILITY) == before + 1, || "point 30 fired twice".to_string())
}

/// `pipelines/copy`: `copy` places a texture rectangle at a target point, converting
/// format by channel, from each texture format into each target format.
pub fn pipeline_copy() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    for (seed, tf) in [format::B8G8R8A8, format::R8G8B8A8, format::R8].into_iter().enumerate() {
        let tex = texture(tf, 9, 7)?;
        let img = Image::pattern(tf, 9, 7, 190 + seed as u32);
        upload_all(ctx, tex, tf, &img)?;
        for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
            let t = scanout_target(fmt, 21, 13)?;
            let inst = [[2, 3, 7, 6, 1, 2], [10, 0, 19, 7, 0, 0], [-3, 9, 4, 13, 2, 0]];
            frame(ctx, &sampled_frame(t, BG, tex, &inst))?;
            show(ctx, t)?;
            let mut model = Image::filled(fmt, 21, 13, &colour_bytes(BG));
            model_sampled(&mut model, &img, tf, &inst);
            expect_image(t, fmt, &model, &format!("copy from format {tf} into format {fmt}"))?;
        }
    }
    pass()
}
