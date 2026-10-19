//! G5 expectations from the contract's tables and formulas, independently of TGSI.
use super::common::*;
use gpu::abi::draw::{self, Instance};
use libbreenix::gpu;
use libbreenix::suite::{value, CaseResult};
type R<T> = Result<T, libbreenix::suite::CaseError>;
const BG: u32 = 0xff312719;
const FG: u32 = 0xffb76321;
fn instance(rect: [i32; 4], params: &[i32]) -> Instance {
    let mut a = [0; 16];
    for (d, &v) in a.iter_mut().zip(params) {
        *d = v as u32;
    }
    Instance { rect, params: a }
}
fn draw_frame(
    ctx: u64,
    t: u64,
    p: u32,
    colour: u32,
    tex: u64,
    instances: &[Instance],
) -> CaseResult {
    let mut s = scene(t, BG, &[]);
    draw::encode(&mut s, p, colour, tex, 0, 0, instances);
    frame(ctx, &s)?;
    pass()
}
fn tex(ctx: u64, fmt: u32, w: u32, h: u32, bytes: &[u8]) -> R<u64> {
    let t =
        gpu::texture_create(fmt, w, h).map_err(|e| fail_msg(format!("texture: {}", errno(e))))?;
    let source = match fmt {
        format::B8G8R8A8 => gpu::abi::upload_source::B8G8R8A8,
        format::R8G8B8A8 => gpu::abi::upload_source::R8G8B8A8,
        _ => gpu::abi::upload_source::R8,
    };
    let (r, _) = gpu::upload(
        ctx,
        t,
        [0, 0, w, h],
        bytes,
        w * format::bytes(fmt),
        source,
        gpu::abi::purpose::FIXTURE,
    );
    ensure(r == 0, || format!("fixture upload: {}", errno(r)))?;
    Ok(t)
}
fn put(c: &mut Canvas, x: u32, y: u32, colour: u32) {
    let at = ((y * c.w + x) * 4) as usize;
    c.px[at..at + 4].copy_from_slice(&colour.to_le_bytes());
}
fn pixel(c: &Canvas, x: u32, y: u32) -> u32 {
    u32_at(&c.px, ((y * c.w + x) * 4) as usize)
}
fn rounded_div255(x: u32) -> u32 {
    (x + 127) / 255
}
fn over(source: u32, dest: u32, straight: bool) -> u32 {
    let mut s = source.to_le_bytes().map(u32::from);
    let d = dest.to_le_bytes().map(u32::from);
    if straight {
        for i in 0..3 {
            s[i] = rounded_div255(s[i] * s[3]);
        }
    }
    let mut out = 0;
    for i in 0..4 {
        out |= (s[i] + rounded_div255(d[i] * (255 - s[3]))) << (8 * i);
    }
    out
}
fn coverage(f: u32, b: u32, a: [u8; 3]) -> u32 {
    let f = f.to_le_bytes();
    let b = b.to_le_bytes();
    let mut out = 0xff000000;
    for i in 0..3 {
        let a = a[i] as u32;
        out |= rounded_div255(f[i] as u32 * a + b[i] as u32 * (255 - a)) << (8 * i);
    }
    out
}
fn expect_draw(
    ctx: u64,
    t: u64,
    w: u32,
    h: u32,
    mut formula: impl FnMut(u32, u32) -> u32,
    what: &str,
) -> CaseResult {
    let mut c = Canvas::new(w, h);
    for y in 0..h {
        for x in 0..w {
            put(&mut c, x, y, formula(x, y));
        }
    }
    observe(ctx, t, &c, what)?;
    pass()
}

fn observe(ctx: u64, t: u64, want: &Canvas, what: &str) -> R<[u8; record::SIZE]> {
    let rec = super::common::expect(t, want, what)?;
    libbreenix::graphics::take_over_display()
        .map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0, || format!("PRESENT: {}", errno(r)))?;
    wait_ok(ctx, p.fence, 2_000)?;
    Ok(rec)
}

pub fn solid_rects() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 19, 13)?;
    let rects = [
        ([-9, -7, 4, 3], FG),
        ([3, 2, 17, 11], 0x12345678),
        ([18, 0, 19, 13], FG),
        ([2, 5, 2, 10], 0),
    ];
    frame(ctx, &scene(t, BG, &rects))?;
    observe(
        ctx,
        t,
        &scene_canvas(19, 13, BG, &rects),
        "half-open rectangles",
    )?;
    pass()
}
pub fn rect_batch() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 64, 32)?;
    let rects: Vec<_> = (0..100)
        .map(|i| {
            [
                (i * 17 % 71) - 5,
                (i * 11 % 41) - 4,
                (i * 17 % 71) + 2,
                (i * 11 % 41) + 1,
            ]
        })
        .collect();
    let mut s = scene(t, BG, &[]);
    encode::draw(&mut s, pipeline::SOLID, FG, &rects);
    frame(ctx, &s)?;
    let a = read(t, 64, 32)?.0;
    let mut s = scene(t, BG, &[]);
    for r in &rects {
        encode::draw(&mut s, pipeline::SOLID, FG, &[*r]);
    }
    frame(ctx, &s)?;
    let mut expected = scene_canvas(64, 32, BG, &[]);
    for r in rects {
        expected.rect(r, FG);
    }
    observe(ctx, t, &expected, "unbatched rectangles")?;
    ensure(a == expected.px, || {
        "batched pixels differ from the literal rectangle set".to_string()
    })
}
pub fn copy_clip() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let source: Vec<u8> = (0..8 * 6)
        .flat_map(|i| [i as u8, (2 * i) as u8, (3 * i) as u8, 255])
        .collect();
    let texture = tex(ctx, format::B8G8R8A8, 8, 6, &source)?;
    let t = scanout_target(format::R8G8B8A8, 9, 7)?;
    let mut s = scene(t, BG, &[]);
    s.extend([8, 6, 1, 2, 7, 6]);
    encode::draw_sampled(&mut s, pipeline::COPY, texture, &[[-3, -1, 12, 12, 0, 0]]);
    frame(ctx, &s)?;
    expect_draw(
        ctx,
        t,
        9,
        7,
        |x, y| {
            if (1..5).contains(&x) && (2..5).contains(&y) {
                u32_at(&source, (((y + 1) * 8 + x + 3) * 4) as usize)
            } else {
                BG
            }
        },
        "copy source, target and scissor clipping",
    )
}
pub fn copy_scaled() -> CaseResult {
    scale(pipeline::COPY_SCALED)
}
pub fn nearest_scale() -> CaseResult {
    scale(pipeline::NEAREST_SCALED)
}
pub fn box_filter() -> CaseResult {
    scale(pipeline::BOX_FILTER)
}
fn scale(p: u32) -> CaseResult {
    let _case = begin()?;
    scale_at(p,0,0,13,11)?;
    scale_at(p,2,3,13,11)?;
    if p == pipeline::BOX_FILTER { scale_at(p,1,2,7,5)?; }
    pass()
}
fn scale_at(p: u32, sx: u32, sy: u32, sw: u32, sh: u32) -> CaseResult {
    let ctx = context()?;
    let tw = sw+sx;
    let source: Vec<u8> = (0..tw * (sh+sy))
        .flat_map(|i| [(i * 13) as u8, (i * 37) as u8, (i * 71) as u8, if sx==0 {255} else {(i*19) as u8}])
        .collect();
    let texture = tex(ctx, format::B8G8R8A8, tw, sh+sy, &source)?;
    let t = scanout_target(format::R8G8B8A8, 43, 37)?;
    let (w, h) = if p == pipeline::COPY_SCALED {
        (sw as i32*3, sh as i32*3)
    } else if p == pipeline::BOX_FILTER {
        (5, 4)
    } else {
        (29, 23)
    };
    let params = if p == pipeline::COPY_SCALED {
        vec![2, 3, sx as i32, sy as i32, 3]
    } else {
        vec![2, 3, w, h, sx as i32, sy as i32, sw as i32, sh as i32]
    };
    draw_frame(
        ctx,
        t,
        p,
        0,
        texture,
        &[instance([2, 3, 2 + w, 3 + h], &params)],
    )?;
    expect_draw(
        ctx,
        t,
        43,
        37,
        |x, y| {
            if x < 2 || y < 3 || x >= 2 + w as u32 || y >= 3 + h as u32 {
                return BG;
            }
            let (x, y) = (x - 2, y - 3);
            if p == pipeline::COPY_SCALED {
                return u32_at(&source, (((sy+y / 3) * tw + sx+x / 3) * 4) as usize);
            }
            if p == pipeline::NEAREST_SCALED {
                return u32_at(
                    &source,
                    (((sy+y * sh / h as u32) * tw + sx+x * sw / w as u32) * 4) as usize,
                );
            }
            let (x0, x1, y0, y1) = (
                sx + x * sw / w as u32,
                sx + (x + 1) * sw / w as u32,
                sy + y * sh / h as u32,
                sy + (y + 1) * sh / h as u32,
            );
            let n = (x1 - x0) * (y1 - y0);
            let mut sum = [0; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    for k in 0..4 {
                        sum[k] += source[((yy * tw + xx) * 4) as usize + k] as u32;
                    }
                }
            }
            let mut word = 0xff000000;
            for k in 0..3 {
                word |= ((sum[k] + n / 2) / n) << (8 * k);
            }
            word
        },
        "integer texture scaling",
    )
}
pub fn over_premultiplied() -> CaseResult {
    alpha(false)
}
pub fn over_straight() -> CaseResult {
    alpha(true)
}
fn alpha(straight: bool) -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 256, 256)?;
    let mut source = Vec::with_capacity(256 * 256 * 4);
    for a in 0..256u32 {
        for c in 0..256u32 {
            let colour = if straight { c } else { rounded_div255(c * a) };
            source.extend([colour as u8, colour as u8, colour as u8, a as u8]);
        }
    }
    let texture = tex(ctx, format::B8G8R8A8, 256, 256, &source)?;
    for background in [0u32, 0xff000000, 0x80402010, 0xffffffff] {
        let mut s = scene(t, background, &[]);
        draw::encode(
            &mut s,
            if straight {
                pipeline::OVER_STRAIGHT
            } else {
                pipeline::OVER
            },
            0,
            texture,
            0,
            0,
            &[instance([0, 0, 256, 256], &[0, 0, 0, 0])],
        );
        frame(ctx, &s)?;
        expect_draw(
            ctx,
            t,
            256,
            256,
            |x, y| {
                over(
                    u32_at(&source, ((y * 256 + x) * 4) as usize),
                    background,
                    straight,
                )
            },
            "65536 colour/alpha pairs",
        )?;
    }
    pass()
}
pub fn over_overlap() -> CaseResult {
    let _case = begin()?;
    let mut ctx = bgl::Context::new().map_err(|e| fail_msg(errno(e)))?;
    let source = 0x80602010u32;
    let texture = tex(
        ctx.handle(),
        format::B8G8R8A8,
        8,
        8,
        &source.to_le_bytes().repeat(64),
    )?;
    let t = scanout_target(format::B8G8R8A8, 12, 12)?;
    let instances = [
        instance([1, 1, 9, 9], &[1, 1, 0, 0]),
        instance([4, 4, 12, 12], &[4, 4, 0, 0]),
        instance([0, 10, 2, 12], &[0, 10, 0, 0]),
    ];
    ctx.begin(t, 12, 12);
    ctx.clear(BG);
    ctx.draw(pipeline::OVER, 0, texture, 0, 0, &instances);
    let commands = ctx.encoded();
    let mut at = 0;
    let mut draws = 0;
    while at < commands.len() {
        if commands[at] == 9 {
            draws += 1;
        }
        at += commands[at + 1] as usize;
    }
    ensure(draws == 2, || {
        format!("BGL emitted {draws} draws instead of splitting at the first overlap")
    })?;
    ctx.submit()
        .map_err(|e| fail_msg(errno(e)))?
        .wait(2_000_000_000)
        .map_err(|e| fail_msg(errno(e)))?;
    let mut want = scene_canvas(12, 12, BG, &[]);
    for i in instances {
        for y in i.rect[1]..i.rect[3] {
            for x in i.rect[0]..i.rect[2] {
                let d = pixel(&want, x as u32, y as u32);
                put(&mut want, x as u32, y as u32, over(source, d, false));
            }
        }
    }
    observe(ctx.handle(), t, &want, "sequential blended overlap")?;
    let mut bad = scene(t, 0, &[]);
    draw::encode(&mut bad, pipeline::OVER, 0, texture, 0, 0, &instances);
    let (r, a) = gpu::submit(ctx.handle(), &bad);
    ensure(r == -EINVAL && a.error_index == 2 && a.error == 20, || {
        format!("overlap returned {r} at {} code {}", a.error_index, a.error)
    })?;
    observe(ctx.handle(), t, &want, "atomic overlap refusal")?;
    pass()
}
pub fn coverage_blend() -> CaseResult {
    mask(false)
}
pub fn coverage_lcd() -> CaseResult {
    mask(true)
}
fn mask(lcd: bool) -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 256, 4)?;
    let mut source = Vec::new();
    for _ in 0..4 {
        for x in 0..256u32 {
            if lcd {
                source.extend([x as u8, (255 - x) as u8, (x * 37) as u8, 255]);
            } else {
                source.push(x as u8);
            }
        }
    }
    let texture = tex(
        ctx,
        if lcd { format::B8G8R8A8 } else { format::R8 },
        256,
        4,
        &source,
    )?;
    draw_frame(
        ctx,
        t,
        pipeline::COVERAGE,
        FG,
        texture,
        &[instance([0, 0, 256, 4], &[0, 0, 0, 0, i32::from(lcd)])],
    )?;
    expect_draw(
        ctx,
        t,
        256,
        4,
        |x, _| {
            coverage(
                FG,
                BG,
                if lcd {
                    [x as u8, (255 - x) as u8, (x * 37) as u8]
                } else {
                    [x as u8; 3]
                },
            )
        },
        "mask coverage formula",
    )
}

pub fn line_bresenham() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 12, 12)?;
    let tables: [([i32; 4], &[(u32, u32)]); 8] = [
        (
            [0, 0, 7, 3],
            &[
                (0, 0),
                (1, 0),
                (2, 1),
                (3, 1),
                (4, 2),
                (5, 2),
                (6, 3),
                (7, 3),
            ],
        ),
        (
            [7, 3, 0, 0],
            &[
                (7, 3),
                (6, 3),
                (5, 2),
                (4, 2),
                (3, 1),
                (2, 1),
                (1, 0),
                (0, 0),
            ],
        ),
        (
            [0, 0, 3, 7],
            &[
                (0, 0),
                (0, 1),
                (1, 2),
                (1, 3),
                (2, 4),
                (2, 5),
                (3, 6),
                (3, 7),
            ],
        ),
        (
            [3, 7, 0, 0],
            &[
                (3, 7),
                (3, 6),
                (2, 5),
                (2, 4),
                (1, 3),
                (1, 2),
                (0, 1),
                (0, 0),
            ],
        ),
        (
            [0, 0, 5, 5],
            &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)],
        ),
        (
            [0, 5, 5, 0],
            &[(0, 5), (1, 4), (2, 3), (3, 2), (4, 1), (5, 0)],
        ),
        (
            [0, 0, 9, 2],
            &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 1),
                (4, 1),
                (5, 1),
                (6, 1),
                (7, 2),
                (8, 2),
                (9, 2),
            ],
        ),
        (
            [2, 9, 0, 0],
            &[
                (2, 9),
                (2, 8),
                (2, 7),
                (1, 6),
                (1, 5),
                (1, 4),
                (1, 3),
                (0, 2),
                (0, 1),
                (0, 0),
            ],
        ),
    ];
    for (a, points) in tables {
        draw_frame(
            ctx,
            t,
            pipeline::LINE,
            FG,
            0,
            &[instance([0, 0, 12, 12], &a)],
        )?;
        expect_draw(
            ctx,
            t,
            12,
            12,
            |x, y| if points.contains(&(x, y)) { FG } else { BG },
            "literal line table",
        )?;
    }
    pass()
}
pub fn line_clip() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 17, 13)?;
    for a in [
        [-27i32, -13, 45, 28],
        [39, -8, -19, 25],
        [-30, 6, 31, 6],
        [7, -90, 7, 80],
    ] {
        let mut points = Vec::new();
        let [mut x, mut y, x1, y1] = a;
        let dx = (x1 - x).abs();
        let dy = -(y1 - y).abs();
        let sx = if x < x1 { 1 } else { -1 };
        let sy = if y < y1 { 1 } else { -1 };
        let mut e = dx + dy;
        loop {
            points.push((x, y));
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * e;
            if e2 >= dy {
                e += dy;
                x += sx;
            }
            if e2 <= dx {
                e += dx;
                y += sy;
            }
        }
        let mut s = scene(t, BG, &[]);
        s.extend([8, 6, 2, 1, 15, 11]);
        draw::encode(
            &mut s,
            pipeline::LINE,
            FG,
            0,
            0,
            0,
            &[instance([0, 0, 17, 13], &a)],
        );
        frame(ctx, &s)?;
        expect_draw(
            ctx,
            t,
            17,
            13,
            |x, y| {
                if (2..15).contains(&x)
                    && (1..11).contains(&y)
                    && points.contains(&(x as i32, y as i32))
                {
                    FG
                } else {
                    BG
                }
            },
            "clipped unclipped line walk",
        )?;
    }
    pass()
}
pub fn line_extreme() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 17, 13)?;
    for (a, kind) in [
        ([i32::MIN, i32::MIN, i32::MAX, i32::MAX], 0),
        ([i32::MAX, i32::MAX, i32::MIN, i32::MIN], 0),
        ([i32::MIN, 5, i32::MAX, 5], 1),
        ([7, i32::MAX, 7, i32::MIN], 2),
        ([i32::MIN, i32::MAX, i32::MAX, i32::MIN], 3),
        ([i32::MIN, i32::MAX, i32::MAX, i32::MIN+2], 4),
    ] {
        draw_frame(
            ctx,
            t,
            pipeline::LINE,
            FG,
            0,
            &[instance([0, 0, 17, 13], &a)],
        )?;
        expect_draw(
            ctx,
            t,
            17,
            13,
            |x, y| {
                if match kind {
                    0 => x == y,
                    1 => y == 5,
                    2 => x == 7,
                    4 => x == 0 && y == 0,
                    _ => false,
                } {
                    FG
                } else {
                    BG
                }
            },
            "extreme line literal pixel set",
        )?;
    }
    pass()
}
pub fn circle_fill() -> CaseResult {
    circles(false)
}
pub fn circle_outline() -> CaseResult {
    circles(true)
}
fn circles(outline: bool) -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 83, 83)?;
    for r in -1..=40i32 {
        let mut points = Vec::new();
        if outline && r >= 0 {
            for (y, &x) in CIRCLE_OCTANTS[r as usize].iter().enumerate() {
                for (dx, dy) in [(x, y as i32), (y as i32, x)] {
                    for sx in [-1, 1] {
                        for sy in [-1, 1] {
                            points.push((41 + sx * dx, 41 + sy * dy));
                        }
                    }
                }
            }
        }
        draw_frame(
            ctx,
            t,
            if outline {
                pipeline::CIRCLE_OUTLINE
            } else {
                pipeline::CIRCLE_FILL
            },
            FG,
            0,
            &[instance([0, 0, 83, 83], &[41, 41, r])],
        )?;
        expect_draw(
            ctx,
            t,
            83,
            83,
            |x, y| {
                if if outline {
                    points.contains(&(x as i32, y as i32))
                } else {
                    let dx = x as i64 - 41;
                    let dy = y as i64 - 41;
                    r >= 0 && dx * dx + dy * dy <= (r as i64) * (r as i64)
                } {
                    FG
                } else {
                    BG
                }
            },
            "contract circle pixel set",
        )?;
    }
    draw_frame(
        ctx,
        t,
        if outline {
            pipeline::CIRCLE_OUTLINE
        } else {
            pipeline::CIRCLE_FILL
        },
        FG,
        0,
        &[instance([0, 0, 83, 83], &[i32::MAX, 0, i32::MAX])],
    )?;
    expect_draw(
        ctx,
        t,
        83,
        83,
        |x, y| {
            let dx = x as i128 - i32::MAX as i128;
            let dy = y as i128;
            let r = i32::MAX as i128;
            let hit = if outline {
                x == 0
            } else {
                dx * dx + dy * dy <= r * r
            };
            if hit {
                FG
            } else {
                BG
            }
        },
        "extreme circle actual drawing",
    )
}
pub fn rounded_rect() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 31, 23)?;
    for a in [
        [2, 3, 25, 17, 5],
        [-5, -3, 27, 25, 100],
        [1, 1, 25, 17, 0],
        [0, 0, 0, 20, 5],
        [i32::MIN + 10, i32::MIN + 10, i32::MAX, i32::MAX, i32::MAX],
    ] {
        draw_frame(
            ctx,
            t,
            pipeline::ROUNDED_RECT,
            FG,
            0,
            &[instance([0, 0, 31, 23], &a)],
        )?;
        expect_draw(
            ctx,
            t,
            31,
            23,
            |x, y| {
                let [ox, oy, w, h, r] = a.map(i64::from);
                let (x, y) = (x as i64 - ox, y as i64 - oy);
                let r = r.max(0).min(w / 2).min(h / 2);
                let inside = if x < 0 || y < 0 || x >= w || y >= h || w <= 0 || h <= 0 {
                    false
                } else if r <= 0 || ((r..w - r).contains(&x)) || ((r..h - r).contains(&y)) {
                    true
                } else {
                    let cx = if x < r { r } else { w - r };
                    let cy = if y < r { r } else { h - r };
                    let dx = 2 * x + 1 - 2 * cx;
                    let dy = 2 * y + 1 - 2 * cy;
                    (dx as i128) * (dx as i128) + (dy as i128) * (dy as i128)
                        <= 4 * (r as i128) * (r as i128)
                };
                if inside {
                    FG
                } else {
                    BG
                }
            },
            "rounded rectangle formula",
        )?;
    }
    pass()
}
pub fn gradient() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 7, 67)?;
    let bottom = 0x3a17e2f9u32;
    for h in [1, 2, 3, 4, 7, 16, 31, 65, i32::MAX] {
        draw_frame(
            ctx,
            t,
            pipeline::GRADIENT_V,
            FG,
            0,
            &[instance([1, 1, 6, 67], &[1, h, bottom as i32])],
        )?;
        expect_draw(
            ctx,
            t,
            7,
            67,
            |x, y| {
                if !(1..6).contains(&x) || y < 1 || y as i64 >= 1 + h as i64 {
                    return BG;
                }
                let i = (y - 1) as u64;
                let n = (h - 1) as u64;
                if n == 0 {
                    return FG;
                }
                let mut out = 0;
                let a = FG.to_le_bytes();
                let b = bottom.to_le_bytes();
                for k in 0..4 {
                    out |=
                        (((a[k] as u64 * (n - i) + b[k] as u64 * i + n / 2) / n) as u32) << (8 * k);
                }
                out
            },
            "gradient integer interpolation",
        )?;
    }
    pass()
}
pub fn triangle_integer() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 17, 13)?;
    for mut a in [
        [1, 1, 15, 1, 1, 11],
        [1, 11, 15, 1, 1, 1],
        [-20, -10, 28, 3, 6, 30],
        [0, 0, 3, 3, 6, 6],
        [i32::MIN, i32::MIN, i32::MAX, i32::MIN, i32::MAX, i32::MAX],
    ] {
        draw_frame(
            ctx,
            t,
            pipeline::TRIANGLE,
            FG,
            0,
            &[instance([0, 0, 17, 13], &a)],
        )?;
        let area = (a[2] as i128 - a[0] as i128) * (a[5] as i128 - a[1] as i128)
            - (a[3] as i128 - a[1] as i128) * (a[4] as i128 - a[0] as i128);
        if area < 0 {
            a.swap(2, 4);
            a.swap(3, 5);
        }
        expect_draw(
            ctx,
            t,
            17,
            13,
            |x, y| {
                let mut inside = area != 0;
                for (i, j) in [(0, 2), (2, 4), (4, 0)] {
                    let dx = a[j] as i128 - a[i] as i128;
                    let dy = a[j + 1] as i128 - a[i + 1] as i128;
                    let e = dx * (2 * y as i128 + 1 - 2 * a[i + 1] as i128)
                        - dy * (2 * x as i128 + 1 - 2 * a[i] as i128);
                    inside &= e > 0 || (e == 0 && (dy < 0 || (dy == 0 && dx > 0)));
                }
                if inside {
                    FG
                } else {
                    BG
                }
            },
            "integer edge-function triangle",
        )?;
    }
    pass()
}
pub fn clip_nested() -> CaseResult {
    let _case = begin()?;
    let mut c = bgl::Context::new().map_err(|e| fail_msg(errno(e)))?;
    let t = scanout_target(format::B8G8R8A8, 20, 17)?;
    c.begin(t, 20, 17);
    c.clear(BG);
    c.push_clip([2, 3, 18, 16]);
    c.solid(FG, &[[0, 0, 20, 17]]);
    c.push_clip([7, 0, 14, 11]);
    c.solid(0xff102030, &[[0, 0, 20, 17]]);
    c.pop_clip();
    c.solid(0xff908070, &[[3, 12, 5, 15]]);
    c.pop_clip();
    c.solid(0xffabcdef, &[[0, 0, 1, 1]]);
    c.submit()
        .map_err(|e| fail_msg(errno(e)))?
        .wait(2_000_000_000)
        .map_err(|e| fail_msg(errno(e)))?;
    let mut expected = scene_canvas(20, 17, BG, &[]);
    expected.rect([2, 3, 18, 16], FG);
    expected.rect([7, 3, 14, 11], 0xff102030);
    expected.rect([3, 12, 5, 15], 0xff908070);
    expected.rect([0, 0, 1, 1], 0xffabcdef);
    observe(c.handle(), t, &expected, "BGL nested clips and restoration")?;
    pass()
}
pub fn order() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 37, 29)?;
    let draws: Vec<_> = (0..200)
        .map(|i| {
            (
                [i % 19, i % 13, i % 19 + 18, i % 13 + 16],
                0x80112233u32.wrapping_mul(i as u32 + 1),
            )
        })
        .collect();
    frame(ctx, &scene(t, BG, &draws))?;
    expect(
        t,
        &scene_canvas(37, 29, BG, &draws),
        "unblended primitive order",
    )?;
    pass()
}

pub fn bitmap_text() -> CaseResult {
    use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 67, 61)?;
    // §8.1's literal A table, followed by the independent crate's 16 px A cells.
    let rows = [0x0eu8, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11];
    let small = tex(ctx, format::R8, 1, 7, &rows)?;
    let raster = get_raster('A', FontWeight::Regular, RasterHeight::Size16)
        .ok_or_else(|| fail_msg("reference bitmap A missing".to_string()))?;
    let (rw, rh) = (raster.width() as u32, raster.height() as u32);
    let cells: Vec<u8> = raster
        .raster()
        .iter()
        .flat_map(|r| r.iter().copied())
        .collect();
    let large = tex(ctx, format::R8, rw, rh, &cells)?;
    for scale in [1, 2, 3] {
        draw_frame(
            ctx,
            t,
            pipeline::BITMAP_TEXT,
            FG,
            small,
            &[instance(
                [2, 3, 2 + 5 * scale, 3 + 7 * scale],
                &[2, 3, 0, 0, 5, 7, scale, 0],
            )],
        )?;
        expect_draw(
            ctx,
            t,
            67,
            61,
            |x, y| {
                if x >= 2
                    && x < 2 + 5 * scale as u32
                    && y >= 3
                    && y < 3 + 7 * scale as u32
                    && rows[((y - 3) / scale as u32) as usize] & (0x10 >> ((x - 2) / scale as u32))
                        != 0
                {
                    FG
                } else {
                    BG
                }
            },
            "literal 5x7 bitmap A",
        )?;
        draw_frame(
            ctx,
            t,
            pipeline::BITMAP_TEXT,
            FG,
            large,
            &[instance(
                [2, 3, 2 + rw as i32 * scale, 3 + rh as i32 * scale],
                &[2, 3, 0, 0, rw as i32, rh as i32, scale, 1],
            )],
        )?;
        expect_draw(
            ctx,
            t,
            67,
            61,
            |x, y| {
                if x < 2 || x >= 2 + rw * scale as u32 || y < 3 || y >= 3 + rh * scale as u32 {
                    BG
                } else {
                    let a =
                        cells[(((y - 3) / scale as u32) * rw + (x - 2) / scale as u32) as usize];
                    coverage(FG, BG, [a; 3])
                }
            },
            "16 px bitmap cells at integer scales",
        )?;
    }
    let mut atlas = vec![0u8; ((rw+3)*(rh+2)) as usize];
    for y in 0..rh as usize {
        atlas[(y+2)*(rw as usize+3)+3..(y+2)*(rw as usize+3)+3+rw as usize]
            .copy_from_slice(&cells[y*rw as usize..(y+1)*rw as usize]);
    }
    let atlas = tex(ctx,format::R8,rw+3,rh+2,&atlas)?;
    draw_frame(ctx,t,pipeline::BITMAP_TEXT,FG,atlas,&[instance(
        [2,3,2+rw as i32,3+rh as i32], &[2,3,3,2,rw as i32,rh as i32,1,1])])?;
    expect_draw(ctx,t,67,61,|x,y| {
        if x>=2 && x<2+rw && y>=3 && y<3+rh {
            coverage(FG,BG,[cells[((y-3)*rw+x-2) as usize];3])
        } else {BG}
    },"nonzero bitmap atlas origin")?;
    let mut bits=vec![0u8;27];
    for y in 0..7 {bits[(y+2)*3+1]=rows[y];}
    let bits=tex(ctx,format::R8,3,9,&bits)?;
    draw_frame(ctx,t,pipeline::BITMAP_TEXT,FG,bits,&[instance([2,3,7,10],&[2,3,1,2,5,7,1,0])])?;
    expect_draw(ctx,t,67,61,|x,y| {
        if (2..7).contains(&x) && (3..10).contains(&y) && rows[(y-3) as usize] & (0x10>>(x-2)) != 0 {FG} else {BG}
    },"nonzero bit-row atlas origin")?;
    pass()
}
use libfont::outline::LineSegment;
pub(super) fn edges(points: &[(f32, f32)]) -> Vec<LineSegment> {
    (0..points.len())
        .map(|i| {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            LineSegment {
                x0: a.0,
                y0: a.1,
                x1: b.0,
                y1: b.1,
            }
        })
        .collect()
}
/// Geometry only: published crossing formula and stable x sort. No pixels or
/// expected coverage enter the submission's data buffer.
pub(super) fn crossings(segments: &[LineSegment], height: usize) -> Vec<u8> {
    let mut offsets = vec![0u32];
    let mut data: Vec<(u32, u32)> = Vec::new();
    for row in 0..height {
        for sub in 0..8 {
            let y = row as f32 + (sub as f32 + 0.5) * 0.125;
            let mut line = Vec::new();
            for e in segments {
                if (e.y0 < y && e.y1 < y) || (e.y0 >= y && e.y1 >= y) {
                    continue;
                }
                let dy = e.y1 - e.y0;
                if dy == 0.0 {
                    continue;
                }
                let t = (y - e.y0) / dy;
                line.push((e.x0 + t * (e.x1 - e.x0), if e.y1 > e.y0 { 1 } else { -1 }));
            }
            line.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            data.extend(line.into_iter().map(|(x, w)| (x.to_bits(), w as u32)));
            offsets.push(data.len() as u32);
        }
    }
    let mut bytes = Vec::new();
    for o in offsets {
        bytes.extend(o.to_le_bytes());
    }
    for (x, w) in data {
        bytes.extend(x.to_le_bytes());
        bytes.extend(w.to_le_bytes());
    }
    bytes
}
fn spans_draw(
    ctx: u64,
    t: u64,
    buffer: u64,
    w: u32,
    h: u32,
    segments: &[LineSegment],
) -> CaseResult {
    let bytes = crossings(segments, h as usize);
    let (r, _) = gpu::upload(
        ctx,
        buffer,
        [0, 0, bytes.len() as u32, 1],
        &bytes,
        bytes.len() as u32,
        gpu::abi::upload_source::R8,
        gpu::abi::purpose::FIXTURE,
    );
    ensure(r == 0, || format!("crossing-buffer upload: {}", errno(r)))?;
    let mut s = scene(t, BG, &[]);
    draw::encode(
        &mut s,
        pipeline::COVERAGE_SPANS,
        FG,
        0,
        buffer,
        bytes.len() as u32,
        &[instance(
            [0, 0, w as i32, h as i32],
            &[0, 0, w as i32, h as i32, 0],
        )],
    );
    frame(ctx, &s)?;
    pass()
}
fn reference(ctx: u64, t: u64, w: u32, h: u32, segments: &[LineSegment]) -> CaseResult {
    let raster = super::expectations::rasterize(segments, w as usize, h as usize, 0, 0)
        .map_err(|e| fail_msg(format!("reference rasterizer: {e}")))?;
    expect_draw(
        ctx,
        t,
        w,
        h,
        |x, y| coverage(FG, BG, [raster.coverage[(y * w + x) as usize]; 3]),
        "preserved reference coverage",
    )
}
pub fn coverage_spans_square() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 4, 4)?;
    let buffer = gpu::buffer_create(4096).map_err(|e| fail_msg(errno(e)))?;
    let segments = edges(&[(0.25, 0.25), (3.75, 0.25), (3.75, 3.75), (0.25, 3.75)]);
    spans_draw(ctx, t, buffer, 4, 4, &segments)?;
    let table = [
        [143, 191, 191, 143],
        [191, 255, 255, 191],
        [191, 255, 255, 191],
        [143, 191, 191, 143],
    ];
    expect_draw(
        ctx,
        t,
        4,
        4,
        |x, y| coverage(FG, BG, [table[y as usize][x as usize]; 3]),
        "literal fractional-square coverage table",
    )?;
    let t=scanout_target(format::B8G8R8A8,12,4)?;
    let second=gpu::buffer_create(4096).map_err(|e| fail_msg(errno(e)))?;
    let whole=edges(&[(0.0,0.0),(4.0,0.0),(4.0,4.0),(0.0,4.0)]);
    let data=[crossings(&segments,4),crossings(&whole,4)];
    for (h,bytes) in [(buffer,&data[0]),(second,&data[1])] {
        let (r,_)=gpu::upload(ctx,h,[0,0,bytes.len() as u32,1],bytes,bytes.len() as u32,
            gpu::abi::upload_source::R8,gpu::abi::purpose::FIXTURE);
        ensure(r==0,||format!("packed crossing upload: {}",errno(r)))?;
    }
    let mut s=scene(t,BG,&[]);
    for (x,h,bytes) in [(0,buffer,&data[0]),(4,second,&data[1]),(8,buffer,&data[0])] {
        draw::encode(&mut s,pipeline::COVERAGE_SPANS,FG,0,h,bytes.len() as u32,
            &[instance([x,0,x+4,4],&[x,0,4,4,0])]);
    }
    frame(ctx,&s)?;
    expect_draw(ctx,t,12,4,|x,y| {
        if (4..8).contains(&x) {FG}
        else {coverage(FG,BG,[table[y as usize][(x%4) as usize];3])}
    },"distinct and repeated crossing buffers share one data texture")
}
pub fn coverage_spans_winding() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 17, 13)?;
    let buffer = gpu::buffer_create(16384).map_err(|e| fail_msg(errno(e)))?;
    let outer = edges(&[(-2.25, -1.75), (15.5, 0.25), (16.125, 12.75), (0.5, 11.625)]);
    let mut hole = outer.clone();
    hole.extend(edges(&[
        (3.125, 3.25),
        (4.5, 9.75),
        (12.875, 8.625),
        (11.75, 2.5),
    ]));
    let mut same = outer.clone();
    same.extend(edges(&[
        (3.125, 3.25),
        (11.75, 2.5),
        (12.875, 8.625),
        (4.5, 9.75),
    ]));
    let crossing = edges(&[(1.25, 1.5), (15.75, 11.25), (0.5, 11.5), (15.125, 0.75)]);
    let open = vec![LineSegment {
        x0: 1.125,
        y0: -1.0,
        x1: 4.75,
        y1: 15.0,
    }];
    for s in [outer, hole, same, crossing, open] {
        spans_draw(ctx, t, buffer, 17, 13, &s)?;
        reference(ctx, t, 17, 13, &s)?;
    }
    pass()
}
pub fn coverage_spans_corpus() -> CaseResult {
    let _case = begin()?;
    let start = now_ns();
    let ctx = context()?;
    let t = scanout_target(format::B8G8R8A8, 72, 72)?;
    let buffer = gpu::buffer_create(256 * 1024).map_err(|e| fail_msg(errno(e)))?;
    let mut glyphs = 0;
    for name in ["DejaVuSansMono.ttf", "DejaVuSans.ttf"] {
        let bytes = std::fs::read(format!("/usr/share/fonts/{name}"))
            .map_err(|e| fail_msg(format!("reference font: {e}")))?;
        let font = libfont::Font::parse(&bytes).map_err(fail_msg)?;
        for size in [8.0, 12.0, 16.0, 24.0, 48.0] {
            for cp in 32u8..=126 {
                let (w, h, s) = font
                    .glyph_segments(font.glyph_index(cp as char), size)
                    .map_err(fail_msg)?;
                if w != 0 && h != 0 {
                    spans_draw(ctx, t, buffer, w as u32, h as u32, &s)?;
                    let raster = super::expectations::rasterize(&s, w, h, 0, 0)
                        .map_err(|e| fail_msg(format!("{name} {size} {cp}: {e}")))?;
                    let mut want = scene_canvas(72, 72, BG, &[]);
                    for y in 0..h {
                        for x in 0..w {
                            put(
                                &mut want,
                                x as u32,
                                y as u32,
                                coverage(FG, BG, [raster.coverage[y * w + x]; 3]),
                            );
                        }
                    }
                    observe(ctx, t, &want, &format!("{name} {size} U+{cp:04x}"))?;
                } else {
                    frame(ctx, &scene(t, BG, &[]))?;
                    expect_draw(ctx,t,72,72,|_,_| BG,&format!("{name} {size} U+{cp:04x} empty glyph"))?;
                }
                glyphs += 1;
            }
        }
    }
    value("glyphs", glyphs, "glyphs", Some((950, 950)));
    value("corpus-ms", ms_since(start) as i64, "ms", None);
    ensure(glyphs == 950, || format!("{glyphs} glyphs checked"))
}

// Literal first-octant table from breengel-cpu-393, §5.5.
const CIRCLE_OCTANTS: &[&[i32]] = &[
    &[0],
    &[1, 1],
    &[2, 2],
    &[3, 3, 2],
    &[4, 4, 4, 3],
    &[5, 5, 5, 4],
    &[6, 6, 6, 5, 5],
    &[7, 7, 7, 6, 6, 5],
    &[8, 8, 8, 7, 7, 6],
    &[9, 9, 9, 9, 8, 8, 7],
    &[10, 10, 10, 10, 9, 9, 8, 7],
    &[11, 11, 11, 11, 10, 10, 9, 9, 8],
    &[12, 12, 12, 12, 11, 11, 10, 10, 9],
    &[13, 13, 13, 13, 12, 12, 12, 11, 10, 9],
    &[14, 14, 14, 14, 13, 13, 13, 12, 12, 11, 10],
    &[15, 15, 15, 15, 14, 14, 14, 13, 13, 12, 11],
    &[16, 16, 16, 16, 16, 15, 15, 14, 14, 13, 13, 12],
    &[17, 17, 17, 17, 17, 16, 16, 16, 15, 14, 14, 13, 12],
    &[18, 18, 18, 18, 18, 17, 17, 17, 16, 16, 15, 14, 13],
    &[19, 19, 19, 19, 19, 18, 18, 18, 17, 17, 16, 16, 15, 14],
    &[20, 20, 20, 20, 20, 19, 19, 19, 18, 18, 17, 17, 16, 15, 14],
    &[
        21, 21, 21, 21, 21, 20, 20, 20, 19, 19, 18, 18, 17, 17, 16, 15,
    ],
    &[
        22, 22, 22, 22, 22, 21, 21, 21, 21, 20, 20, 19, 18, 18, 17, 16,
    ],
    &[
        23, 23, 23, 23, 23, 22, 22, 22, 22, 21, 21, 20, 20, 19, 18, 17, 17,
    ],
    &[
        24, 24, 24, 24, 24, 23, 23, 23, 23, 22, 22, 21, 21, 20, 20, 19, 18, 17,
    ],
    &[
        25, 25, 25, 25, 25, 25, 24, 24, 24, 23, 23, 22, 22, 21, 21, 20, 19, 18,
    ],
    &[
        26, 26, 26, 26, 26, 26, 25, 25, 25, 24, 24, 24, 23, 23, 22, 21, 21, 20, 19,
    ],
    &[
        27, 27, 27, 27, 27, 27, 26, 26, 26, 25, 25, 25, 24, 24, 23, 22, 22, 21, 20, 19,
    ],
    &[
        28, 28, 28, 28, 28, 28, 27, 27, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21, 21, 20,
    ],
    &[
        29, 29, 29, 29, 29, 29, 28, 28, 28, 28, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21,
    ],
    &[
        30, 30, 30, 30, 30, 30, 29, 29, 29, 29, 28, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22, 21,
    ],
    &[
        31, 31, 31, 31, 31, 31, 30, 30, 30, 30, 29, 29, 29, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22,
    ],
    &[
        32, 32, 32, 32, 32, 32, 31, 31, 31, 31, 30, 30, 30, 29, 29, 28, 28, 27, 26, 26, 25, 24, 23,
    ],
    &[
        33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 31, 31, 31, 30, 30, 29, 29, 28, 28, 27, 26, 25, 25,
        24,
    ],
    &[
        34, 34, 34, 34, 34, 34, 33, 33, 33, 33, 33, 32, 32, 31, 31, 31, 30, 29, 29, 28, 28, 27, 26,
        25, 24,
    ],
    &[
        35, 35, 35, 35, 35, 35, 34, 34, 34, 34, 34, 33, 33, 33, 32, 32, 31, 31, 30, 29, 29, 28, 27,
        26, 25, 25,
    ],
    &[
        36, 36, 36, 36, 36, 36, 36, 35, 35, 35, 35, 34, 34, 34, 33, 33, 32, 32, 31, 31, 30, 29, 29,
        28, 27, 26,
    ],
    &[
        37, 37, 37, 37, 37, 37, 37, 36, 36, 36, 36, 35, 35, 35, 34, 34, 33, 33, 32, 32, 31, 30, 30,
        29, 28, 27, 26,
    ],
    &[
        38, 38, 38, 38, 38, 38, 38, 37, 37, 37, 37, 36, 36, 36, 35, 35, 34, 34, 33, 33, 32, 32, 31,
        30, 29, 29, 28, 27,
    ],
    &[
        39, 39, 39, 39, 39, 39, 39, 38, 38, 38, 38, 37, 37, 37, 36, 36, 36, 35, 35, 34, 33, 33, 32,
        32, 31, 30, 29, 28,
    ],
    &[
        40, 40, 40, 40, 40, 40, 40, 39, 39, 39, 39, 38, 38, 38, 37, 37, 37, 36, 36, 35, 35, 34, 33,
        33, 32, 31, 30, 30, 29,
    ],
];
