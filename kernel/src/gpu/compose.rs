//! The display side of window composition (syscall 414 op 19, ruling B13; breengel.md
//! section 18.2): the display owner that also holds the compositor claim builds its scene in
//! userspace, and the core validates and submits that stream with the snapshot's sample-only
//! source aliases, waits for its render and presents the caller's scanout target. The
//! drawing commands are the caller's own.
//!
//! The stream has two parts. The scene part draws into the persistent scene target C: it may
//! sample the aliases, and its writes are the composite's scene work. The transport part is
//! exactly one BEGIN of the destination scanout target followed by one whole-target COPY of C
//! into it. `composite_pixels` and the record's pixels count the union of the scene part's
//! clipped writes, once per pixel; the transport copy is counted only in the ordinary GPU
//! counters.

use super::*;
use crate::window::present::{Backend, Composed};
use crate::window::scene::Admission;
use crate::window::Caller;
use breengel_abi::Composite;

/// A composition's provenance, kept on the scanout target with the generation its transport
/// produced: the composite serial and, in stacking order, each window's id, commit, native
/// render fence and sampled target generation (record v2 offsets 176, 196 and 200).
pub(super) struct Provenance {
    pub generation: u64,
    pub serial: u64,
    pub windows: Vec<[u64; 4]>,
}

pub(super) struct Compose;
static BACKEND: Compose = Compose;

/// Make the GPU core the window layer's display backend.
pub(super) fn register() {
    crate::window::present::set_backend(&BACKEND);
}

impl Backend for Compose {
    fn submit_scene(&self, caller: &Caller, frame: &mut Admission, a: &mut Composite) -> Result<Composed, i32> {
        let _path = GpuPath::enter();
        compose(caller.tgid, frame, a).map_err(|e| e as i32)
    }

    fn code(&self) -> u32 {
        dev::backend()
    }

    fn give_display(&self, caller: &Caller, pid: u32) -> Result<(), i32> {
        let _path = GpuPath::enter();
        display::give(caller.tgid, pid).map_err(|e| e as i32)
    }
}

/// Device attachments of composition sources, by context and resource: a source stays
/// attached to the compositor context that first sampled it; only sampled sources are
/// attached.
static ATTACHED: spin::Mutex<BTreeSet<(u32, u32)>> = spin::Mutex::new(BTreeSet::new());

/// What the scene part of a validated stream does: the rectangles it writes in C and the
/// aliases it samples.
struct Scene {
    writes: Vec<[u32; 4]>,
    sampled: Vec<u64>,
}

fn handle_at(words: &WordBuf, at: usize) -> u64 {
    words.word(at) as u64 | (words.word(at + 1) as u64) << 32
}

/// Check the two parts of a validated stream and collect its scene writes. The scene part
/// may use SCISSOR, CLEAR, DRAW, COPY, DRAW_SAMPLED and the parameterized draw (whose
/// pipelines may read their geometry from a buffer). Any other shape (a write of anything
/// but C in the scene part, an alias anywhere but a source position, DRAW_BUFFER's buffer
/// rectangles or a PROBE, or a transport other than the one whole copy) is EINVAL.
fn scene_of(words: &WordBuf, view: &View, scene: u64, target: u64, width: u32, height: u32) -> Result<Scene, i64> {
    use stream::cmd;
    let mut out = Scene { writes: Vec::new(), sampled: Vec::new() };
    let alias = |h: u64| view.get(h).is_some_and(|e| e.sample_only);
    let sampled = |out: &mut Scene, h: u64| -> Result<(), i64> {
        if alias(h) && !out.sampled.contains(&h) {
            out.sampled.try_reserve(1).map_err(|_| ENOMEM)?;
            out.sampled.push(h);
        }
        Ok(())
    };
    let push = |out: &mut Scene, r: Option<[u32; 4]>| -> Result<(), i64> {
        if let Some(r) = r {
            out.writes.try_reserve(1).map_err(|_| ENOMEM)?;
            out.writes.push(r);
        }
        Ok(())
    };
    let (mut transport, mut copied) = (false, false);
    let mut scissor: Option<[i32; 4]> = None;
    let mut at = 0;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        if code == cmd::BEGIN {
            match handle_at(words, at + 2) {
                h if h == scene && !transport => scissor = None,
                h if h == target && !transport => transport = true,
                _ => return Err(EINVAL),
            }
        } else if transport {
            let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
            if code != cmd::COPY || copied || handle_at(words, at + 2) != scene || handle_at(words, at + 8) != target
                || r != [0, 0, width, height, 0, 0]
            {
                return Err(EINVAL);
            }
            copied = true;
        } else {
            match code {
                cmd::SCISSOR => scissor = Some([2, 3, 4, 5].map(|k| words.word(at + k) as i32)),
                cmd::CLEAR => push(&mut out, stream::clipped([0, 0, width as i32, height as i32], width, height, scissor))?,
                cmd::DRAW => {
                    for i in 0..words.word(at + 4) as usize {
                        let rect = [0, 1, 2, 3].map(|k| words.word(at + 5 + 4 * i + k) as i32);
                        push(&mut out, stream::clipped(rect, width, height, scissor))?;
                    }
                }
                cmd::COPY => {
                    let source = handle_at(words, at + 2);
                    if handle_at(words, at + 8) != scene {
                        return Err(EINVAL);
                    }
                    sampled(&mut out, source)?;
                    let e = view.get(source).ok_or(EINVAL)?;
                    let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
                    if let Some([_, _, w, h, dx, dy]) = stream::copy_clip(r, [e.width, e.height], [width, height]) {
                        push(&mut out, Some([dx, dy, dx + w, dy + h]))?;
                    }
                }
                cmd::DRAW_SAMPLED => {
                    let texture = handle_at(words, at + 3);
                    sampled(&mut out, texture)?;
                    let e = view.get(texture).ok_or(EINVAL)?;
                    for i in 0..words.word(at + 5) as usize {
                        let r = [0, 1, 2, 3, 4, 5].map(|k| words.word(at + 6 + 6 * i + k) as i32);
                        let clip = stream::sampled_clip(r, width, height, e.width, e.height)
                            .and_then(|c| stream::clipped(c.map(|v| v as i32), width, height, scissor));
                        push(&mut out, clip)?;
                    }
                }
                abi::draw::COMMAND => {
                    use abi::draw as d;
                    let pipe = words.word(at + 2);
                    let texture = handle_at(words, at + 4);
                    let source = if d::sampled(pipe) {
                        sampled(&mut out, texture)?;
                        let e = view.get(texture).ok_or(EINVAL)?;
                        Some(stream::SourceInfo { width: e.width, height: e.height, format: e.format, invalid: e.invalid })
                    } else {
                        None
                    };
                    for i in 0..words.word(at + 9) as usize {
                        let base = at + d::HEADER + d::INSTANCE * i;
                        let instance = d::Instance {
                            rect: core::array::from_fn(|k| words.word(base + k) as i32),
                            params: core::array::from_fn(|k| words.word(base + 4 + k)),
                        };
                        let clip = d::source_clip(pipe, instance, source).and_then(|i| d::clip(pipe, i, width, height, scissor));
                        push(&mut out, clip)?;
                    }
                }
                // DRAW_BUFFER's rectangles from a buffer, and integer probes, are not scene drawing.
                _ => return Err(EINVAL),
            }
        }
        at += len.max(2);
    }
    if !copied {
        return Err(EINVAL);
    }
    Ok(out)
}

/// Apply `f` to each word of `row` that bits `a..b` touch, with the mask of those bits.
fn span(row: &mut [u64], a: usize, b: usize, mut f: impl FnMut(&mut u64, u64)) {
    let (first, last) = (a / 64, (b - 1) / 64);
    for (i, word) in row.iter_mut().enumerate().take(last + 1).skip(first) {
        let lo = if i == first { a % 64 } else { 0 };
        let hi = if i == last { (b - 1) % 64 + 1 } else { 64 };
        let mask = if hi - lo == 64 { u64::MAX } else { ((1u64 << (hi - lo)) - 1) << lo };
        f(word, mask);
    }
}

/// The union of `writes` (each `[x0, y0, x1, y1]`, clipped to the screen), and its
/// intersection with each window's topmost visible region (`windows` bottom to top, on the
/// screen): a coverage bitmap of the writes' bounding box, one bit per pixel, marked by each
/// write and then counted and cleared window by window from the top. The cost is linear in
/// the writes and windows, each bounded by its rows and 64-pixel words, and the bitmap by one
/// screen.
fn work(writes: &[[u32; 4]], windows: &[[i64; 4]]) -> Result<(u64, Vec<u64>), i64> {
    let mut per_window = Vec::new();
    per_window.try_reserve_exact(windows.len()).map_err(|_| ENOMEM)?;
    per_window.resize(windows.len(), 0u64);
    let writes = writes.iter().filter(|r| r[0] < r[2] && r[1] < r[3]);
    let Some(bounds) = writes.clone().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) else {
        return Ok((0, per_window));
    };
    let (x0, y0) = (bounds[0] as usize, bounds[1] as usize);
    let stride = (bounds[2] as usize - x0).div_ceil(64);
    let rows = bounds[3] as usize - y0;
    let mut bits: Vec<u64> = Vec::new();
    bits.try_reserve_exact(stride * rows).map_err(|_| ENOMEM)?;
    bits.resize(stride * rows, 0);
    for r in writes {
        for y in r[1] as usize..r[3] as usize {
            let row = &mut bits[(y - y0) * stride..(y - y0 + 1) * stride];
            span(row, r[0] as usize - x0, r[2] as usize - x0, |w, m| *w |= m);
        }
    }
    let total = bits.iter().map(|w| w.count_ones() as u64).sum();
    for (k, w) in windows.iter().enumerate().rev() {
        let clip = [w[0].max(bounds[0] as i64), w[1].max(bounds[1] as i64), w[2].min(bounds[2] as i64), w[3].min(bounds[3] as i64)];
        if clip[0] >= clip[2] || clip[1] >= clip[3] {
            continue;
        }
        let mut covered = 0u64;
        for y in clip[1] as usize..clip[3] as usize {
            let row = &mut bits[(y - y0) * stride..(y - y0 + 1) * stride];
            span(row, clip[0] as usize - x0, clip[2] as usize - x0, |word, m| {
                covered += (*word & m).count_ones() as u64;
                *word &= !m;
            });
        }
        per_window[k] = covered;
    }
    Ok((total, per_window))
}

/// A fence status as an observation reason (gpu-abi.md, the reason table).
fn reason(status: u32) -> u32 {
    match status {
        fence_status::DEVICE_ERROR => readback_reason::DEVICE_ERROR,
        fence_status::DEVICE_HUNG => readback_reason::DEVICE_HUNG,
        fence_status::DEVICE_RESET => readback_reason::DEVICE_RESET,
        fence_status::FENCE_MISMATCH => readback_reason::FENCE_MISMATCH,
        _ => readback_reason::NONE,
    }
}

fn compose(tgid: u64, frame: &mut Admission, a: &mut Composite) -> Result<Composed, i64> {
    if !matches!(with_core(|c| c.admission()), Some(Ok(()))) {
        return Err(ENODEV);
    }
    let context = frame.context();
    let epoch = frame.epoch();
    let (width, height) = (epoch.width, epoch.height);
    let (scene, target) = (a.scene_target, a.target);
    // The destination and the persistent scene are the caller's own mode-sized targets, and a
    // bound or saved scanout is refused as either: the composite writes both.
    let (ctx_id, target_id, before) = with_core(|c| {
        let ctx = c.ctx_of(tgid, context).ok_or(EINVAL)?;
        let check = |handle: u64, scanout: bool| -> Result<u32, i64> {
            let id = c.res_of(tgid, handle).ok_or(EINVAL)?;
            let r = c.resources.get(&id).ok_or(EINVAL)?;
            if r.kind != kind::TARGET || r.format != format::B8G8R8A8 || (r.width, r.height) != (width, height) || (scanout && !r.scanout) {
                return Err(EINVAL);
            }
            if r.lost {
                return Err(EIO);
            }
            Ok(id)
        };
        let target_id = check(target, true)?;
        let scene_id = check(scene, false)?;
        if scene_id == target_id {
            return Err(EINVAL);
        }
        let shown = |id: u32| c.display.binding.is_some_and(|b| b.id == id)
            || c.display.stack.iter().any(|o| o.last.is_some_and(|b| b.id == id));
        if shown(target_id) || shown(scene_id) {
            return Err(breengel_abi::errno::EBUSY as i64);
        }
        Ok((ctx, target_id, c.resources.get(&target_id).map_or(0, |r| r.generation)))
    })
    .ok_or(ENODEV)??;
    // Each sampled source: its alias and the window target it names, pinned by the window layer.
    let mut aliases = Vec::new();
    aliases.try_reserve_exact(frame.sources.len()).map_err(|_| ENOMEM)?;
    for s in &frame.sources {
        if let Some(pin) = &s.pin {
            aliases.push(Alias { handle: s.window.sample, id: pin.mem().id() });
        }
    }
    let mut shown = Vec::new();
    shown.try_reserve_exact(frame.sources.len()).map_err(|_| ENOMEM)?;
    for s in &frame.sources {
        let w = &s.window;
        shown.push([w.x as i64, w.y as i64, w.x as i64 + w.width as i64, w.y as i64 + w.height as i64]);
    }
    let mut scene_work = None;
    let mut check = |words: &WordBuf, view: &View| -> Result<(), i64> {
        let s = scene_of(words, view, scene, target, width, height)?;
        // The scene's work is counted at admission, so a refusal here has no device effect
        // and a presented frame is not failed afterwards.
        let counted = work(&s.writes, &shown)?;
        // The serial is reserved after the scene is admitted and before any device effect.
        frame.begin().map_err(|e| e as i64)?;
        let pairs: Vec<(u32, u32)> = {
            let mut attached = ATTACHED.lock();
            let mut pairs = Vec::new();
            pairs.try_reserve_exact(s.sampled.len()).map_err(|_| ENOMEM)?;
            for h in &s.sampled {
                let id = aliases.iter().find(|x| x.handle == *h).ok_or(EINVAL)?.id;
                if !attached.contains(&(ctx_id, id)) {
                    pairs.push((ctx_id, id));
                }
            }
            // Forget attachments whose context or resource is gone.
            with_core(|c| attached.retain(|(ctx, id)| c.contexts.contains_key(ctx) && c.resources.contains_key(id)));
            pairs
        };
        for (pair, done) in pairs.iter().zip(attach_all(&pairs)) {
            done?;
            ATTACHED.lock().insert(*pair);
        }
        scene_work = Some(counted);
        Ok(())
    };
    // Test point 5, `backend-error`: this composite's render carries test point 27's
    // undefined command, which the device can only refuse.
    let forced = crate::test_control::reserve(breengel_abi::tc_point::BACKEND_ERROR, 0);
    let mut submit = abi::Submit { context, stream: a.stream, length: a.length, ..abi::Submit::ZERO };
    let extra = Extra { aliases: &aliases, check: &mut check, device_error: forced.is_some(), destination: target_id };
    let submitted = submit_stream(tgid, &mut submit, Some(extra), &|_| Ok(()));
    a.error_index = submit.error_index;
    a.error = submit.error;
    submitted?;
    if let Some(point) = forced {
        point.fire();
    }
    let scene_work = scene_work.ok_or(EIO)?;
    a.render_fence = submit.fence;
    a.estimate = submit.estimate;
    a.phase = 1;
    // The render's result, after its device requests are returned: the sources stay pinned
    // until the device has stopped sampling them, even when the wait is interrupted.
    let n = (submit.fence & 0xFFFF_FFFF) as u32;
    let finished = |c: &mut Core| -> Option<(u32, u32)> {
        let Some(ctx) = c.contexts.get(&ctx_id) else {
            return Some((fence_status::DEVICE_RESET, 0));
        };
        if n > ctx.done_upto || c.subs.values().any(|s| s.ctx == ctx_id && s.n == n) {
            return None;
        }
        Some(ctx.outcomes.get(&n).copied().unwrap_or((fence_status::OK, 0)))
    };
    let (status, answer) = match wait_for(None, true, finished) {
        Ok(outcome) => outcome,
        Err(_) => {
            let _ = wait_for(None, false, finished);
            return Err(EINTR);
        }
    };
    if status != fence_status::OK {
        a.fence_status = status;
        a.answer = answer;
        a.reason = reason(status);
        return Err(EIO);
    }
    // Provenance for the generation this render produced, when it was the target's only write.
    let mut windows = Vec::new();
    windows.try_reserve_exact(frame.sources.len()).map_err(|_| ENOMEM)?;
    for s in &frame.sources {
        windows.push([s.window.id, s.window.commit_id, s.window.render_fence, s.window.target_generation]);
    }
    let provenance = Arc::try_new(Provenance { generation: before + 1, serial: frame.serial, windows }).map_err(|_| ENOMEM)?;
    with_core(|c| {
        if let Some(r) = c.resources.get_mut(&target_id) {
            if r.generation == before + 1 {
                r.composition = Some(provenance);
            }
        }
    });
    a.phase = 2;
    let (fence, status, answer) = display::present_as(tgid, context, target, &|_| Ok(()))?;
    a.present_fence = fence;
    if status != fence_status::OK {
        a.fence_status = status;
        a.answer = answer;
        a.reason = reason(status);
        return Err(EIO);
    }
    a.answer = answer;
    let (pixels, per_window) = scene_work;
    a.pixels = pixels;
    Ok(Composed { pixels, per_window })
}
