//! G2 checks, derived from gpu.md sections 1.4.1, 1.8 and 1.10.
use libbreenix::gpu::{self, abi, abi::Plain};
use libbreenix::suite::{CaseResult, value};
use libbreenix::{graphics, io, memory, process::{self, ForkResult}};
use super::common::*;
use abi::readback_reason as reason;

const A: u32 = bgra(0xC3, 0xA5, 0x5A, 0x3C); // Equals the first poison, deliberately.
const B: u32 = bgra(17, 83, 209, 127);
const C: u32 = bgra(241, 7, 61, 0);

fn setup(fmt: u32, w: u32, h: u32) -> Result<(u64, u64), libbreenix::suite::CaseError> {
    Ok((context()?, scanout_target(fmt, w, h)?))
}
fn observe(t: u64, rect: [u32; 4], px: &mut [u8], stride: u32, rec: &mut [u8; record::SIZE]) -> (i64, abi::Readback) {
    let mut a = abi::Readback { target: t, x: rect[0], y: rect[1], width: rect[2], height: rect[3],
        pixels: px.as_mut_ptr() as u64, pixels_len: px.len() as u64, stride,
        record: rec.as_mut_ptr() as u64, ..abi::Readback::ZERO };
    let r = gpu::call_with(abi::op::READBACK, &mut a);
    (r, a)
}
/// Bind a target this case drew to the scanout (section 1.10: each rendering case binds
/// its target while it owns the display).
fn present_fence(ctx: u64, t: u64) -> Result<u64, libbreenix::suite::CaseError> {
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == 0 && p.fence != 0 && p.answer == native_success()?, || format!("PRESENT: {} fence={} answer={:#x}", errno(r), p.fence, p.answer))?;
    let (r, w) = gpu::wait(ctx, p.fence, 2_000_000_000);
    ensure(r == 0 && w.status == abi::fence_status::OK, || format!("WAIT PRESENT: {} status={}", errno(r), w.status))?;
    Ok(p.fence)
}
fn present(ctx: u64, t: u64) -> CaseResult {
    present_fence(ctx, t).map(|_| ())
}
fn draw(ctx: u64, t: u64, bg: u32) -> Result<u64, libbreenix::suite::CaseError> {
    let f = frame(ctx, &scene(t, bg, &[([1, 1, 4, 3], B), ([3, 2, 7, 5], C)]))?;
    present(ctx, t)?;
    Ok(f)
}
fn want(w: u32, h: u32, bg: u32) -> Canvas {
    scene_canvas(w, h, bg, &[([1, 1, 4, 3], B), ([3, 2, 7, 5], C)])
}

pub fn full_target() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 47, 13)?;
    graphics::take_over_display().map_err(|e| fail_msg(format!("take display: {e:?}")))?;
    let before = counters(ctx)?.submissions;
    let (r, p) = gpu::present(ctx, t);
    ensure(r == -EINVAL && p.fence == 0 && p.answer == 0, || format!("unwritten PRESENT: {} fence={} answer={:#x}", errno(r), p.fence, p.answer))?;
    ensure(counters(ctx)?.submissions == before, || "unwritten PRESENT changed the submission count".to_string())?;
    // Creation intent is explicit: an ordinary target renders and reads offscreen,
    // but even after a completed write it cannot be presented.
    let offscreen = target(format::B8G8R8A8, 47, 13)?;
    let first = frame(ctx, &scene(offscreen, B, &[]))?;
    ensure(first as u32 == 1, || "unwritten PRESENT consumed a context fence".to_string())?;
    let rec = expect(offscreen, &scene_canvas(47, 13, B, &[]), "offscreen clear")?;
    ensure(u32_at(&rec, record::FLAGS) == 0, || "offscreen target claims scanout".to_string())?;
    let (r, p) = gpu::present(ctx, offscreen);
    ensure(r == -EINVAL && p.fence == 0 && p.answer == 0, || format!("offscreen PRESENT: {} fence={} answer={:#x}", errno(r), p.fence, p.answer))?;
    // Unknown flags, non-target scanout and the three reserved words stay invalid.
    for (kind, flags, reserved) in [
        (abi::kind::TARGET, 2, [0; 3]),
        (abi::kind::TEXTURE, 1, [0; 3]),
        (abi::kind::BUFFER, 1, [0; 3]),
        (abi::kind::TARGET, 1, [1, 0, 0]),
        (abi::kind::TARGET, 1, [0, 1, 0]),
        (abi::kind::TARGET, 1, [0, 0, 1]),
    ] {
        let buffer = kind == abi::kind::BUFFER;
        let mut a = [0u8; 48];
        for (offset, value) in [(0, kind), (8, u32::from(!buffer)), (12, u32::from(!buffer)),
            (16, u32::from(buffer)), (32, flags), (36, reserved[0]), (40, reserved[1]), (44, reserved[2])] {
            a[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let r = resource_create(&mut a);
        ensure(r == -EINVAL && u64_at(&a, 24) == 0, || format!("invalid RESOURCE_CREATE: {} handle={}", errno(r), u64_at(&a, 24)))?;
    }
    let second = frame(ctx, &scene(t, B, &[]))?;
    ensure(second as u32 == 2, || "offscreen PRESENT consumed a context fence".to_string())?;
    let presented = present_fence(ctx, t)?;
    ensure(presented as u32 == 3, || "PRESENT did not follow the context's submissions".to_string())?;
    expect(t, &scene_canvas(47, 13, B, &[]), "full clear")?;
    pass()
}
pub fn poison_unwritten() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 48, 7)?;
    for colour in [A, !A, B] {
        frame(ctx, &scene(t, colour, &[]))?;
        present(ctx, t)?;
        let rec = expect(t, &scene_canvas(48, 7, colour, &[]), "poison-valued pixels")?;
        ensure(u32_at(&rec, record::POISON) == A && u32_at(&rec, record::UNWRITTEN) == 0, || "complementary-poison record".to_string())?;
    }
    // One omitted pass must be detected even when the target is filled with its poison.
    fault_skip_transfer()
}
pub fn scenes_aba() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 19, 11)?;
    let mut previous = 0;
    for bg in [A, B, A] {
        draw(ctx, t, bg)?;
        let rec = expect(t, &want(19, 11, bg), "A B A")?;
        let generation = u64_at(&rec, record::GENERATION);
        ensure(generation > previous, || "generation did not advance".to_string())?;
        previous = generation;
    }
    pass()
}
fn rectangle(padded: bool) -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 73, 17)?;
    draw(ctx, t, A)?;
    let expected = want(73, 17, A);
    for [x, y, w, h] in [[1, 1, 3, 4], [2, 2, 48, 7], [72, 16, 1, 1], [0, 0, 73, 17]] {
        let row = w as usize * 4;
        let stride = row + if padded { 13 } else { 0 };
        let mut px = vec![0xD7; stride * h as usize + 11];
        let mut rec = [0u8; record::SIZE];
        let (r, a) = observe(t, [x, y, w, h], &mut px, stride as u32, &mut rec);
        ensure(r == 0 && a.reason == reason::NONE, || format!("rectangle {x},{y},{w},{h}: {} reason={}", errno(r), a.reason))?;
        for ry in 0..h as usize {
            let at = ((y as usize + ry) * 73 + x as usize) * 4;
            ensure(px[ry * stride..ry * stride + row] == expected.px[at..at + row], || format!("rectangle row {ry}"))?;
            ensure(px[ry * stride + row..(ry + 1) * stride].iter().all(|&b| b == 0xD7), || "stride padding was written".to_string())?;
        }
        ensure(px[stride * h as usize..].iter().all(|&b| b == 0xD7), || "tail guard was written".to_string())?;
        for (at, v) in [(record::X, x), (record::Y, y), (record::WIDTH, w), (record::HEIGHT, h), (record::STRIDE, stride as u32)] {
            ensure(u32_at(&rec, at) == v, || format!("record offset {at}"))?;
        }
    }
    pass()
}
pub fn partial_rect() -> CaseResult { rectangle(false) }
pub fn padded_stride() -> CaseResult { rectangle(true) }
pub fn format_canonical() -> CaseResult {
    let _case = begin()?;
    for fmt in [format::B8G8R8A8, format::R8G8B8A8] {
        let (ctx, t) = setup(fmt, 13, 7)?;
        draw(ctx, t, A)?;
        let rec = expect(t, &want(13, 7, A), "canonical BGRA")?;
        ensure(u32_at(&rec, record::FORMAT) == fmt && u32_at(&rec, record::CANONICAL) == 0, || "format record".to_string())?;
    }
    pass()
}
pub fn record_fields() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 17, 9)?;
    frame(ctx, &scene(t, B, &[([1, 1, 4, 3], B), ([3, 2, 7, 5], C)]))?;
    let presented = present_fence(ctx, t)?;
    let (px, rec) = read(t, 17, 9)?;
    ensure(u64_at(&rec, 184) == presented, || format!("record present fence {} differs from PRESENT {presented}", u64_at(&rec, 184)))?;
    want(17, 9, B).matches(&px, "record's pixels")?;
    let native = observation_backend(&rec)?;
    for (at, v) in [(0, 2), (4, 1024), (16, caps()?.backend), (20, 4), (24, 1), (28, 0), (32, 0), (36, 0),
        (40, 17), (44, 9), (48, 17), (52, 9), (56, 68), (60, 0), (112, native.success), (116, native.draw),
        (128, native.success), (132, native.readback), (160, A), (164, 0), (192, native.success), (196, 0)] {
        ensure(u32_at(&rec, at) == v, || format!("record[{at}]={} expected {v}", u32_at(&rec, at)))?;
    }
    for at in [8, 64, 80, 88, 96, 104, 120, 184] {
        ensure(u64_at(&rec, at) != 0, || format!("record[{at}] is zero"))?;
    }
    ensure(u64_at(&rec, 72) == t, || "record target handle".to_string())?;
    for range in [168..184, 200..712, 776..1024] {
        ensure(rec[range.clone()].iter().all(|&b| b == 0), || format!("nonzero reserved/window range {range:?}"))?;
    }
    ensure(rec[712..776] == caps()?.renderer, || "renderer differs from CAPS".to_string())?;
    let (_, next) = read(t, 17, 9)?;
    ensure(u64_at(&next, 8) > u64_at(&rec, 8), || "observation serial not monotonic".to_string())
}
pub fn device_path() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 9, 5)?;
    draw(ctx, t, B)?;
    let rec = expect(t, &want(9, 5, B), "device path")?;
    let native = observation_backend(&rec)?;
    for (at, v) in [(record::RENDER_KIND, native.draw), (record::RENDER_STATUS, native.success),
        (record::READBACK_KIND, native.readback), (record::READBACK_STATUS, native.success)] {
        ensure(u32_at(&rec, at) == v, || format!("device path field {at}"))?;
    }
    ensure(u32_at(&rec, record::UNWRITTEN) == 0, || "unwritten device bytes".to_string())
}
pub fn fence_identity() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 9, 5)?;
    draw(ctx, t, A)?;
    let (_, first) = read(t, 9, 5)?;
    let (_, again) = read(t, 9, 5)?;
    ensure(u64_at(&first, 104) == u64_at(&again, 104) && u64_at(&first, 96) == u64_at(&again, 96), || "read changed render identity".to_string())?;
    ensure(u64_at(&again, 120) > u64_at(&first, 120) && u64_at(&first, 120) > u64_at(&first, 104), || "transfer fence is stale or render fence reused".to_string())?;
    draw(ctx, t, B)?;
    let (_, second) = read(t, 9, 5)?;
    ensure(u64_at(&second, 104) > u64_at(&first, 104) && u64_at(&second, 96) > u64_at(&first, 96), || "later submission reused render identity".to_string())?;
    // Identity checker has to reject a real completed answer with a wrong echo.
    fault_stale_fence()
}
pub fn ordered_after_draws() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 97, 31)?;
    let rects: Vec<_> = (0..90).map(|n| ([n, n % 19, n + 7, n % 19 + 9], B.wrapping_mul(n as u32 + 1))).collect();
    // READBACK itself waits for the outstanding draw, with no WAIT in the case.
    submit(ctx, &scene(t, A, &rects))?;
    let mut px = vec![0; 97 * 31 * 4];
    let mut rec = [0; record::SIZE];
    let (r, _) = observe(t, [0, 0, 97, 31], &mut px, 97 * 4, &mut rec);
    ensure(r == 0, || format!("READBACK of outstanding draws: {}", errno(r)))?;
    scene_canvas(97, 31, A, &rects).matches(&px, "all ordered draws")?;
    present(ctx, t)?;
    ensure(u64_at(&rec, record::GENERATION) == 1, || "first submission not published once".to_string())
}
fn refused(point: u32, expected_reason: u32) -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 13, 7)?;
    draw(ctx, t, A)?;
    // A second differently coloured resource is required for wrong-resource.
    let other = target(format::B8G8R8A8, 13, 7)?;
    frame(ctx, &scene(other, B, &[]))?;
    let before = fired(point);
    arm(point, 0)?;
    let mut px = vec![0xD7; 13 * 7 * 4];
    let mut rec = [0u8; record::SIZE];
    let (r, a) = observe(t, [0, 0, 13, 7], &mut px, 52, &mut rec);
    value("reason", a.reason as i64, "", Some((expected_reason as i64, expected_reason as i64)));
    ensure(r == -EIO && a.reason == expected_reason, || format!("fault {point}: {} reason={} expected {expected_reason}", errno(r), a.reason))?;
    ensure(fired(point) == before + 1, || format!("fault {point} did not fire once"))?;
    ensure(px.iter().all(|&b| b == 0xD7), || "refused observation published pixels".to_string())?;
    // One-shot disarms itself, no explicit disarm before the recovery.
    expect(t, &want(13, 7, A), "clean read after fault")?;
    ensure(fired(point) == before + 1, || "fault fired twice".to_string())
}
pub fn fault_skip_transfer() -> CaseResult { refused(test_point::SKIP_TRANSFER, reason::UNWRITTEN) }
pub fn fault_stale_fence() -> CaseResult { refused(test_point::STALE_FENCE, reason::FENCE_MISMATCH) }
pub fn fault_wrong_resource() -> CaseResult { refused(test_point::WRONG_RESOURCE, reason::UNWRITTEN) }
pub fn fault_corrupt_pixel() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 13, 7)?;
    frame(ctx, &scene(t, A, &[]))?;
    let before = fired(test_point::CORRUPT_PIXEL);
    arm(test_point::CORRUPT_PIXEL, 0)?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::draw(&mut s, pipeline::SOLID, B, &[[2, 2, 5, 4]]);
    frame(ctx, &s)?;
    present(ctx, t)?;
    let (px, _) = read(t, 13, 7)?;
    let expected = scene_canvas(13, 7, A, &[([2, 2, 5, 4], B)]);
    let differences: Vec<_> = px.chunks_exact(4).zip(expected.px.chunks_exact(4)).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
    ensure(differences == [0] && expected.matches(&px, "planted outside pixel").is_err(), || format!("corrupt-pixel mismatches {differences:?}, expected pixel (0,0)"))?;
    ensure(fired(test_point::CORRUPT_PIXEL) == before + 1, || "corruption never fired".to_string())?;
    draw(ctx, t, A)?;
    expect(t, &want(13, 7, A), "corruption disarmed")?;
    pass()
}
pub fn fault_suppress_draw() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 13, 7)?;
    frame(ctx, &scene(t, A, &[]))?;
    let before = fired(test_point::SUPPRESS_DRAW);
    arm(test_point::SUPPRESS_DRAW, 0)?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::draw(&mut s, pipeline::SOLID, B, &[[2, 2, 5, 4]]);
    frame(ctx, &s)?; // The device really completes its fence.
    present(ctx, t)?;
    let (px, _) = read(t, 13, 7)?;
    scene_canvas(13, 7, A, &[]).matches(&px, "draw actually omitted")?;
    ensure(scene_canvas(13, 7, A, &[([2, 2, 5, 4], B)]).matches(&px, "suppressed draw").is_err(), || "exact comparator accepted suppressed draw".to_string())?;
    ensure(fired(test_point::SUPPRESS_DRAW) == before + 1, || "draw suppression never fired".to_string())?;
    frame(ctx, &s)?;
    expect(t, &scene_canvas(13, 7, A, &[([2, 2, 5, 4], B)]), "suppression disarmed")?;
    pass()
}
pub fn error_invalidates_targets() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 13, 7)?;
    let other = target(format::B8G8R8A8, 13, 7)?;
    draw(ctx, t, A)?;
    frame(ctx, &scene(other, B, &[]))?;
    arm(test_point::DEVICE_ERROR, 0)?;
    let mut s = scene(t, B, &[]);
    s.extend(scene(other, C, &[]));
    let f = submit(ctx, &s)?;
    ensure(gpu::wait(ctx, f, 2_000_000_000).0 == -EIO, || "device error did not fail submission".to_string())?;
    let mut failed_serial = 0;
    for target in [t, other] {
        for partial in [false, true] {
            if partial {
                let mut s = Vec::new(); encode::begin(&mut s, target);
                encode::draw(&mut s, pipeline::SOLID, A, &[[1, 1, 2, 2]]);
                frame(ctx, &s)?;
            }
            let mut px = vec![0xD7; 364]; let mut rec = [0; record::SIZE];
            let (r, a) = observe(target, [0, 0, 13, 7], &mut px, 52, &mut rec);
            ensure(r == -EIO && a.reason == reason::TARGET_INVALID && a.failed_submission != 0, || format!("invalid target: {} reason={} failed={}", errno(r), a.reason, a.failed_submission))?;
            if failed_serial == 0 { failed_serial = a.failed_submission; }
            ensure(a.failed_submission == failed_serial, || "invalidation lost its producing submission".to_string())?;
            ensure(px.iter().all(|&b| b == 0xD7), || "invalid pixels published".to_string())?;
        }
        frame(ctx, &scene(target, A, &[]))?;
        let rec = expect(target, &scene_canvas(13, 7, A, &[]), "whole write recovers")?;
        ensure(u64_at(&rec, record::RENDER_SERIAL) > failed_serial, || "recovery did not name a later submission".to_string())?;
    }
    pass()
}
pub fn owner_only() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 13, 7)?;
    draw(ctx, t, A)?;
    let (ready_r, ready_w) = io::pipe().map_err(|e| fail_msg(format!("pipe: {e:?}")))?;
    let (release_r, release_w) = io::pipe().map_err(|e| fail_msg(format!("pipe: {e:?}")))?;
    match process::fork().map_err(|e| fail_msg(format!("fork: {e:?}")))? {
        ForkResult::Child => {
            let _ = io::close(ready_r);
            let _ = io::close(release_w);
            let ok = (|| -> CaseResult {
                let (c, own) = setup(format::B8G8R8A8, 13, 7)?;
                frame(c, &scene(own, B, &[]))?;
                ensure(gpu::present(c, own).0 == -abi::errno::EPERM, || "non-owner presented".to_string())?;
                // Child cannot guess the parent's scanout handle or read it.
                let mut px = vec![0; 364]; let mut rec = [0; record::SIZE];
                ensure(gpu::readback(t, [0, 0, 13, 7], &mut px, 52, &mut rec) < 0, || "child read parent's scanout".to_string())?;
                // Private targets still read back without display ownership.
                let mut private = vec![0; 364];
                ensure(gpu::readback(own, [0, 0, 13, 7], &mut private, 52, &mut rec) == 0, || "private non-owner read refused".to_string())?;
                scene_canvas(13, 7, B, &[]).matches(&private, "private non-owner read")?;
                graphics::take_over_display().map_err(|e| fail_msg(format!("take: {e:?}")))?;
                ensure(io::write(ready_w, &[1]).ok() == Some(1), || "owner ready signal failed".to_string())?;
                let mut release = [0];
                ensure(io::read(release_r, &mut release).ok() == Some(1), || "owner release signal failed".to_string())
            })();
            process::exit(if ok.is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(ready_w);
            let _ = io::close(release_r);
            let mut ready = [0];
            let live = io::read(ready_r, &mut ready).ok() == Some(1);
            let mut px = vec![0; 364]; let mut rec = [0; record::SIZE];
            let refused = live && gpu::readback(0, [0, 0, 13, 7], &mut px, 52, &mut rec) == -abi::errno::EPERM;
            let _ = io::write(release_w, &[1]);
            let _ = io::close(ready_r);
            let _ = io::close(release_w);
            let mut status = 0; process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| fail_msg(format!("wait: {e:?}")))?;
            ensure(status == 0, || format!("non-owner child status {status}"))?;
            ensure(refused, || "live non-owner read the current scanout".to_string())?;
        }
    }
    // Child exit restores the live parent and its completed scanout binding.
    expect(0, &want(13, 7, A), "restored parent scanout read")?;
    present(ctx, t)?;
    expect(t, &want(13, 7, A), "owner scanout read")?;
    pass()
}
pub fn efault() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 32, 40)?;
    frame(ctx, &scene(t, B, &[]))?;
    present(ctx, t)?;
    let ptr = memory::mmap(core::ptr::null_mut(), 8192, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| fail_msg(format!("mmap: {e:?}")))?;
    // Late failure must leave the preceding writable page and the other output intact.
    unsafe { core::ptr::write_bytes(ptr, 0xD7, 8192); }
    memory::mprotect(unsafe { ptr.add(4096) }, 4096, memory::PROT_READ).map_err(|e| fail_msg(format!("mprotect: {e:?}")))?;
    for bad_record in [false, true] {
        let before = counters(ctx)?.readbacks;
        let mut px = vec![0xD7; 5120]; let mut rec = [0xD7; record::SIZE];
        let mut a = abi::Readback { target: t, width: 32, height: 40, pixels: if bad_record { px.as_mut_ptr() as u64 } else { ptr as u64 },
            pixels_len: 5120, stride: 128, record: if bad_record { (unsafe { ptr.add(4096 - 512) }) as u64 } else { rec.as_mut_ptr() as u64 }, ..abi::Readback::ZERO };
        ensure(gpu::call_with(abi::op::READBACK, &mut a) == -EFAULT, || "read-only output accepted".to_string())?;
        ensure(px.iter().all(|&b| b == 0xD7) && rec.iter().all(|&b| b == 0xD7), || "EFAULT changed another output".to_string())?;
        let prefix = unsafe { core::slice::from_raw_parts(ptr, 4096) };
        ensure(prefix.iter().all(|&b| b == 0xD7) && counters(ctx)?.readbacks == before, || "EFAULT had a side effect".to_string())?;
    }
    memory::munmap(ptr, 8192).map_err(|e| fail_msg(format!("munmap: {e:?}")))?;
    let mut px = vec![0xD7; 5120]; let mut rec = [0xD7; record::SIZE];
    for bad_record in [false, true] {
        let mut a = abi::Readback { target: t, width: 32, height: 40, pixels: if bad_record { px.as_mut_ptr() as u64 } else { ptr as u64 },
            pixels_len: 5120, stride: 128, record: if bad_record { ptr as u64 } else { rec.as_mut_ptr() as u64 }, ..abi::Readback::ZERO };
        ensure(gpu::call_with(abi::op::READBACK, &mut a) == -EFAULT, || "unmapped output accepted".to_string())?;
        ensure(px.iter().all(|&b| b == 0xD7) && rec.iter().all(|&b| b == 0xD7), || "unmapped EFAULT changed output".to_string())?;
    }
    expect(t, &scene_canvas(32, 40, B, &[]), "target unchanged after EFAULT")?;
    pass()
}
pub fn large_target() -> CaseResult {
    let _case = begin()?;
    let (ctx, t) = setup(format::B8G8R8A8, 2560, 1600)?;
    let rects = [([0, 0, 1, 1600], A), ([2559, 0, 2560, 1600], C), ([0, 1599, 2560, 1600], A), ([1279, 799, 1281, 801], C)];
    frame(ctx, &scene(t, B, &rects))?;
    present(ctx, t)?;
    expect(t, &scene_canvas(2560, 1600, B, &rects), "large target every pixel")?;
    // The staging charge must honor RLIMIT_AS, leave no charge or observation on
    // refusal, and be released after a successful read. Allocate outputs beforehand.
    let mut px = [0xD7; 4];
    let mut rec = [0xD7; record::SIZE];
    let before = counters(0)?;
    let mut old = [0u64; 2];
    let limit = [0u64, u64::MAX];
    // ARM64's prlimit64, resource 9 (RLIMIT_AS), pid 0 (the caller).
    let r = unsafe { libbreenix::syscall::raw::syscall4(libbreenix::syscall::nr::PRLIMIT64,
        0, 9, limit.as_ptr() as u64, old.as_mut_ptr() as u64) } as i64;
    ensure(r == 0, || format!("set RLIMIT_AS: {}", errno(r)))?;
    let (r, a) = observe(t, [0, 0, 1, 1], &mut px, 4, &mut rec);
    let restored = unsafe { libbreenix::syscall::raw::syscall4(libbreenix::syscall::nr::PRLIMIT64,
        0, 9, old.as_ptr() as u64, 0) } as i64;
    ensure(restored == 0, || format!("restore RLIMIT_AS: {}", errno(restored)))?;
    ensure(r == -abi::errno::ENOMEM && a.reason == reason::NONE && a.failed_submission == 0,
        || format!("staging over RLIMIT_AS: {} reason={}", errno(r), a.reason))?;
    ensure(px == [0xD7; 4] && rec == [0xD7; record::SIZE], || "ENOMEM changed readback outputs".to_string())?;
    let after = counters(0)?;
    ensure(after.resource_bytes == before.resource_bytes && after.readbacks == before.readbacks,
        || "ENOMEM leaked staging or counted an observation".to_string())?;
    let (r, _) = observe(t, [0, 0, 1, 1], &mut px, 4, &mut rec);
    ensure(r == 0 && px == A.to_le_bytes(), || format!("readback after restoring RLIMIT_AS: {}", errno(r)))?;
    ensure(counters(0)?.resource_bytes == before.resource_bytes, || "successful readback leaked staging".to_string())?;
    pass()
}
