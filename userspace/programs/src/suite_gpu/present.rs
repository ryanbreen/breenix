//! G7 device checks from gpu.md section 1.7 and gpu-abi.md's display interface.
use super::common::*;
use super::isolation::spawn;
use libbreenix::gpu::abi::{self, Plain};
use libbreenix::suite::{value, CaseError, CaseResult};
use libbreenix::{gpu, graphics};
type R<T> = Result<T, CaseError>;
const A: u32 = 0xffd15321;
const B: u32 = 0xff25b3e7;
const EBUSY: i64 = 16;
fn modes() -> R<[u8; 256]> {
    let mut a = [0u8; 256];
    let r = unsafe { gpu::call(abi::op::MODES, a.as_mut_ptr(), a.len()) };
    ensure(r == 0, || format!("MODES: {}", errno(r)))?;
    Ok(a)
}
fn mode(index: u32) -> R<()> {
    let mut a = [index, 0, 0, 0];
    let r = unsafe { gpu::call(abi::op::SET_MODE, a.as_mut_ptr().cast(), 16) };
    ensure(r == 0, || format!("SET_MODE {index}: {}", errno(r)))
}
fn take() -> R<()> {
    graphics::take_over_display().map_err(|e| fail_msg(format!("take: {e:?}")))
}
fn back() -> R<()> {
    graphics::give_back_display().map_err(|e| fail_msg(format!("give back: {e:?}")))
}
fn present(ctx: u64, t: u64) -> R<u64> {
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0 && p.answer == 0x1100 && p.fence != 0, || {
        format!(
            "PRESENT: {} answer {:#x} fence {}",
            errno(r),
            p.answer,
            p.fence
        )
    })?;
    wait_ok(ctx, p.fence, 0)?;
    Ok(p.fence)
}
fn setup(w: u32, h: u32, colour: u32) -> R<(u64, u64, Canvas)> {
    take()?;
    let c = context()?;
    let t = scanout_target(format::B8G8R8A8, w, h)?;
    let rects = [
        ([0, 0, 1, h as i32], B),
        ([w as i32 - 1, 0, w as i32, h as i32], B),
        ([0, h as i32 - 1, w as i32, h as i32], B),
        (
            [
                w as i32 / 2,
                h as i32 / 2,
                w as i32 / 2 + 2,
                h as i32 / 2 + 2,
            ],
            B,
        ),
    ];
    frame(c, &scene(t, colour, &rects))?;
    Ok((c, t, scene_canvas(w, h, colour, &rects)))
}
fn shown(t: u64, want: &Canvas, fence: u64) -> R<u64> {
    let rec = expect(t, want, "presented pixels")?;
    ensure(
        u32_at(&rec, record::FLAGS) & 1 == 1
            && u64_at(&rec, record::PRESENT_FENCE) == fence
            && u32_at(&rec, record::PRESENT_STATUS) == 0x1100,
        || "record did not identify the completed present".to_string(),
    )?;
    let actual = expect(0, want, "current scanout pixels")?;
    let id = u64_at(&rec, record::RESOURCE);
    ensure(
        id != 0 && u64_at(&actual, record::RESOURCE) == id && u64_at(&actual, record::TARGET) == 0,
        || "current scanout names another resource".to_string(),
    )?;
    ensure(u64_at(&modes()?, 64) == id, || {
        "MODES binding differs from observation".to_string()
    })?;
    Ok(id)
}
pub fn modes_listed() -> CaseResult {
    let _case = begin()?;
    let m = modes()?;
    let c = caps()?;
    for (at, want) in [
        (0, 1),
        (4, 256),
        (8, 2),
        (16, c.max_width),
        (20, c.max_height),
        (32, 1280),
        (36, 800),
        (40, 1),
        (44, 0),
        (48, 2560),
        (52, 1600),
        (56, 2),
        (60, 0),
    ] {
        ensure(u32_at(&m, at) == want, || {
            format!("MODES offset {at}: {} expected {want}", u32_at(&m, at))
        })?;
    }
    ensure(m[120..].iter().all(|&b| b == 0), || {
        "MODES reserved bytes changed".to_string()
    })?;
    Ok(())
}
fn full(index: u32, w: u32, h: u32) -> CaseResult {
    let _case = begin()?;
    let (c, t, want) = setup(w, h, A)?;
    mode(index)?;
    let f = present(c, t)?;
    shown(t, &want, f)?;
    ensure(u32_at(&modes()?, 12) == index, || {
        "selected mode not published".to_string()
    })?;
    Ok(())
}
pub fn mode_1280x800() -> CaseResult {
    full(0, 1280, 800)
}
pub fn mode_2560x1600() -> CaseResult {
    full(1, 2560, 1600)
}
pub fn scale_reported() -> CaseResult {
    let _case = begin()?;
    take()?;
    for (i, s) in [(0, 1), (1, 2)] {
        mode(i)?;
        let m = modes()?;
        let n = u32_at(&m, 12) as usize;
        ensure(n == i as usize && u32_at(&m, 32 + n * 16 + 8) == s, || {
            "current mode scale differs".to_string()
        })?;
    }
    Ok(())
}
pub fn scanout_bind() -> CaseResult {
    let _case = begin()?;
    let (c, t, w) = setup(73, 17, A)?;
    shown(t, &w, present(c, t)?)?;
    Ok(())
}
pub fn flip() -> CaseResult {
    let _case = begin()?;
    let (c, a, wa) = setup(1280, 800, A)?;
    let b = scanout_target(format::B8G8R8A8, 1280, 800)?;
    frame(c, &scene(b, B, &[]))?;
    let wb = scene_canvas(1280, 800, B, &[]);
    let mut prior = 0;
    for (t, w) in [(a, &wa), (b, &wb), (a, &wa)] {
        let f = present(c, t)?;
        ensure(f > prior, || "flip fences did not advance".to_string())?;
        shown(t, w, f)?;
        prior = f;
    }
    Ok(())
}
pub fn present_completion() -> CaseResult {
    let _case = begin()?;
    let (c, t, w) = setup(47, 13, A)?;
    let draw = frame(c, &scene(t, A, &[]))?;
    let f = present(c, t)?;
    ensure(f > draw, || {
        "present fence did not order after rendering".to_string()
    })?;
    let rec = expect(t, &scene_canvas(w.w, w.h, A, &[]), "completed present")?;
    ensure(u64_at(&rec, record::PRESENT_FENCE) == f, || {
        "record lost completion fence".to_string()
    })?;
    Ok(())
}
pub fn present_owner_only() -> CaseResult {
    let _case = begin()?;
    let c = context()?;
    let t = scanout_target(format::B8G8R8A8, 13, 7)?;
    let want = scene_canvas(13, 7, A, &[]);
    frame(c, &scene(t, A, &[]))?;
    ensure(gpu::present(c, t).0 == -abi::errno::EPERM, || {
        "non-owner present accepted".to_string()
    })?;
    let r = unsafe { gpu::call(abi::op::SET_MODE, core::ptr::null_mut(), 0) };
    ensure(r == -abi::errno::EPERM, || {
        "non-owner SET_MODE decoded arguments".to_string()
    })?;
    take()?;
    let f = present(c, t)?;
    shown(t, &want, f)?;
    Ok(())
}
pub fn readback_presented() -> CaseResult {
    let _case = begin()?;
    let (c, a, wa) = setup(73, 17, A)?;
    let f = present(c, a)?;
    let b = scanout_target(format::B8G8R8A8, 73, 17)?;
    frame(c, &scene(b, B, &[]))?;
    let rec = expect(b, &scene_canvas(73, 17, B, &[]), "unpresented frame")?;
    ensure(u32_at(&rec, record::FLAGS) & 1 == 0, || {
        "drawing another target changed the completed scanout".to_string()
    })?;
    shown(a, &wa, f)?;
    Ok(())
}
fn child_error(e: CaseError) -> String {
    format!("{e:?}")
}
pub fn takeover_giveback() -> CaseResult {
    let _case = begin()?;
    let (c, t, w) = setup(73, 17, A)?;
    let f = present(c, t)?;
    let id = shown(t, &w, f)?;
    mode(0)?;
    let kid = spawn(|_| {
        (|| -> CaseResult {
            let (c, t, w) = setup(73, 17, B)?;
            mode(1)?;
            shown(t, &w, present(c, t)?)?;
            back()?;
            Ok(())
        })()
        .map_err(child_error)
    })?;
    kid.done()?;
    let rec = expect(0, &w, "owner restore")?;
    ensure(
        u64_at(&rec, record::RESOURCE) == id && u32_at(&modes()?, 12) == 0,
        || "give-back lost target or mode".to_string(),
    )?;
    Ok(())
}
fn nested() -> CaseResult {
    let (c, t, w) = setup(31, 9, A ^ (u32_at(&modes()?, 24) << 8))?;
    present(c, t)?;
    let d = u32_at(&modes()?, 24);
    if d == 4 {
        let kid = spawn(|_| {
            let ret = unsafe { libbreenix::syscall::raw::syscall0(431) } as i64;
            if ret == -EBUSY {
                Ok(())
            } else {
                Err(format!("fifth owner returned {ret}"))
            }
        })?;
        kid.done()?;
        ensure(u32_at(&modes()?, 24) == 4, || {
            "refused fifth push changed depth".to_string()
        })?;
    } else {
        ensure(d < 4, || format!("stack depth {d}"))?;
        let kid = spawn(|_| nested().map_err(child_error))?;
        kid.done()?;
        expect(0, &w, "nested owner restored in order")?;
        ensure(u32_at(&modes()?, 24) == d, || {
            "pop did not restore depth".to_string()
        })?;
    }
    back()?;
    Ok(())
}
pub fn ownership_stack() -> CaseResult {
    let _case = begin()?;
    nested()
}
fn console_pixels() -> CaseResult {
    let m = modes()?;
    ensure(
        u64_at(&m, 72) != 0 && u64_at(&m, 80) > 0 && u64_at(&m, 88) > 0,
        || "console lacks completed GPU glyph draws or fixture upload".to_string(),
    )?;
    let width = noto_sans_mono_bitmap::get_raster_width(
        noto_sans_mono_bitmap::FontWeight::Regular,
        noto_sans_mono_bitmap::RasterHeight::Size16,
    ) as u32;
    let text = b"Breenix GPU console";
    let w = text.len() as u32 * width;
    let mut px = vec![0u8; (w * 16 * 4) as usize];
    let mut rec = [0u8; record::SIZE];
    let r = gpu::readback(0, [16, 16, w, 16], &mut px, w * 4, &mut rec);
    ensure(r == 0, || format!("console device readback: {}", errno(r)))?;
    ensure(
        u64_at(&rec, record::RESOURCE) == u64_at(&m, 72) && u32_at(&rec, record::FLAGS) & 1 == 1,
        || "restored screen is not the GPU console".to_string(),
    )?;
    for (i, &b) in text.iter().enumerate() {
        let glyph = noto_sans_mono_bitmap::get_raster(
            b as char,
            noto_sans_mono_bitmap::FontWeight::Regular,
            noto_sans_mono_bitmap::RasterHeight::Size16,
        )
        .ok_or_else(|| fail_msg("missing built-in glyph".to_string()))?;
        for (y, row) in glyph.raster().iter().enumerate() {
            for (x, &a) in row.iter().enumerate() {
                let at = (y * w as usize + i * width as usize + x) * 4;
                let bg = 0xff181512u32.to_le_bytes();
                let fg = 0xffe8edf2u32.to_le_bytes();
                for ch in 0..4 {
                    let want = ((fg[ch] as u32 * a as u32 + bg[ch] as u32 * (255 - a as u32) + 127)
                        / 255) as u8;
                    ensure(px[at + ch] == want, || {
                        format!(
                            "console glyph {i} ({x},{y}) channel {ch}: {} expected {want}",
                            px[at + ch]
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}
pub fn console_restored() -> CaseResult {
    let _case = begin()?;
    let (c, t, _) = setup(73, 17, A)?;
    present(c, t)?;
    let before = u64_at(&modes()?, 80);
    back()?;
    take()?;
    ensure(u64_at(&modes()?, 80) > before, || {
        "return to kernel did not redraw console".to_string()
    })?;
    console_pixels()
}
pub fn firmware_handoff() -> CaseResult {
    let _case = begin()?;
    take()?;
    let before = modes()?;
    ensure(
        u64_at(&before, 96) == 1
            && u64_at(&before, 104) != 0
            && u64_at(&before, 104) == u64_at(&before, 112),
        || "firmware memory changed after GPU handoff".to_string(),
    )?;
    back()?;
    take()?;
    console_pixels()?;
    let after = modes()?;
    value("handoff", u64_at(&after, 96) as i64, "", Some((1, 1)));
    ensure(
        u64_at(&after, 104) == u64_at(&after, 112) && u64_at(&before, 104) == u64_at(&after, 104),
        || "GPU console restore wrote firmware pixels".to_string(),
    )?;
    Ok(())
}
pub fn mode_change_content() -> CaseResult {
    let _case = begin()?;
    let (c, a, wa) = setup(1280, 800, A)?;
    let b = scanout_target(format::B8G8R8A8, 2560, 1600)?;
    frame(c, &scene(b, B, &[]))?;
    let wb = scene_canvas(2560, 1600, B, &[]);
    for (i, t, w) in [(0, a, &wa), (1, b, &wb), (0, a, &wa)] {
        mode(i)?;
        shown(t, w, present(c, t)?)?;
    }
    Ok(())
}
pub fn fault_skip_flush() -> CaseResult {
    let _case = begin()?;
    let (c, t, w) = setup(47, 13, A)?;
    present(c, t)?;
    arm(25, 0)?;
    let (r, p) = gpu::present(c, t);
    ensure(
        r == -EIO && p.answer == 0 && p.fence == 0 && fired(25) == 1,
        || {
            format!(
                "omitted flush returned {} answer {} fence {} fired {}",
                errno(r),
                p.answer,
                p.fence,
                fired(25)
            )
        },
    )?;
    let mut a = abi::Readback {
        target: 0,
        width: 1,
        height: 1,
        stride: 4,
        ..abi::Readback::ZERO
    };
    let mut px = [0u8; 4];
    let mut rec = [0u8; record::SIZE];
    a.pixels = px.as_mut_ptr() as u64;
    a.pixels_len = 4;
    a.record = rec.as_mut_ptr() as u64;
    let r = gpu::call_with(abi::op::READBACK, &mut a);
    ensure(r == -EIO && a.reason == 10, || {
        "omitted flush was accepted as known scanout".to_string()
    })?;
    shown(t, &w, present(c, t)?)?;
    Ok(())
}
pub fn fault_wrong_scanout() -> CaseResult {
    let _case = begin()?;
    let (c, a, wa) = setup(47, 13, A)?;
    let b = scanout_target(format::B8G8R8A8, 47, 13)?;
    frame(c, &scene(b, B, &[]))?;
    let wb = scene_canvas(47, 13, B, &[]);
    let f = present(c, a)?;
    let aid = shown(a, &wa, f)?;
    let brec = expect(b, &wb, "other target")?;
    let bid = u64_at(&brec, record::RESOURCE);
    arm(26, 0)?;
    present(c, a)?;
    let actual = expect(0, &wb, "wrong binding pixels")?;
    let requested = expect(a, &wa, "requested remains offscreen")?;
    ensure(
        fired(26) == 1
            && aid != bid
            && u64_at(&actual, record::RESOURCE) == bid
            && u32_at(&requested, record::FLAGS) & 1 == 0,
        || "wrong scanout did not differ from intended target".to_string(),
    )?;
    shown(a, &wa, present(c, a)?)?;
    Ok(())
}
