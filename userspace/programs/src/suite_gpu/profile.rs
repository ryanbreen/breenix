//! Profile checks from gpu.md section 1.5. Expected pixels are constructed here;
//! drawing and device readback use the public GPU interface.
use super::common::*;
use libbreenix::gpu::{self, abi};
use breenix_suite::suite::{CaseError, CaseResult};

type R<T> = Result<T, CaseError>;
const BG: u32 = bgra(17, 83, 209, 127);
const FG: u32 = bgra(241, 7, 61, 0);

struct Run {
    ctx: u64,
    resources: Vec<u64>,
}
impl Run {
    fn new() -> R<Self> {
        Ok(Self {
            ctx: context()?,
            resources: Vec::new(),
        })
    }
    fn target(&mut self, fmt: u32, w: u32, h: u32) -> R<u64> {
        let t = scanout_target(fmt, w, h)?;
        self.resources.push(t);
        Ok(t)
    }
    fn texture(&mut self, fmt: u32, w: u32, h: u32) -> R<u64> {
        let t = gpu::texture_create(fmt, w, h)
            .map_err(|r| fail_msg(format!("texture: {}", errno(r))))?;
        self.resources.push(t);
        Ok(t)
    }
    fn show(&self, t: u64) -> CaseResult {
        present_view(self.ctx, t)?;
        Ok(())
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        for &r in &self.resources {
            let _ = gpu::resource_destroy(r);
        }
        let _ = gpu::context_destroy(self.ctx);
    }
}

fn rectangles(w: u32, h: u32, rs: &[([i32; 4], u32)]) -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
        let t = run.target(fmt, w, h)?;
        frame(run.ctx, &scene(t, BG, rs))?;
        expect(t, &scene_canvas(w, h, BG, rs), "profile rectangles")?;
        run.show(t)?;
    }
    Ok(())
}

pub fn clear_exact() -> CaseResult {
    rectangles(47, 13, &[])
}
pub fn rect_coverage() -> CaseResult {
    rectangles(73, 41, &[([3, 5, 70, 37], FG)])
}
pub fn rect_edges() -> CaseResult {
    rectangles(
        73,
        41,
        &[
            ([0, 0, 73, 1], FG),
            ([0, 40, 73, 41], 0),
            ([0, 1, 1, 40], 0xFFFFFFFF),
            ([72, 1, 73, 40], 0x12345678),
            ([35, 2, 36, 39], FG),
            ([2, 20, 71, 21], FG),
        ],
    )
}
pub fn viewport_exact() -> CaseResult {
    rectangles(
        257,
        129,
        &[
            ([0, 0, 1, 1], 0),
            ([256, 128, 257, 129], FG),
            ([128, 64, 129, 65], 0xFFFFFFFF),
            ([-2, -3, 3, 4], 0x44332211),
        ],
    )
}
pub fn alpha_write() -> CaseResult {
    rectangles(
        67,
        19,
        &[
            ([0, 0, 13, 19], bgra(241, 7, 61, 0)),
            ([13, 0, 26, 19], bgra(241, 7, 61, 1)),
            ([26, 0, 39, 19], bgra(241, 7, 61, 127)),
            ([39, 0, 52, 19], bgra(241, 7, 61, 254)),
            ([52, 0, 67, 19], bgra(241, 7, 61, 255)),
        ],
    )
}
pub fn swizzle_bgra_rgba() -> CaseResult {
    rectangles(
        31,
        7,
        &[
            ([0, 0, 7, 7], 0x000000FF),
            ([7, 0, 14, 7], 0x0000FF00),
            ([14, 0, 21, 7], 0x00FF0000),
            ([21, 0, 31, 7], 0xFF000000),
        ],
    )
}
pub fn no_dither() -> CaseResult {
    rectangles(257, 129, &[([0, 0, 257, 129], bgra(33, 127, 199, 67))])
}

/// Sampling fixtures contain input values, rather than a rendered expectation.
fn fetch(check_srgb: bool) -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    for source_fmt in [format::B8G8R8A8, format::R8G8B8A8, format::R8] {
        let tex = run.texture(source_fmt, 256, 4)?;
        let mut bytes = Vec::new();
        let mut want = Canvas::new(256, 4);
        for y in 0..4u32 {
            for k in 0..256u32 {
                let c = if source_fmt == format::R8 {
                    bgra(0, 0, k as u8, 255)
                } else {
                    bgra(
                        k as u8,
                        (255 - k) as u8,
                        (k ^ 0x55) as u8,
                        (k ^ (y * 37)) as u8,
                    )
                };
                want.rect([k as i32, y as i32, k as i32 + 1, y as i32 + 1], c);
                let b = c.to_le_bytes();
                match source_fmt {
                    format::R8 => bytes.push(k as u8),
                    format::R8G8B8A8 => bytes.extend_from_slice(&[b[2], b[1], b[0], b[3]]),
                    _ => bytes.extend_from_slice(&b),
                }
            }
        }
        let stride = if source_fmt == format::R8 { 256 } else { 1024 };
        let (r, _) = gpu::upload(
            run.ctx,
            tex,
            [0, 0, 256, 4],
            &bytes,
            stride,
            source_fmt,
            abi::purpose::FIXTURE,
        );
        ensure(r == 0, || format!("fixture upload: {}", errno(r)))?;
        for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
            let t = run.target(fmt, 256, 4)?;
            let mut s = scene(t, BG, &[]);
            encode::draw_sampled(&mut s, pipeline::COPY, tex, &[[0, 0, 256, 4, 0, 0]]);
            frame(run.ctx, &s)?;
            expect(
                t,
                &want,
                if check_srgb {
                    "linear UNORM sampling and output"
                } else {
                    "integer texel fetch"
                },
            )?;
            run.show(t)?;
        }
    }
    Ok(())
}
pub fn texel_fetch_exact() -> CaseResult {
    fetch(false)
}
pub fn no_srgb() -> CaseResult {
    // Direct integer results independently expose output encoding. The adjacent
    // sampled row exposes input decoding, including decode+encode round trips.
    let _case = begin()?;
    let mut run = Run::new()?;
    for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
        let t = run.target(fmt, 256, 2)?;
        let tex = run.texture(fmt, 256, 1)?;
        let mut bytes = Vec::new();
        let mut want = Canvas::new(256, 2);
        let mut s = scene(t, BG, &[]);
        for k in 0..256u32 {
            let c = bgra(k as u8, (255 - k) as u8, (k ^ 0x55) as u8, 127);
            let b = c.to_le_bytes();
            if fmt == format::R8G8B8A8 {
                bytes.extend_from_slice(&[b[2], b[1], b[0], b[3]]);
            } else {
                bytes.extend_from_slice(&b);
            }
            probe_command(&mut s, 0, 0, [k as i32, 0, k as i32 + 1, 1], &[c]);
            want.rect([k as i32, 0, k as i32 + 1, 2], c);
        }
        let (r, _) = gpu::upload(
            run.ctx,
            tex,
            [0, 0, 256, 1],
            &bytes,
            1024,
            fmt,
            abi::purpose::FIXTURE,
        );
        ensure(r == 0, || format!("sRGB fixture upload: {}", errno(r)))?;
        encode::draw_sampled(&mut s, pipeline::COPY, tex, &[[0, 1, 256, 2, 0, 0]]);
        frame(run.ctx, &s)?;
        expect(t, &want, "independent linear output and sampled input rows")?;
        run.show(t)?;
    }
    // Retain the original format and texel coverage as well.
    fetch(true)
}

pub fn determinism() -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    let t = run.target(format::B8G8R8A8, 73, 41)?;
    let rs = [([3, 5, 70, 37], FG), ([9, 7, 11, 40], 0x12345678)];
    let s = scene(t, BG, &rs);
    frame(run.ctx, &s)?;
    let (first, _) = read(t, 73, 41)?;
    frame(run.ctx, &scene(t, !BG, &[]))?;
    frame(run.ctx, &s)?;
    let (second, _) = read(t, 73, 41)?;
    ensure(first == second, || {
        "identical stream differs after intervening frame".to_string()
    })?;
    scene_canvas(73, 41, BG, &rs).matches(&second, "deterministic result")?;
    run.show(t)
}
pub fn cross_context() -> CaseResult {
    let _case = begin()?;
    let mut a = Run::new()?;
    let mut b = Run::new()?;
    let ta = a.target(format::B8G8R8A8, 73, 41)?;
    let tb = b.target(format::B8G8R8A8, 73, 41)?;
    let rs = [([3, 5, 70, 37], FG), ([9, 7, 11, 40], 0x12345678)];
    frame(a.ctx, &scene(ta, BG, &rs))?;
    frame(b.ctx, &scene(tb, BG, &rs))?;
    let (pa, _) = read(ta, 73, 41)?;
    let (pb, _) = read(tb, 73, 41)?;
    ensure(pa == pb, || {
        "contexts differ for the same scene".to_string()
    })?;
    scene_canvas(73, 41, BG, &rs).matches(&pb, "context result")?;
    a.show(ta)?;
    b.show(tb)
}

pub fn int_float_exact() -> CaseResult {
    // Geometry reaching the vertex stage is clipped to the 16,384 target domain.
    // Walk that entire representable domain, with neighbours at the right edge.
    rectangles(
        16384,
        2,
        &[
            ([0, 0, 16384, 1], FG),
            ([0, 1, 1, 2], 0),
            ([8191, 1, 8193, 2], 0x12345678),
            ([16383, 1, 16384, 2], 0xFFFFFFFF),
        ],
    )
}
pub fn fixed_blend_off() -> CaseResult {
    alpha_write()?;
    fetch(false)?;
    // Transparent and partial-alpha writes replace several different destination
    // values repeatedly, so destination-dependent blending cannot hide in a clear.
    rectangles(
        67,
        19,
        &[
            ([0, 0, 67, 19], 0xffffffff),
            ([0, 0, 67, 19], 0x00112233),
            ([0, 0, 34, 19], 0xffabcdef),
            ([0, 0, 67, 19], 0x7f246891),
            ([17, 3, 51, 16], 0x00112233),
        ],
    )?;
    probes(&[
        (0, 0, &[0x00112233], 0x00112233),
        (0, 0, &[0x7f246891], 0x7f246891),
        (0, 0, &[0xffabcdef], 0xffabcdef),
    ])
}

/// Raw command words use the separately proposed G4 ABI; expected result words below
/// are literal vectors, independent of the kernel arithmetic model.
fn probe_command(s: &mut Vec<u32>, routine: u32, word: u32, rect: [i32; 4], inputs: &[u32]) {
    s.extend_from_slice(&[7, 24, routine, word]);
    s.extend(rect.map(|v| v as u32));
    s.extend_from_slice(inputs);
    s.resize(s.len() + 16 - inputs.len(), 0);
}
fn probes(vectors: &[(u32, u32, &[u32], u32)]) -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
        let t = run.target(fmt, vectors.len() as u32, 1)?;
        let mut s = scene(t, BG, &[]);
        let mut want = Canvas::new(vectors.len() as u32, 1);
        for (i, &(routine, word, inputs, expected)) in vectors.iter().enumerate() {
            let r = [i as i32, 0, i as i32 + 1, 1];
            probe_command(&mut s, routine, word, r, inputs);
            want.rect(r, expected);
        }
        frame(run.ctx, &s)?;
        expect(t, &want, "literal profile probes")?;
        run.show(t)?;
    }
    Ok(())
}
pub fn integer_ops() -> CaseResult {
    probes(&[
        (0, 0, &[0xffffffff, 1], 0),
        (0, 0, &[0x7fffffff, 1], 0x80000000),
        (1, 0, &[0, 1], 0xffffffff),
        (1, 0, &[0x80000000, 1], 0x7fffffff),
        (2, 0, &[0xffffffff, 0xffffffff], 1),
        (3, 0, &[1, 31], 0x80000000),
        (3, 0, &[0xffffffff, 0], 0xffffffff),
        (4, 0, &[0x80000000, 31], 1),
        (5, 0, &[0x80000000, 31], 0xffffffff),
        (5, 0, &[0x80000000, 0], 0x80000000),
        (6, 0, &[0x80000000, 0x7fffffff], 1),
        (6, 0, &[0x7fffffff, 0x80000000], 0),
        (7, 0, &[0xffffffff, 0], 0),
        (7, 0, &[0, 0xffffffff], 1),
        (8, 0, &[0], 0),
        (8, 0, &[127], 0),
        (8, 0, &[128], 1),
        (8, 0, &[65025], 255),
    ])
}
pub fn wide_arithmetic() -> CaseResult {
    probes(&[
        (9, 0, &[0xffffffff, 0xffffffff], 1),
        (9, 1, &[0xffffffff, 0xffffffff], 0xfffffffe),
        (10, 0, &[0xffffffff, 0xffffffff, 0xffffffff], 1),
        (10, 1, &[0xffffffff, 0xffffffff, 0xffffffff], 0xffffffff),
        (10, 2, &[0xffffffff, 0xffffffff, 0xffffffff], 0xfffffffe),
        (11, 0, &[101, 0, 0, 7], 14),
        (11, 3, &[101, 0, 0, 7], 3),
        (11, 0, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0xffffffff),
        (11, 1, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0xffffffff),
        (11, 2, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0xffffffff),
        (11, 3, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0),
        (11, 4, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0),
        (11, 5, &[0xffffffff, 0xffffffff, 0xffffffff, 1], 0),
        (
            11,
            0,
            &[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],
            1,
        ),
        (
            11,
            3,
            &[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],
            0xffffffff,
        ),
        (
            11,
            4,
            &[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],
            0xffffffff,
        ),
        (
            11,
            5,
            &[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],
            0x7fffffff,
        ),
    ])
}
pub fn binary32_emulation() -> CaseResult {
    probes(&[
        (12, 0, &[0x3f800000, 0x33800000], 0x3f800000),
        (12, 0, &[0x3f800001, 0x33800000], 0x3f800002),
        (12, 0, &[1, 1], 2),
        (12, 0, &[0x7fffff, 1], 0x800000),
        (12, 0, &[0x80000000, 0x80000000], 0x80000000),
        (13, 0, &[0x3f800000, 0x3f800000], 0),
        (13, 0, &[0x3f800001, 0x3f800000], 0x34000000),
        (14, 0, &[0x800000, 0x3f000000], 0x400000),
        (14, 0, &[1, 0x3f000000], 0),
        (14, 0, &[3, 0x3f000000], 2),
        (14, 0, &[0x7f7fffff, 0x40000000], 0x7f800000),
        (15, 0, &[0x3ff33333], 1),
        (15, 0, &[0xbff33333], 0xffffffff),
        (15, 0, &[0xcf000000], 0x80000000),
        (16, 0, &[0xbdcccccd], 0x80000000),
        (16, 0, &[0x3f800001], 0x40000000),
        (16, 0, &[1], 0x3f800000),
        (17, 0, &[0x80000000, 0], 0),
        (17, 0, &[0xbf800000, 0], 1),
        (17, 0, &[0x3f800000, 0xbf800000], 0),
    ])
}
pub fn large_coordinates() -> CaseResult {
    probes(&[
        // Range ends, radius clamping, empty shapes, winding and edge ties.
        (
            20,
            0,
            &[0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x00000000],
            1,
        ),
        (
            20,
            0,
            &[0x7fffffff, 0x80000000, 0x80000000, 0x80000000, 0x7fffffff],
            0,
        ),
        (
            21,
            0,
            &[0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x00000000],
            1,
        ),
        (
            21,
            0,
            &[0x80000001, 0x80000000, 0x80000000, 0x80000000, 0x00000000],
            0,
        ),
        (
            22,
            0,
            &[
                0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff, 0x7fffffff,
            ],
            0,
        ),
        (
            22,
            0,
            &[
                0x80000000, 0xbfffffff, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff, 0x7fffffff,
            ],
            1,
        ),
        (
            22,
            0,
            &[
                0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff, 0x00000000,
            ],
            1,
        ),
        (
            22,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x7fffffff, 0x7fffffff,
            ],
            0,
        ),
        (
            22,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x80000000, 0x0000000a, 0x00000002,
            ],
            0,
        ),
        (
            22,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x0000000a, 0x00000000, 0x00000002,
            ],
            0,
        ),
        (
            22,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x0000000a, 0x0000000a, 0xffffffff,
            ],
            1,
        ),
        (
            23,
            0,
            &[
                0xfffffffe, 0xfffffffe, 0x80000000, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff,
                0x80000000,
            ],
            1,
        ),
        (
            23,
            0,
            &[
                0x00000000, 0x00000000, 0x80000000, 0x80000000, 0x00000000, 0x00000000, 0x7fffffff,
                0x7fffffff,
            ],
            0,
        ),
        (
            23,
            0,
            &[
                0x00000001, 0x00000002, 0x00000000, 0x00000000, 0x00000004, 0x00000000, 0x00000000,
                0x00000004,
            ],
            0,
        ),
        (
            23,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000004, 0x00000000, 0x00000000,
                0x00000004,
            ],
            1,
        ),
        (
            24,
            0,
            &[
                0x00000000, 0x00000000, 0x80000000, 0x00000000, 0x7fffffff, 0x00000000,
            ],
            1,
        ),
        (
            24,
            0,
            &[
                0x00000000, 0x00000000, 0x00000000, 0x80000000, 0x00000000, 0x7fffffff,
            ],
            1,
        ),
        (
            24,
            0,
            &[
                0x7fffffff, 0x7fffffff, 0x7fffffff, 0x7fffffff, 0x7fffffff, 0x7fffffff,
            ],
            1,
        ),
        (20, 0, &[0x7fffffff, 0, 0, 0, 0x7fffffff], 1),
        (20, 0, &[0x7fffffff, 1, 0, 0, 0x7fffffff], 0),
        (
            20,
            0,
            &[0, 0x80000000, 0x80000000, 0x80000000, 0x7fffffff],
            0,
        ),
        (21, 0, &[0x7fffffff, 1, 0, 0, 0x7fffffff], 1),
        (21, 0, &[0x7ffffffe, 0, 0, 0, 0x7fffffff], 0),
        (
            22,
            0,
            &[0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 10, 10, 2],
            0,
        ),
        (
            22,
            0,
            &[0x7ffffff6, 0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 10, 10, 2],
            1,
        ),
        (
            23,
            0,
            &[
                0xfffffffe, 0xfffffffe, 0x80000000, 0x80000000, 0x7fffffff, 0x80000000, 0x80000000,
                0x7fffffff,
            ],
            1,
        ),
        (
            23,
            0,
            &[
                0, 0, 0x80000000, 0x80000000, 0x7fffffff, 0x80000000, 0x80000000, 0x7fffffff,
            ],
            0,
        ),
        (
            24,
            0,
            &[0, 0, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff],
            1,
        ),
        (
            24,
            0,
            &[0, 1, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff],
            0,
        ),
    ])
}
pub fn unorm_roundtrip() -> CaseResult {
    let values: Vec<u32> = (0..256)
        .map(|k| k | ((255 - k) << 8) | ((k ^ 0x55) << 16) | ((k ^ 0x7f) << 24))
        .collect();
    let vectors: Vec<_> = values
        .iter()
        .map(|v| (0, 0, core::slice::from_ref(v), *v))
        .collect();
    probes(&vectors)
}
pub fn fragcoord_integer() -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    for routine in [18, 19] {
        let t = run.target(format::B8G8R8A8, 257, 19)?;
        let mut s = scene(t, BG, &[]);
        probe_command(&mut s, routine, 0, [0, 0, 257, 19], &[]);
        frame(run.ctx, &s)?;
        let mut want = Canvas::new(257, 19);
        for y in 0..19 {
            for x in 0..257 {
                want.rect(
                    [x, y, x + 1, y + 1],
                    if routine == 18 { x as u32 } else { y as u32 },
                );
            }
        }
        expect(t, &want, "fragment window coordinates")?;
        run.show(t)?;
    }
    Ok(())
}
pub fn scissor_exact() -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    let t = run.target(format::B8G8R8A8, 73, 41)?;
    let mut s = scene(t, BG, &[]);
    s.extend_from_slice(&[8, 6, 3, 5, 70, 37]);
    encode::draw(&mut s, pipeline::SOLID, FG, &[[-100, -100, 100, 100]]);
    frame(run.ctx, &s)?;
    expect(
        t,
        &scene_canvas(73, 41, BG, &[([3, 5, 70, 37], FG)]),
        "half-open scissor",
    )?;
    run.show(t)?;
    // A clipped clear does not repair pixels invalidated by a failed submission.
    arm(test_point::DEVICE_ERROR, 0)?;
    let f = submit(run.ctx, &scene(t, FG, &[]))?;
    let (r, _) = gpu::wait(run.ctx, f, 2_000_000_000);
    ensure(r == -EIO, || format!("planted device error: {}", errno(r)))?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    s.extend_from_slice(&[8, 6, 3, 5, 70, 37]);
    encode::clear(&mut s, FG);
    frame(run.ctx, &s)?;
    let mut px = vec![0u8; 73 * 41 * 4];
    let mut record = [0u8; record::SIZE];
    let r = gpu::readback(t, [0, 0, 73, 41], &mut px, 73 * 4, &mut record);
    ensure(r == -EIO, || {
        format!("partial clear repaired an invalid target: {}", errno(r))
    })?;
    frame(run.ctx, &scene(t, BG, &[]))?;
    expect(
        t,
        &scene_canvas(73, 41, BG, &[]),
        "whole clear after scissor",
    )?;
    run.show(t)
}
fn filtered(inject: bool) -> CaseResult {
    let _case = begin()?;
    let mut run = Run::new()?;
    let t = run.target(format::B8G8R8A8, 17, 9)?;
    let s = scene(t, BG, &[]);
    frame(run.ctx, &s)?;
    let tex = run.texture(format::B8G8R8A8, 4, 4)?;
    let fixture = FG.to_le_bytes().repeat(16);
    let (r, _) = gpu::upload(
        run.ctx,
        tex,
        [0, 0, 4, 4],
        &fixture,
        16,
        abi::upload_source::B8G8R8A8,
        abi::purpose::FIXTURE,
    );
    ensure(r == 0, || format!("filter fixture upload: {}", errno(r)))?;
    let other = if inject {
        Some(run.target(format::B8G8R8A8, 17, 9)?)
    } else {
        None
    };
    if let Some(o) = other {
        frame(run.ctx, &scene(o, BG, &[]))?;
    }
    // A refusal changes no counters, generations or pixels (gpu-abi.md, point 24).
    let generation = |t: u64| read(t, 17, 9).map(|(_, rec)| u64_at(&rec, record::GENERATION));
    let generations = (generation(t)?, other.map(generation).transpose()?);
    let before = counters(run.ctx)?;
    let mut s = Vec::new();
    if let Some(o) = other {
        encode::begin(&mut s, o);
        encode::draw_sampled(&mut s, pipeline::COPY, tex, &[[1, 1, 7, 5, 0, 0]]);
    }
    encode::begin(&mut s, t);
    encode::draw_sampled(
        &mut s,
        if inject {
            pipeline::COPY
        } else {
            pipeline::COPY | 0x80000000
        },
        tex,
        &[[1, 1, 7, 5, 0, 0]],
    );
    if inject {
        let r = gpu::tc_arm(24, 0, 0);
        ensure(r == 0, || format!("profile point: {}", errno(r)))?;
    }
    let (r, a) = gpu::submit(run.ctx, &s);
    ensure(r == -EINVAL && a.error_index == 1 && a.error == 19, || {
        format!(
            "filtered draw: {} index={} error={}",
            errno(r),
            a.error_index,
            a.error
        )
    })?;
    let after = counters(run.ctx)?;
    let gpu_counters = |k: &abi::Counters| {
        (k.submissions, k.draws, k.instances, k.fragment_area, k.work_estimate, k.device_errors)
    };
    ensure(gpu_counters(&after) == gpu_counters(&before), || {
        format!(
            "refused filtered draw changed the context's counters from {:?} to {:?}",
            gpu_counters(&before),
            gpu_counters(&after)
        )
    })?;
    if inject {
        ensure(fired(24) == 1, || {
            format!("profile-violation fired {} times", fired(24))
        })?;
    }
    let rec = expect(
        t,
        &scene_canvas(17, 9, BG, &[]),
        "filtered refusal atomicity",
    )?;
    let mut now = (u64_at(&rec, record::GENERATION), None);
    if let Some(o) = other {
        let rec = expect(
            o,
            &scene_canvas(17, 9, BG, &[]),
            "draw on another target before the planted refusal",
        )?;
        now.1 = Some(u64_at(&rec, record::GENERATION));
    }
    ensure(now == generations, || {
        format!("refused filtered draw moved generations from {generations:?} to {now:?}")
    })?;
    if inject {
        // Point 24's event target is 0, so an arm naming a resource does not match it (gpu-abi.md).
        let r = gpu::tc_arm(24, t, 0);
        ensure(r == 0, || format!("profile point with a target: {}", errno(r)))?;
    }
    let mut clean = Vec::new();
    encode::begin(&mut clean, t);
    encode::draw_sampled(&mut clean, pipeline::COPY, tex, &[[1, 1, 7, 5, 0, 0]]);
    let r = frame(run.ctx, &clean);
    if inject {
        let fired = fired(24);
        ensure(gpu::tc_disarm_all() == 0, || "DISARM_ALL refused".to_string())?;
        r?;
        ensure(fired == 1, || {
            format!("profile-violation armed for target {t:#x} fired; {fired} firings in all")
        })?;
    } else {
        r?;
    }
    expect(
        t,
        &scene_canvas(17, 9, BG, &[([1, 1, 5, 5], FG)]),
        "profile refusal recovery",
    )?;
    run.show(t)
}
pub fn filtered_sampler_refused() -> CaseResult {
    filtered(false)
}
pub fn profile_violation_detected() -> CaseResult {
    filtered(true)
}
