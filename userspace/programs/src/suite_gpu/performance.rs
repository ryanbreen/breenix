//! G8 checks: the performance bounds of `docs/gui/gpu.md` section 1.10, at 2560x1600.
//!
//! The bounds are measured on contexts whose fixed rendering catalogue has warmed
//! (`common::context`). A device's answer does not show the GPU did the work, so each timed
//! interval ends with a dependent observation: a readback of what the measured work wrote,
//! compared with the suite's own expectation. The throughput draws cannot be culled: the
//! rectangles do not overlap and the coverage draw blends. Whole-target comparisons run
//! after the timed interval, outside it.
use super::common::*;
use super::pipelines::{crossings, edges};
use gpu::abi::draw::{self, Instance};
use libbreenix::gpu;
use libbreenix::suite::{value, CaseError, CaseResult};
type R<T> = Result<T, CaseError>;

const W: u32 = 2560;
const H: u32 = 1600;
const BG: u32 = 0xff312719;
const FG: u32 = 0xffb76321;
/// The 2560x1600 mode's index in `MODES` (`docs/gui/gpu-abi.md`, G7 display interface).
const MODE_2560: u32 = 1;
/// CLOCK_THREAD_CPUTIME_ID: the calling thread's user and system time.
const THREAD_CPUTIME: u32 = 3;

// The bounds, from the contract's table (section 1.10).
const CLEARS_PER_S: u64 = 60;
const RECTS: usize = 10_000;
const RECTS_US: u64 = 50_000;
const SUBMIT_MEDIAN_US: u64 = 2_000;
const SUBMIT_MAX_US: u64 = 10_000;
const SUBMITS: usize = 1_000;
const FIRST_DRAW_US: u64 = 10_000;
const READBACK_US: u64 = 100_000;
const UPLOAD_US: u64 = 100_000;
const COVERAGE_US: u64 = 16_700;
const WAIT_CPU_PERCENT: u64 = 5;
const PRESENTS_PER_S: u64 = 60;

/// Frames timed by the two rate cases: two seconds' worth at the bound.
const FRAMES: u64 = 120;

fn us_since(start: u64) -> u64 {
    now_ns().saturating_sub(start) / 1_000
}

fn thread_cpu_ns() -> R<u64> {
    let mut ts = libbreenix::Timespec::new();
    libbreenix::time::clock_gettime(THREAD_CPUTIME, &mut ts)
        .map_err(|e| fail_msg(format!("clock_gettime(CLOCK_THREAD_CPUTIME_ID): {e:?}")))?;
    Ok(ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64)
}

fn take_display_2560() -> R<()> {
    libbreenix::graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let r = gpu::set_mode(MODE_2560);
    ensure(r == 0, || format!("SET_MODE 2560x1600: {}", errno(r)))
}

fn present(ctx: u64, t: u64) -> R<()> {
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0 && p.answer == 0x1100, || format!("PRESENT: {} answer {:#x}", errno(r), p.answer))
}

/// Colour `n` of a sequence, so consecutive frames of a timed run differ.
fn colour(n: u64) -> u32 {
    let n = n as u32;
    bgra((n * 37) as u8, (n * 101 + 17) as u8, (n * 59 + 113) as u8, 255)
}

/// Read back the rectangle `r` of target `t` (0 for the current scanout).
fn read_rect(t: u64, r: [u32; 4]) -> R<Vec<u8>> {
    let mut px = vec![0u8; (r[2] * r[3] * 4) as usize];
    let mut rec = [0u8; record::SIZE];
    let ret = gpu::readback(t, r, &mut px, r[2] * 4, &mut rec);
    ensure(ret == 0, || format!("READBACK returned {} (unwritten {})", errno(ret), u32_at(&rec, record::UNWRITTEN)))?;
    Ok(px)
}

/// `Ok` when each pixel of `px` is `c`, else the first that differs.
fn all(px: &[u8], c: u32, what: &str) -> R<()> {
    match px.chunks_exact(4).position(|p| u32_at(p, 0) != c) {
        None => Ok(()),
        Some(i) => Err(fail_msg(format!("{what}: pixel {i} read {:#010x}, expected {c:#010x}", u32_at(&px[i * 4..], 0)))),
    }
}

/// Whether `us` microseconds for `frames` frames is at least `per_s` frames a second.
fn rate_met(frames: u64, us: u64, per_s: u64) -> bool {
    frames * 1_000_000 >= per_s * us
}

pub fn clear_rate_2560() -> CaseResult {
    let _case = begin()?;
    take_display_2560()?;
    let ctx = context()?;
    let targets = [scanout_target(format::B8G8R8A8, W, H)?, scanout_target(format::B8G8R8A8, W, H)?];
    let start = now_ns();
    for n in 0..FRAMES {
        let t = targets[(n % 2) as usize];
        let mut s = Vec::new();
        encode::begin(&mut s, t);
        encode::clear(&mut s, colour(n));
        submit(ctx, &s)?;
        present(ctx, t)?;
    }
    let last = colour(FRAMES - 1);
    all(&read_rect(0, [W - 16, H - 16, 16, 16])?, last, "the scanout after the last presented clear")?;
    let us = us_since(start);
    value("clears-per-second", (FRAMES * 1_000_000 / us.max(1)) as i64, "", Some((CLEARS_PER_S as i64, i64::MAX)));
    ensure(rate_met(FRAMES, us, CLEARS_PER_S), || {
        format!("{FRAMES} presented 2560x1600 clears took {us} us, under {CLEARS_PER_S} a second")
    })?;
    all(&read_rect(0, [0, 0, W, H])?, last, "the whole scanout after the last presented clear")
}

pub fn rect_throughput() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, W, H)?;
    frame(ctx, &scene(t, BG, &[]))?;
    // 10,000 disjoint 16x16 tiles, row-major across the target, a hundred draws of a
    // hundred rectangles with a colour per draw.
    let per_row = (W / 16) as usize;
    let tile = |i: usize| {
        let (x, y) = ((i % per_row) as i32 * 16, (i / per_row) as i32 * 16);
        [x, y, x + 16, y + 16]
    };
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    let mut want = scene_canvas(W, H, BG, &[]);
    for d in 0..RECTS / 100 {
        let rects: Vec<[i32; 4]> = (d * 100..d * 100 + 100).map(tile).collect();
        encode::draw(&mut s, pipeline::SOLID, colour(d as u64), &rects);
        for r in &rects {
            want.rect(*r, colour(d as u64));
        }
    }
    let last = tile(RECTS - 1).map(|v| v as u32);
    let start = now_ns();
    let f = submit(ctx, &s)?;
    wait_ok(ctx, f, 2_000)?;
    all(&read_rect(t, [last[0], last[1], 16, 16])?, colour((RECTS / 100 - 1) as u64), "the last rectangle")?;
    let us = us_since(start);
    value("submission-us", us as i64, "us", Some((0, RECTS_US as i64)));
    ensure(us <= RECTS_US, || format!("{RECTS} solid 16x16 rectangles in one submission took {us} us"))?;
    expect(t, &want, "the 10000 rectangles")?;
    pass()
}

pub fn submit_latency() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 16, 16)?;
    // An empty submission: a BEGIN with no draw after it. Its fence is the measurement.
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    let mut us = Vec::with_capacity(SUBMITS);
    for _ in 0..SUBMITS {
        let start = now_ns();
        let f = submit(ctx, &s)?;
        wait_ok(ctx, f, 2_000)?;
        us.push(us_since(start));
    }
    us.sort_unstable();
    let median = (us[SUBMITS / 2 - 1] + us[SUBMITS / 2]) / 2;
    let max = us[SUBMITS - 1];
    value("median-us", median as i64, "us", Some((0, SUBMIT_MEDIAN_US as i64)));
    value("max-us", max as i64, "us", Some((0, SUBMIT_MAX_US as i64)));
    ensure(median <= SUBMIT_MEDIAN_US && max <= SUBMIT_MAX_US, || {
        format!("empty submissions' fences over {SUBMITS}: median {median} us, maximum {max} us")
    })
}

/// A texture of `fmt`, `w` by `h`, holding `bytes`.
fn texture(ctx: u64, fmt: u32, w: u32, h: u32, bytes: &[u8]) -> R<u64> {
    let t = gpu::texture_create(fmt, w, h).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let source = match fmt {
        format::R8 => gpu::abi::upload_source::R8,
        _ => gpu::abi::upload_source::B8G8R8A8,
    };
    let (r, _) = gpu::upload(ctx, t, [0, 0, w, h], bytes, w * format::bytes(fmt), source, gpu::abi::purpose::FIXTURE);
    ensure(r == 0, || format!("upload: {}", errno(r)))?;
    Ok(t)
}

fn instance(rect: [i32; 4], params: &[i32]) -> Instance {
    let mut a = [0; 16];
    for (d, &v) in a.iter_mut().zip(params) {
        *d = v as u32;
    }
    Instance { rect, params: a }
}

pub fn first_draw_latency() -> CaseResult {
    const S: i32 = 16;
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, S as u32, S as u32)?;
    let image = texture(ctx, format::B8G8R8A8, 16, 16, &FG.to_le_bytes().repeat(256))?;
    let mask = texture(ctx, format::R8, 16, 16, &[255u8; 256])?;
    let bits = texture(ctx, format::R8, 1, 7, &[0x1f; 7])?;
    let spans = gpu::buffer_create(4096).map_err(|e| fail_msg(format!("buffer: {}", errno(e))))?;
    let crossing = crossings(&edges(&[(0.0, 0.0), (16.0, 0.0), (16.0, 16.0), (0.0, 16.0)]), S as usize);
    let (r, _) = gpu::upload(
        ctx,
        spans,
        [0, 0, crossing.len() as u32, 1],
        &crossing,
        crossing.len() as u32,
        gpu::abi::upload_source::R8,
        gpu::abi::purpose::FIXTURE,
    );
    ensure(r == 0, || format!("crossing upload: {}", errno(r)))?;
    let all_px = [0, 0, S, S];
    let mut slowest = 0;
    for p in 0..pipeline::PROBE_INT {
        frame(ctx, &scene(t, BG, &[]))?;
        let mut s = Vec::new();
        encode::begin(&mut s, t);
        match p {
            pipeline::SOLID => encode::draw(&mut s, p, FG, &[all_px]),
            pipeline::COPY => encode::draw_sampled(&mut s, p, image, &[[0, 0, S, S, 0, 0]]),
            pipeline::COPY_SCALED => draw::encode(&mut s, p, 0, image, 0, 0, &[instance(all_px, &[0, 0, 0, 0, 1])]),
            pipeline::NEAREST_SCALED | pipeline::BOX_FILTER => {
                draw::encode(&mut s, p, 0, image, 0, 0, &[instance(all_px, &[0, 0, S, S, 0, 0, S, S])])
            }
            pipeline::OVER | pipeline::OVER_STRAIGHT => {
                draw::encode(&mut s, p, 0, image, 0, 0, &[instance(all_px, &[0, 0, 0, 0])])
            }
            pipeline::COVERAGE => draw::encode(&mut s, p, FG, mask, 0, 0, &[instance(all_px, &[0, 0, 0, 0, 0])]),
            pipeline::COVERAGE_SPANS => {
                draw::encode(&mut s, p, FG, 0, spans, crossing.len() as u32, &[instance(all_px, &[0, 0, S, S, 0])])
            }
            pipeline::BITMAP_TEXT => draw::encode(&mut s, p, FG, bits, 0, 0, &[instance(all_px, &[0, 0, 0, 0, 5, 7, 2, 0])]),
            pipeline::LINE => draw::encode(&mut s, p, FG, 0, 0, 0, &[instance(all_px, &[0, 0, 15, 15])]),
            pipeline::CIRCLE_FILL | pipeline::CIRCLE_OUTLINE => {
                draw::encode(&mut s, p, FG, 0, 0, 0, &[instance(all_px, &[8, 8, 6])])
            }
            pipeline::ROUNDED_RECT => draw::encode(&mut s, p, FG, 0, 0, 0, &[instance(all_px, &[0, 0, S, S, 4])]),
            pipeline::GRADIENT_V => draw::encode(&mut s, p, FG, 0, 0, 0, &[instance(all_px, &[0, S, 0xff102030u32 as i32])]),
            _ => draw::encode(&mut s, p, FG, 0, 0, 0, &[instance(all_px, &[0, 0, 15, 0, 0, 15])]),
        }
        let start = now_ns();
        let f = submit(ctx, &s)?;
        wait_ok(ctx, f, 2_000)?;
        let px = read_rect(t, [0, 0, S as u32, S as u32])?;
        let us = us_since(start);
        ensure(px.chunks_exact(4).any(|q| u32_at(q, 0) != BG), || format!("pipeline {p}'s first draw changed no pixel"))?;
        ensure(us <= FIRST_DRAW_US, || format!("pipeline {p}'s first draw in a warmed context took {us} us"))?;
        slowest = slowest.max(us);
    }
    value("slowest-us", slowest as i64, "us", Some((0, FIRST_DRAW_US as i64)));
    pass()
}

pub fn readback_time_2560() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, W, H)?;
    frame(ctx, &scene(t, FG, &[]))?;
    let start = now_ns();
    let px = read_rect(t, [0, 0, W, H])?;
    let us = us_since(start);
    value("readback-us", us as i64, "us", Some((0, READBACK_US as i64)));
    ensure(us <= READBACK_US, || format!("a full 2560x1600 readback took {us} us"))?;
    all(&px, FG, "the full readback")
}

pub fn upload_bandwidth() -> CaseResult {
    const S: u32 = 2048;
    let _case = begin()?;
    let ctx = context()?;
    let tex = gpu::texture_create(format::B8G8R8A8, S, S).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let t = target(format::B8G8R8A8, S, S)?;
    frame(ctx, &scene(t, BG, &[]))?;
    let mut want = Canvas::new(S, S);
    for (i, p) in want.px.chunks_exact_mut(4).enumerate() {
        let i = i as u32;
        p.copy_from_slice(&[(i * 13) as u8, (i >> 3) as u8, (i >> 11) as u8, 255]);
    }
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::draw_sampled(&mut s, pipeline::COPY, tex, &[[0, 0, S as i32, S as i32, 0, 0]]);
    let corner = [S - 16, S - 16, 16, 16];
    let start = now_ns();
    let (r, _) = gpu::upload(ctx, tex, [0, 0, S, S], &want.px, S * 4, gpu::abi::upload_source::B8G8R8A8, gpu::abi::purpose::IMAGE);
    ensure(r == 0, || format!("the 16 MiB upload returned {}", errno(r)))?;
    let f = submit(ctx, &s)?;
    wait_ok(ctx, f, 2_000)?;
    let px = read_rect(t, corner)?;
    let us = us_since(start);
    let mut c = Canvas::new(16, 16);
    for (y, row) in c.px.chunks_exact_mut(64).enumerate() {
        let from = (((S - 16 + y as u32) * S + S - 16) * 4) as usize;
        row.copy_from_slice(&want.px[from..from + 64]);
    }
    c.matches(&px, "the last uploaded rows, sampled by the next submission")?;
    value("upload-us", us as i64, "us", Some((0, UPLOAD_US as i64)));
    ensure(us <= UPLOAD_US, || format!("a 16 MiB upload took {us} us to be visible to the next submission"))?;
    expect(t, &want, "the whole sampled upload")?;
    pass()
}

/// The coverage pipeline's blend of `f` over `b` at coverage `a` (section 1.5's formula).
fn coverage(f: u32, b: u32, a: u8) -> u32 {
    let (f, b) = (f.to_le_bytes(), b.to_le_bytes());
    let mut out = 0xff000000;
    for i in 0..3 {
        out |= ((f[i] as u32 * a as u32 + b[i] as u32 * (255 - a as u32) + 127) / 255) << (8 * i);
    }
    out
}

pub fn coverage_throughput() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let mask: Vec<u8> = (0..W * H).map(|i| ((i % W) * 7 + (i / W) * 13) as u8).collect();
    let tex = texture(ctx, format::R8, W, H, &mask)?;
    let t = target(format::B8G8R8A8, W, H)?;
    frame(ctx, &scene(t, BG, &[]))?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    draw::encode(&mut s, pipeline::COVERAGE, FG, tex, 0, 0, &[instance([0, 0, W as i32, H as i32], &[0, 0, 0, 0, 0])]);
    let corner = [W - 16, H - 16, 16, 16];
    let start = now_ns();
    let f = submit(ctx, &s)?;
    wait_ok(ctx, f, 2_000)?;
    let px = read_rect(t, corner)?;
    let us = us_since(start);
    let mut want = Canvas::new(W, H);
    for (i, p) in want.px.chunks_exact_mut(4).enumerate() {
        p.copy_from_slice(&coverage(FG, BG, mask[i]).to_le_bytes());
    }
    let mut c = Canvas::new(16, 16);
    for (y, row) in c.px.chunks_exact_mut(64).enumerate() {
        let from = (((H - 16 + y as u32) * W + W - 16) * 4) as usize;
        row.copy_from_slice(&want.px[from..from + 64]);
    }
    c.matches(&px, "the blended corner")?;
    value("coverage-us", us as i64, "us", Some((0, COVERAGE_US as i64)));
    ensure(us <= COVERAGE_US, || format!("a full-screen 2560x1600 coverage blend took {us} us"))?;
    expect(t, &want, "the whole coverage blend")?;
    pass()
}

pub fn wait_no_spin() -> CaseResult {
    let _case = begin()?;
    // The gauge first: a 20 ms spin in this thread must show as CPU time.
    let (cpu, wall) = (thread_cpu_ns()?, now_ns());
    while now_ns() - wall < 20_000_000 {
        core::hint::spin_loop();
    }
    let spun = thread_cpu_ns()? - cpu;
    ensure(spun >= 10_000_000, || format!("a 20 ms spin read {spun} ns of thread CPU time"))?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, W, H)?;
    frame(ctx, &scene(t, BG, &[]))?;
    // Work the device takes long enough over that a spinning wait would show: 1x1
    // rectangles at distinct pixels, one draw each, more submissions in flight each round
    // until the wait lasts 50 ms; a shorter wait is not counted.
    for round in 0..8u32 {
        let draws = 4096usize << round.min(4);
        let in_flight = 1 + (round.saturating_sub(4) as usize).min(3);
        let mut s = Vec::new();
        encode::begin(&mut s, t);
        for i in 0..draws {
            let (x, y) = ((i % W as usize) as i32, (i / W as usize) as i32);
            encode::draw(&mut s, pipeline::SOLID, colour(round as u64), &[[x, y, x + 1, y + 1]]);
        }
        let mut fence = 0;
        for _ in 0..in_flight {
            fence = submit(ctx, &s)?;
        }
        let (r, _) = gpu::wait(ctx, fence, 0);
        if r != -ETIMEDOUT {
            ensure(r == 0, || format!("polling WAIT returned {}", errno(r)))?;
            continue;
        }
        let (cpu, wall) = (thread_cpu_ns()?, now_ns());
        wait_ok(ctx, fence, 2_000)?;
        let (cpu, wall) = (thread_cpu_ns()? - cpu, now_ns() - wall);
        let last = draws - 1;
        let p = read_rect(t, [(last % W as usize) as u32, (last / W as usize) as u32, 1, 1])?;
        all(&p, colour(round as u64), "the last rectangle after the wait")?;
        if wall < 50_000_000 {
            continue;
        }
        value("wait-us", (wall / 1_000) as i64, "us", None);
        value("cpu-us", (cpu / 1_000) as i64, "us", Some((0, (wall / 1_000 * WAIT_CPU_PERCENT / 100) as i64)));
        return ensure(cpu * 100 <= wall * WAIT_CPU_PERCENT, || {
            format!("waiting {} us on a fence used {} us of CPU time", wall / 1_000, cpu / 1_000)
        });
    }
    Err(fail_msg("no submission kept its fence outstanding for 50 ms, so no wait was measured".to_string()))
}

pub fn present_rate_2560() -> CaseResult {
    let _case = begin()?;
    take_display_2560()?;
    let ctx = context()?;
    let targets = [scanout_target(format::B8G8R8A8, W, H)?, scanout_target(format::B8G8R8A8, W, H)?];
    let colours = [FG, BG];
    for (t, c) in targets.iter().zip(colours) {
        frame(ctx, &scene(*t, c, &[]))?;
    }
    let start = now_ns();
    for n in 0..FRAMES as usize {
        present(ctx, targets[n % 2])?;
    }
    all(&read_rect(0, [W - 16, H - 16, 16, 16])?, colours[(FRAMES as usize - 1) % 2], "the scanout after the last present")?;
    let us = us_since(start);
    value("presents-per-second", (FRAMES * 1_000_000 / us.max(1)) as i64, "", Some((PRESENTS_PER_S as i64, i64::MAX)));
    ensure(rate_met(FRAMES, us, PRESENTS_PER_S), || {
        format!("{FRAMES} presents alternating two 2560x1600 targets took {us} us, under {PRESENTS_PER_S} a second")
    })
}
