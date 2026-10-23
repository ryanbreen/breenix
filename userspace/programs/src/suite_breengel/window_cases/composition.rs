//! The window cases of composition (contract section 18).

use crate::*;

pub(crate) const COMPOSITION: &[Case] = &[
    pictured!("compose-position", "A committed pattern appears at the window's position in the composed frame, exactly", composition_compose_position),
    pictured!("compose-overlap", "Two overlapping windows compose in z order, exactly", composition_compose_overlap),
    pictured!("compose-offscreen", "A window partly off the screen is clipped at the screen edge", composition_compose_offscreen),
    pictured!("compose-fullscreen", "A full-mode window at 0,0 fills the frame exactly", composition_compose_fullscreen),
    pictured!("compose-many", "Sixteen windows in a fixed stack compose to the formula, exactly", composition_compose_many),
    pictured!("generation", "Each commit advances the window's commit id by exactly one", composition_generation),
    pictured!("no-compositor", "A commit with no compositor returns within 250 ms and is composed once a compositor claims", composition_no_compositor),
    pictured!("compositor-dies", "An acquire blocked when the compositor exits, and a blocking event read on another window, return within 250 ms, and the next commit succeeds", composition_compositor_dies),
    pictured!("eintr", "A signal interrupts a blocked acquire with EINTR", composition_eintr),
    pictured!("damage-metadata", "A commit's damage rectangles are recorded exactly", composition_damage_metadata),
    pictured!("damage-work", "A composite processes exactly the pixels in its damage union, by the compositor's counter, and with `ignore-damage` armed the excess is reported by count", composition_damage_work),
    pictured!("damage-screen", "A partial composite gives the same frame as a full recomposition", composition_damage_screen),
    pictured!("composite-record", "Each composite publishes its serial and the commit id and pixel generation of every presentation-required window it accounts for, a wholly occluded one included with zero composed pixels", composition_composite_record),
    pictured!("backend-error", "A forced backend failure leaves the commit uncomposited, is reported to the compositor, and the retry succeeds", composition_backend_error),
    pictured!("inflight-bound", "At most one composite per window is in flight", composition_inflight_bound),
    pictured!("no-change-commit", "commit_no_change publishes new metadata with empty damage and the same pixel generation, causes no composite, and is presented by the composite that already holds that generation", composition_no_change_commit),
    pictured!("cursor-hidden", "With the cursor hidden, no cursor pixel appears in the frame", composition_cursor_hidden),
    pictured!("cursor-no-trail", "Moving the cursor over a static scene restores every pixel it covered", composition_cursor_no_trail),
    pictured!("cursor-shape", "Setting a cursor shape draws that shape at the pointer", composition_cursor_shape),
    pictured!("cursor-stationary-repaint", "Content committed under a stationary cursor is shown, with the cursor redrawn above it", composition_cursor_stationary_repaint),
    pictured!("irq-bound", "The longest IRQ-masked interval during full-screen damage stays within 500 µs, after `irq-hold` has shown the gauge reads a 2 ms hold", composition_irq_bound),
    pictured!("present-rate", "One 256x256 window composes at least 60 commits a second", composition_present_rate),
    pictured!("takeover-giveback", "Display takeover and giveback restore the previous owner's frame exactly", composition_takeover_giveback),
    pictured!("ownership-stack", "When a compositor dies, the display returns to the process that gave it, not to the runner", composition_ownership_stack),
    pictured!("gpu-composite", "GPU composition draws client targets without CPU pixel uploads", composition_gpu_composite),
    pictured!("scale-2", "Scale-2 GPU composition preserves placement, clipping and exact pixels", composition_scale_2),
    pictured!("target-lifetime", "Compositor, scanout, capture and replay holds retain GPU targets until their final release", composition_target_lifetime),
];

// ---------------------------------------------------------------------------------------
// composition: commit, composition & present (contract section 18)
// ---------------------------------------------------------------------------------------

use super::kit::*;
use abi::{op, Plain};

/// A window of a scene: where the compositor places it, its stacking and the pattern it shows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placed {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) z: u32,
    pub(crate) seed: u32,
}

impl Placed {
    pub(crate) fn new(x: i32, y: i32, w: u32, h: u32, z: u32, seed: u32) -> Placed {
        Placed { x, y, w, h, z, seed }
    }

    fn contains(&self, px: u32, py: u32) -> bool {
        let (px, py) = (px as i64, py as i64);
        px >= self.x as i64 && py >= self.y as i64 && px < self.x as i64 + self.w as i64 && py < self.y as i64 + self.h as i64
    }
}

/// Section 18's formula for a screen pixel: the topmost visible window's committed pixel there
/// (a later entry of equal `z` above an earlier one), or black; X reads back as 0xFF.
pub(crate) fn formula(scene: &[Placed]) -> impl Fn(u32, u32) -> [u8; 4] + '_ {
    move |px, py| {
        let mut top: Option<&Placed> = None;
        for p in scene {
            if p.contains(px, py) && top.map_or(true, |t| p.z >= t.z) {
                top = Some(p);
            }
        }
        match top {
            Some(p) => {
                let c = pattern(p.seed, (px as i64 - p.x as i64) as u32, (py as i64 - p.y as i64) as u32);
                [c[0], c[1], c[2], 0xFF]
            }
            None => [0, 0, 0, 0xFF],
        }
    }
}

/// kiosk_test, claimed, composing only when told, with the cursor hidden.
pub(crate) fn compositor() -> Result<Helper, breenix_suite::suite::CaseError> {
    let (mut k, _) = kiosk().cite("24")?;
    let auto = k.ask("auto 0").cite("24")?;
    ensure(auto == "ok", "24", format!("kiosk_test auto 0 answered `{auto}`"))?;
    let hidden = k.ask(&format!("cursor {} 0 0 0", abi::cursor_shape::HIDDEN)).cite("24")?;
    ensure(hidden == "ok", "18", format!("the compositor's SET_CURSOR hiding the cursor answered `{hidden}`"))?;
    Ok(k)
}

/// Register `w`, commit pattern `p.seed` over it and have the compositor place it visible.
pub(crate) fn show(k: &mut Helper, w: &W, p: &Placed) -> CaseResult {
    want_ok(w.register(b"composition"), "17.1", "REGISTER")?;
    w.frame(p.seed).cite("17.3")?;
    place(k, w.id(), p, abi::geometry_flags::VISIBLE)
}

pub(crate) fn place(k: &mut Helper, id: u64, p: &Placed, flags: u32) -> CaseResult {
    let r = k.ask(&format!("place {id} {} {} {} {flags}", p.x, p.y, p.z)).cite("24")?;
    ensure(r == "ok", "18", format!("the compositor's SET_GEOMETRY of window {id} answered `{r}`"))
}

/// The compositor composes once: its serial and pixel count.
pub(crate) fn compose(k: &mut Helper) -> Result<(u64, u64), breenix_suite::suite::CaseError> {
    let r = k.ask("composite").cite("24")?;
    let mut w = r.split(' ');
    match (w.next(), w.next().and_then(|s| s.parse().ok()), w.next().and_then(|s| s.parse().ok())) {
        (Some("ok"), Some(serial), Some(pixels)) => Ok((serial, pixels)),
        _ => Err(breenix_suite::suite::CaseError::Fail(format!("gpu-display: B13 userspace snapshot/stream COMPOSITE answered `{r}` ({CONTRACT} section 18)"))),
    }
}

// claim-lint:ok: docs/gui/breengel.md specifies this assertion; the GPU lanes supply its implementation.
/// The active physical mode owns every screen allocation and row index (B8).
pub(crate) fn screen_size() -> Result<(usize,usize), breenix_suite::suite::CaseError> {
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("18")?;
    ensure((modes.current as usize)<modes.modes.len(),"18","gpu-display: invalid current mode")?;
    let mode=&modes.modes[modes.current as usize];
    ensure((mode.width,mode.height,mode.scale)==(2560,1600,2),"18","gpu-display: required scale-2 mode is absent")?;
    Ok((mode.width as usize,mode.height as usize))
}

/// The whole composed screen and its observation record (`SCANOUT_READBACK`, as root).
pub(crate) fn screen() -> Result<(Vec<u8>, [u8; 1024]), breenix_suite::suite::CaseError> {
    let (width,height) = screen_size()?;
    let mut px = vec![0u8; width * height * 4];
    let mut rec = [0u8; 1024];
    let mut rb = super::handoff::Readback::ZERO;
    rb.dst = px.as_mut_ptr() as u64;
    rb.dst_len = px.len() as u64;
    rb.record = rec.as_mut_ptr() as u64;
    rb.record_len = 1024;
    let r = call(op::SCANOUT_READBACK, &mut rb);
    if r < 0 {
        return Err(breenix_suite::suite::CaseError::Fail(format!("gpu-display: B13 48-byte SCANOUT_READBACK {} reason {} ({CONTRACT} section 20.1)", outcome(r), rb.reason)));
    }
    ensure(super::handoff::word(&rec,0)==2 && super::handoff::word(&rec,4)==1024,
        "20.1", "gpu-display: B13 window observation must decode core record v2")?;
    Ok((px, rec))
}

/// The screen must equal `scene`'s formula everywhere.
pub(crate) fn screen_is(px: &[u8], scene: &[Placed], what: &str) -> CaseResult {
    let (width,height)=screen_size()?;
    same(px, width as u32, height as u32, true, what, "18", formula(scene))
}

/// A composite's record entries, `(id, commit_id, pixel_gen, pixels)`, with its header pixels.
pub(crate) fn record(k: &mut Helper, serial: u64) -> Result<(u64, Vec<(u64, u64, u64, u64)>), breenix_suite::suite::CaseError> {
    let r = k.ask(&format!("record {serial}")).cite("24")?;
    let mut words = r.split(' ');
    if words.next() != Some("ok") {
        return Err(breenix_suite::suite::CaseError::Fail(format!("COMPOSITE_RECORD({serial}) answered `{r}` ({CONTRACT} section 18)")));
    }
    let head: Vec<u64> = words.by_ref().take(4).filter_map(|w| w.parse().ok()).collect();
    if head.len() != 4 || head[0] != serial {
        return Err(breenix_suite::suite::CaseError::Fail(format!("COMPOSITE_RECORD({serial}) answered `{r}` ({CONTRACT} section 18)")));
    }
    let entries = words.filter_map(|e| {
        let v: Vec<u64> = e.split(':').filter_map(|n| n.parse().ok()).collect();
        (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
    }).collect();
    Ok((head[1], entries))
}

fn scene_case(scene: &[Placed], what: &str) -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let wins: Vec<W> = scene.iter().map(|p| W::new(p.w, p.h, 2)).collect::<Result<_, _>>()?;
    for (w, p) in wins.iter().zip(scene) {
        show(&mut k, w, p)?;
    }
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, scene, what)
}

fn composition_compose_position() -> CaseResult {
    scene_case(&[Placed::new(200, 150, 100, 60, 0, 1)], "a 100x60 window at (200, 150)")
}

fn composition_compose_overlap() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let mut scene = [Placed::new(100, 100, 200, 120, 1, 2), Placed::new(180, 150, 200, 120, 2, 3)];
    let wins: Vec<W> = scene.iter().map(|p| W::new(p.w, p.h, 2)).collect::<Result<_, _>>()?;
    for (w, p) in wins.iter().zip(scene.iter()) {
        show(&mut k, w, p)?;
    }
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &scene, "window B above window A")?;
    // Restacked: A above B.
    scene[0].z = 3;
    place(&mut k, wins[0].id(), &scene[0], abi::geometry_flags::VISIBLE)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &scene, "window A raised above window B")
}

fn composition_compose_offscreen() -> CaseResult {
    let (width,height)=screen_size()?;
    scene_case(&[Placed::new(-30, -20, 100, 60, 0, 4), Placed::new(width as i32-60, height as i32-30, 120, 50, 1, 5), Placed::new(-10, 500, 40, 40, 2, 6)],
        "windows partly off every screen edge")
}

fn composition_compose_fullscreen() -> CaseResult {
    let (width,height)=screen_size()?;
    scene_case(&[Placed::new(0, 0, width as u32, height as u32, 0, 7)], "a full-mode window at (0, 0)")
}

fn composition_compose_many() -> CaseResult {
    let scene: Vec<Placed> = (0..16u32).map(|i| Placed::new(40 + 60 * i as i32, 30 + 40 * i as i32, 160, 120, 15 - i, 10 + i)).collect();
    scene_case(&scene, "sixteen windows, each under the one before it")?;
    let mut k=compositor()?;
    let wins:Vec<W>=scene.iter().map(|p|W::new(p.w,p.h,2)).collect::<Result<_,_>>()?;
    for (w,p) in wins.iter().zip(&scene) {show(&mut k,w,p)?;}
    // The per-process limit remains sixteen. A separate client supplies window 17.
    let mut raw=raw_client().cite("24")?;
    let created=raw.ask("create 8 8 2").cite("24")?;
    let id=created.strip_prefix("ok ").and_then(|s|s.split_whitespace().next()).and_then(|s|s.parse::<u64>().ok())
        .ok_or_else(||format!("gpu-window: seventeenth-window fixture: {created}"))?;
    ensure(raw.ask(&format!("register {id} overflow")).cite("24")?=="ok","17.1","gpu-window: extra window registration")?;
    ensure(raw.ask(&format!("fill {id} 27")).cite("24")?.starts_with("ok "),"17.3","gpu-window: extra window publication")?;
    place(&mut k,id,&Placed::new(0,0,8,8,16,27),abi::geometry_flags::VISIBLE)?;
    let (serial,_)=compose(&mut k)?;
    let (_,entries)=record(&mut k,serial)?;
    ensure(entries.len()==17,"18","gpu-display: COMPOSITE_RECORD truncated a scene to sixteen windows")?;
    let (width,height)=screen_size()?;
    let mut pixels=vec![0xA5; width*height*4]; let mut observed=[0xA5u8;1024];
    let mut rb=super::handoff::Readback::ZERO;
    rb.dst=pixels.as_mut_ptr() as u64;rb.dst_len=pixels.len() as u64;
    rb.record=observed.as_mut_ptr() as u64;rb.record_len=1024;
    want_errno(call(op::SCANOUT_READBACK,&mut rb),5,"20.1","gpu-display: B13 composite-overflow observation")?;
    ensure(rb.reason==11 && rb.serial==0 && pixels.iter().chain(observed.iter()).all(|&b|b==0xA5),
        "20.1","gpu-display: overflow must report reason 11 without partial pixels, record or serial")
}

fn composition_generation() -> CaseResult {
    interface()?;
    let w = W::new(32, 32, 2)?;
    want_ok(w.register(b"generation"), "17.1", "REGISTER")?;
    let mut last = (0u64, 0u64);
    for step in 0..6u32 {
        let (commit_id, pixel_gen) = match step % 3 {
            0 => {
                let c = w.frame(step).cite("17.3")?;
                (c.commit_id, c.pixel_gen)
            }
            1 => {
                let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
                let c = w.commit(a.buffer, &[]).map_err(|r| format!("COMMIT with no damage {}", outcome(r))).cite("17.3")?;
                (c.commit_id, c.pixel_gen)
            }
            _ => {
                let mut n = abi::NoChange::ZERO;
                n.id = w.id();
                want_ok(call(op::COMMIT_NO_CHANGE, &mut n), "17.3", "COMMIT_NO_CHANGE")?;
                (n.commit_id, last.1)
            }
        };
        ensure(commit_id == last.0 + 1, "17.3", format!("commit {step} has id {commit_id}; the one before had {}", last.0))?;
        let want_gen = if step % 3 == 0 { last.1 + 1 } else { last.1 };
        ensure(pixel_gen == want_gen, "17.3", format!("commit {step} has pixel generation {pixel_gen}; with {} damage it is {want_gen}",
            if step % 3 == 0 { "whole-window" } else { "no" }))?;
        let i = listed(w.id()).cite("17.1")?.ok_or("the window is not listed (section 17.2)").cite("17.2")?;
        ensure((i.commit_id, i.pixel_gen) == (commit_id, pixel_gen), "17.3", format!(
            "LIST gives commit {} generation {} after commit {commit_id} generation {pixel_gen}", i.commit_id, i.pixel_gen))?;
        last = (commit_id, pixel_gen);
    }
    Ok(())
}

fn composition_no_compositor() -> CaseResult {
    interface()?;
    let p = Placed::new(300, 200, 120, 80, 0, 20);
    let w = W::new(p.w, p.h, 2)?;
    want_ok(w.register(b"no-compositor"), "17.1", "REGISTER")?;
    let mut longest = 0;
    for seed in [18, 19, 20] {
        let start = now_ms();
        w.frame(seed).cite("17.3")?;
        longest = longest.max(since(start));
    }
    value("commit-ms", longest as i64, "ms", Some((0, 250)));
    ensure(longest <= 250, "17.3", format!("with no compositor a commit took {longest} ms; it returns within 250 ms"))?;
    let mut k = compositor()?;
    place(&mut k, w.id(), &p, abi::geometry_flags::VISIBLE)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &[p], "the frame committed before the compositor claimed")
}

fn composition_compositor_dies() -> CaseResult {
    interface()?;
    let (mut k, _) = kiosk().cite("24")?;
    let w = std::sync::Arc::new(W::new(256, 256, 2)?);
    want_ok(w.register(b"compositor-dies"), "17.1", "REGISTER")?;
    place(&mut k, w.id(), &Placed::new(10, 10, 256, 256, 0, 0), abi::geometry_flags::VISIBLE)?;
    let first = w.frame(1).cite("17.3")?;
    // Ruling B38: the stall is armed once the compositor's composite of frame 1 is recorded,
    // so that frame's buffer is no longer sampled and the case does not wait out the stall.
    let start = now_ms();
    loop {
        let r = k.ask(&format!("composed {}", w.id())).cite("24")?;
        match r.strip_prefix("ok ").and_then(|v| v.parse::<u64>().ok()) {
            Some(c) if c >= first.commit_id => break,
            Some(_) if since(start) < 2_000 => { let _ = time::sleep_ms(2); }
            _ => return at("18", format!("the compositor's composite of the window's first commit was not recorded within 2 s: kiosk_test answered `{r}`")),
        }
    }
    // The compositor's next composite of this window pauses with the window's front pinned.
    let fired = |point| libbreenix::test_control::fired(point).map_err(|e| format!("FIRED: {e}"));
    let stalls = fired(abi::tc_point::COMPOSITE_STALL).cite("21")?;
    libbreenix::test_control::arm(abi::tc_point::COMPOSITE_STALL, w.id(), 0).map_err(|e| format!("ARM composite-stall: {e}")).cite("21")?;
    w.frame(2).cite("17.3")?;
    let start = now_ms();
    while fired(abi::tc_point::COMPOSITE_STALL).cite("21")? == stalls && since(start) < 1_000 {
        let _ = time::sleep_ms(5);
    }
    if fired(abi::tc_point::COMPOSITE_STALL).cite("21")? == stalls {
        let _ = libbreenix::test_control::disarm_all();
        return at("18", "the compositor's composite of a new commit never reached the backend with the window pinned (composite-stall did not fire within 1 s)");
    }
    // Commit the other buffer: the pinned one is retired, so no buffer is free and the next
    // ACQUIRE blocks until the compositor lets go of it.
    w.frame(3).cite("17.3")?;
    let client = {
        let w = w.clone();
        std::thread::spawn(move || -> (u64, Result<abi::Acquire, i64>) {
            let a = w.acquire(2_000_000_000);
            (now_ms(), a)
        })
    };
    // A second window's reader blocks with no timeout and nothing queued: the compositor's
    // death ends its wait with no events.
    let quiet = W::new(8, 8, 2)?;
    let _ = read_events(quiet.id());
    let qid = quiet.id();
    let reader = std::thread::spawn(move || -> (u64, i64) {
        let mut ev = vec![abi::WindowEvent::ZERO; 4];
        let mut r = abi::ReadEvents::ZERO;
        r.id = qid;
        r.ptr = ev.as_mut_ptr() as u64;
        r.capacity = 4;
        r.flags = abi::READ_EVENTS_BLOCK;
        r.timeout_ns = u64::MAX;
        let n = call(op::READ_EVENTS, &mut r);
        (now_ms(), n)
    });
    let _ = time::sleep_ms(100);
    let killed = now_ms();
    let death = k.kill_checked();
    let reaped = now_ms();
    let (returned, a) = client.join().map_err(|_| String::from("the client thread panicked")).cite("17.5")?;
    let _ = libbreenix::test_control::disarm_all();
    death.cite("24")?;
    let (read_at, n) = reader.join().map_err(|_| String::from("the reader thread panicked")).cite("17.5")?;
    ensure(n == 0, "17.5", format!("the blocking READ_EVENTS on another window returned {} when the compositor died; it returns 0 events", outcome(n)))?;
    ensure(read_at >= killed, "17.5", format!("the blocking READ_EVENTS returned {} ms before the compositor was killed", killed - read_at))?;
    let read_gap = read_at.saturating_sub(reaped);
    value("read-events-after-death-ms", read_gap as i64, "ms", Some((0, 250)));
    ensure(read_gap <= 250, "17.5", format!(
        "after the compositor was reaped the blocking READ_EVENTS returned {read_gap} ms later; it returns within 250 ms of the compositor's death"))?;
    let a = a.map_err(|r| format!("the blocked ACQUIRE {}", outcome(r))).cite("17.5")?;
    ensure(returned >= killed, "17.5", format!(
        "the ACQUIRE returned {} ms before the compositor was killed; with the only other buffer pinned by the compositor it must block", killed - returned))?;
    let gap = returned.saturating_sub(reaped);
    value("acquire-after-death-ms", gap as i64, "ms", Some((0, 250)));
    ensure(gap <= 250, "17.5", format!(
        "after the compositor was reaped the blocked ACQUIRE returned {gap} ms later; it returns within 250 ms of the compositor's death"))?;
    fill(w.target(a.buffer), 256, 256, 77)?;
    w.commit(a.buffer, &[abi::Rect::new(0, 0, 256, 256)]).map_err(|r| format!("COMMIT after the compositor died {}", outcome(r))).cite("17.3")?;
    Ok(())
}

extern "C" fn on_usr1(_: i32) {}

fn composition_eintr() -> CaseResult {
    interface()?;
    let mut f = || {
        let action = signal::Sigaction::new(on_usr1);
        if signal::sigaction(signal::SIGUSR1, Some(&action), None).is_err() {
            return 2;
        }
        let Ok(w) = W::new(16, 16, 2) else { return 3 };
        for _ in 0..2 {
            if w.acquire(0).is_err() {
                return 4;
            }
        }
        let _ = io::write(Fd::from_raw(1), b"blocking\n");
        let r = w.acquire(5_000_000_000).err().unwrap_or(0);
        let _ = io::write(Fd::from_raw(1), format!("returned {r}\n").as_bytes());
        0
    };
    let mut h = Helper::call(&mut f).cite("24")?;
    let line = h.line(3_000).cite("24")?;
    ensure(line == "blocking", "24", format!("the helper said `{line}`"))?;
    let _ = time::sleep_ms(100);
    signal::kill(h.pid, signal::SIGUSR1).map_err(|e| format!("kill SIGUSR1: {e}")).cite("17.3")?;
    let done = h.line(2_000).cite("17.3")?;
    ensure(done == format!("returned {}", -(EINTR as i64)), "17.3", format!("an ACQUIRE blocked with no free buffer and interrupted by a handled SIGUSR1 {done}; the contract gives EINTR"))
}

fn composition_damage_metadata() -> CaseResult {
    interface()?;
    let w = W::new(96, 64, 3)?;
    w.frame(1).cite("17.3")?;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let rects = [abi::Rect::new(3, 5, 10, 7), abi::Rect::new(40, 0, 20, 64), abi::Rect::new(90, 60, 6, 4)];
    for (i, r) in rects.iter().enumerate() {
        fill_rect(w.target(a.buffer), 96, *r, 50 + i as u32)?;
    }
    w.commit(a.buffer, &rects).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
    // The other buffer that held the first commit replays exactly that commit's damage.
    let b = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let got = &b.replay[..b.replay_count as usize];
    ensure(b.replay_full == 0 && got == rects, "17.3", format!(
        "the next buffer's replay after a commit damaging {rects:?} was {got:?}{}; the damage is recorded exactly",
        if b.replay_full != 0 { " (whole window)" } else { "" }))
}

/// The trace counter `composite_pixels`.
fn composed_pixels() -> Result<u64, breenix_suite::suite::CaseError> {
    counter("composite_pixels").cite("18")
}

fn composition_damage_work() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let p = Placed::new(100, 100, 200, 150, 0, 3);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    let (_, first) = compose(&mut k)?;
    let (width,height)=screen_size()?;
    ensure(first == (width*height) as u64, "18", format!("the claim's first composite processed {first} pixels; it must cover the reported mode"))?;
    let damaged = |w: &W, seed: u32| -> CaseResult {
        let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
        fill_rect(w.target(a.buffer), 200, abi::Rect::new(20, 30, 10, 10), seed)?;
        w.commit(a.buffer, &[abi::Rect::new(20, 30, 10, 10)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
        Ok(())
    };
    damaged(&w, 4)?;
    let c0 = composed_pixels()?;
    let (_, pixels) = compose(&mut k)?;
    let counted = composed_pixels()? - c0;
    value("damage-pixels", pixels as i64, "px", Some((100, 100)));
    ensure(pixels == 100 && counted == 100, "18", format!(
        "a 10x10 damaged commit was composed as {pixels} pixels ({counted} by composite_pixels); a composite processes exactly its damage union"))?;
    // The twin: with ignore-damage armed the whole window is processed, and the excess shows.
    libbreenix::test_control::arm(abi::tc_point::IGNORE_DAMAGE, w.id(), 0).map_err(|e| format!("ARM ignore-damage: {e}")).cite("21")?;
    damaged(&w, 5)?;
    let (_, twin) = compose(&mut k)?;
    let _ = libbreenix::test_control::disarm_all();
    let excess = twin as i64 - 100;
    value("ignore-damage-excess", excess, "px", None);
    ensure(twin == 200 * 150, "21", format!(
        "with ignore-damage armed the composite processed {twin} pixels, an excess of {excess}; the twin processes the whole 200x150 window"))?;
    damaged(&w, 6)?;
    let (_, again) = compose(&mut k)?;
    ensure(again == 100, "18", format!("after ignore-damage fired, a 10x10 damaged commit was composed as {again} pixels"))
}

fn composition_damage_screen() -> CaseResult {
    let (width,height)=screen_size()?;
    interface()?;
    let mut k = compositor()?;
    let scene = [Placed::new(50, 60, 300, 200, 0, 8), Placed::new(250, 160, 200, 150, 1, 9)];
    let wins: Vec<W> = scene.iter().map(|p| W::new(p.w, p.h, 2)).collect::<Result<_, _>>()?;
    for (w, p) in wins.iter().zip(scene.iter()) {
        show(&mut k, w, p)?;
    }
    compose(&mut k)?;
    // A partial update of the lower window, half covered by the upper one.
    let a = wins[0].acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let r = abi::Rect::new(150, 50, 140, 140);
    fill_rect(wins[0].target(a.buffer), 300, r, 33)?;
    wins[0].commit(a.buffer, &[r]).map_err(|e| format!("COMMIT {}", outcome(e))).cite("17.3")?;
    compose(&mut k)?;
    let (partial, _) = screen()?;
    // A new claim composes the whole screen afresh.
    drop(k);
    let mut fresh = compositor()?;
    let (_, pixels) = compose(&mut fresh)?;
    ensure(pixels == (width*height) as u64, "18", format!("a new compositor's first composite processed {pixels} pixels, not the whole screen"))?;
    let (full, _) = screen()?;
    match (0..width * height).find(|&i| partial[i * 4..i * 4 + 4] != full[i * 4..i * 4 + 4]) {
        None => Ok(()),
        Some(i) => at("18", format!("the partial composite's frame differs from a full recomposition at ({}, {}): {:02x?} against {:02x?}",
            i % width, i / width, &partial[i * 4..i * 4 + 4], &full[i * 4..i * 4 + 4])),
    }
}

fn composition_composite_record() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let shown = Placed::new(100, 100, 120, 90, 1, 11);
    let occluded = Placed::new(400, 300, 50, 40, 1, 12);
    let cover = Placed::new(380, 280, 200, 150, 2, 13);
    let a = W::new(shown.w, shown.h, 2)?;
    let b = W::new(occluded.w, occluded.h, 2)?;
    let c = W::new(cover.w, cover.h, 2)?;
    let minimized = W::new(30, 30, 2)?;
    let unregistered = W::new(30, 30, 2)?;
    show(&mut k, &a, &shown)?;
    show(&mut k, &b, &occluded)?;
    show(&mut k, &c, &cover)?;
    show(&mut k, &minimized, &Placed::new(700, 100, 30, 30, 3, 14))?;
    place(&mut k, minimized.id(), &Placed::new(700, 100, 30, 30, 3, 14), abi::geometry_flags::VISIBLE | abi::geometry_flags::MINIMIZED)?;
    unregistered.frame(15).cite("17.3")?;
    place(&mut k, unregistered.id(), &Placed::new(800, 100, 30, 30, 4, 15), abi::geometry_flags::VISIBLE)?;
    let (serial, _) = compose(&mut k)?;
    let (_, entries) = record(&mut k, serial)?;
    let ids: Vec<u64> = entries.iter().map(|e| e.0).collect();
    for (w, name) in [(&a, "the visible window"), (&b, "the wholly occluded window"), (&c, "the covering window")] {
        let i = listed(w.id()).cite("17.1")?.ok_or("a placed window is not listed (section 17.2)").cite("17.2")?;
        let e = entries.iter().find(|e| e.0 == w.id());
        ensure(e.is_some_and(|e| e.1 == i.commit_id && e.2 == i.pixel_gen), "18", format!(
            "composite {serial}'s record lists {name} (window {}) as {e:?}; its commit is {} at generation {}", w.id(), i.commit_id, i.pixel_gen))?;
    }
    let hidden = entries.iter().find(|e| e.0 == b.id()).map_or(u64::MAX, |e| e.3);
    ensure(hidden == 0, "18", format!("the wholly occluded window is recorded with {hidden} composed pixels; it is listed with 0"))?;
    ensure(!ids.contains(&minimized.id()) && !ids.contains(&unregistered.id()), "18", format!(
        "composite {serial}'s record lists windows {ids:?}, including a minimized or an unregistered one, which are not presentation-required"))
}

fn composition_backend_error() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let p = Placed::new(200, 200, 100, 100, 0, 16);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    let (serial, _) = compose(&mut k)?;
    let next = Placed { seed: 17, ..p };
    w.frame(next.seed).cite("17.3")?;
    libbreenix::test_control::arm(abi::tc_point::BACKEND_ERROR, 0, 0).map_err(|e| format!("ARM backend-error: {e}")).cite("21")?;
    let failed = k.ask("composite").cite("24")?;
    let _ = libbreenix::test_control::disarm_all();
    ensure(failed == format!("err {}", 5), "18", format!("COMPOSITE with backend-error armed answered `{failed}`; the contract gives EIO (err 5)"))?;
    // The failed composite is serial + 1, recorded as failed, with no pixels and no windows.
    let record = k.ask(&format!("record {}", serial + 1)).cite("24")?;
    let fields: Vec<u64> = record.strip_prefix("ok ").unwrap_or("").split(' ').take(4).filter_map(|n| n.parse().ok()).collect();
    ensure(fields.len() == 4 && fields[0] == serial + 1 && fields[1] == 0 && fields[2] == 0 && fields[3] & abi::record_flags::FAILED as u64 != 0, "18", format!(
        "COMPOSITE_RECORD of the failed composite {} answered `{record}`; it is recorded with 0 pixels, no windows and flags bit 0 (failed)", serial + 1))?;
    let (px, _) = screen()?;
    screen_is(&px, &[p], "the screen after the failed composite, still the previous commit")?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &[next], "the retried composite")
}

fn composition_inflight_bound() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let p = Placed::new(0, 0, 640, 400, 0, 18);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    w.frame(19).cite("17.3")?;
    // Two callers, each a thread with its own context and targets and no lock in common,
    // compose at once. The first the kernel admits pauses there with the window's front
    // pinned (point 16), so the other enters while it is in flight.
    let fired = |point| libbreenix::test_control::fired(point).map_err(|e| format!("FIRED: {e}"));
    let stalls = fired(abi::tc_point::COMPOSITE_STALL).cite("21")?;
    libbreenix::test_control::arm(abi::tc_point::COMPOSITE_STALL, w.id(), 0).map_err(|e| format!("ARM composite-stall: {e}")).cite("21")?;
    let both = k.ask("composite2");
    let stalled = fired(abi::tc_point::COMPOSITE_STALL).cite("21");
    let _ = libbreenix::test_control::disarm_all();
    let (both, stalled) = (both.cite("24")?, stalled? - stalls);
    ensure(stalled == 1, "21", format!("composite-stall fired {stalled} times during the two composites; the first admitted pauses once"))?;
    // Each answer: serial, pixels, then the caller's trace: from just before it asked for the
    // snapshot its composite used to just after that COMPOSITE returned, and how many times,
    // first when, the kernel refused it EBUSY.
    let parts: Vec<Vec<u64>> = both.split(", ").map(|s| s.strip_prefix("ok ").unwrap_or("").split(' ').filter_map(|n| n.parse().ok()).collect()).collect();
    ensure(parts.len() == 2 && parts.iter().all(|v| v.len() == 6), "18", format!(
        "two composites started at once answered `{both}`; each finishes, one after the other"))?;
    let (s1, s2) = (parts[0][0], parts[1][0]);
    ensure(s1.abs_diff(s2) == 1, "18", format!("two simultaneous composites have serials {s1} and {s2}; they run one at a time"))?;
    let (first, second) = if s1 < s2 { (&parts[0], &parts[1]) } else { (&parts[1], &parts[0]) };
    // The overlap: the kernel refused the second caller while it had admitted the first,
    // from the first's snapshot reservation to its composite's return.
    let (began, returned, refused, refused_at) = (first[2], first[3], second[4], second[5]);
    ensure(refused > 0 && began <= refused_at && refused_at <= returned, "18", format!(
        "composite {} was refused {refused} times, first at {refused_at} ns, while the kernel held composite {} from {began} to {returned} ns; \
         the second caller enters while the first is in flight and is held off", second[0], first[0]))?;
    // The window's generation was composed once: by whichever ran first.
    let (_, e1) = record(&mut k, first[0])?;
    let (_, e2) = record(&mut k, second[0])?;
    let p1 = e1.iter().find(|e| e.0 == w.id()).map_or(0, |e| e.3);
    let p2 = e2.iter().find(|e| e.0 == w.id()).map_or(0, |e| e.3);
    ensure(p1 == 640 * 400 && p2 == 0, "18", format!(
        "the window's new commit was composed {p1} pixels by composite {} and {p2} by composite {}; one composite composes it, the next has no damage",
        first[0], second[0]))
}

fn composition_no_change_commit() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let p = Placed::new(500, 300, 100, 80, 0, 21);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    let (s1, _) = compose(&mut k)?;
    let before = listed(w.id()).cite("17.1")?.ok_or("the window is not listed").cite("17.2")?;
    let mut n = abi::NoChange::ZERO;
    n.id = w.id();
    want_ok(call(op::COMMIT_NO_CHANGE, &mut n), "17.3", "COMMIT_NO_CHANGE")?;
    let after = listed(w.id()).cite("17.1")?.ok_or("the window is not listed").cite("17.2")?;
    ensure(n.commit_id == before.commit_id + 1 && after.pixel_gen == before.pixel_gen, "17.3", format!(
        "COMMIT_NO_CHANGE after commit {} gave commit {} and generation {} (it was {})", before.commit_id, n.commit_id, after.pixel_gen, before.pixel_gen))?;
    let (s2, pixels) = compose(&mut k)?;
    ensure(pixels == 0, "18", format!("the composite after COMMIT_NO_CHANGE processed {pixels} pixels; it causes no composite work"))?;
    let (_, e1) = record(&mut k, s1)?;
    let held = e1.iter().find(|e| e.0 == w.id()).map(|e| e.2);
    ensure(held.is_some_and(|g| g >= after.pixel_gen), "18", format!(
        "composite {s1}'s record holds the window at generation {held:?}; the no-change commit (generation {}) is presented by it", after.pixel_gen))?;
    let (_, e2) = record(&mut k, s2)?;
    ensure(e2.iter().find(|e| e.0 == w.id()).is_some_and(|e| e.3 == 0), "18", "the composite after COMMIT_NO_CHANGE composed the window's pixels again")
}

/// The pixels of `px` that differ from the formula, as (x, y).
fn differing(px: &[u8], scene: &[Placed]) -> Result<Vec<(u32, u32)>,breenix_suite::suite::CaseError> {
    let (width,height)=screen_size()?;
    let f = formula(scene);
    let mut reference=Vec::with_capacity(width*height*4);
    for y in 0..height as u32 {for x in 0..width as u32 {reference.extend_from_slice(&f(x,y));}}
    crate::remember_frame(px,width,height,Some(reference));
    let mut out = Vec::new();
    for y in 0..height as u32 {
        for x in 0..width as u32 {
            let i = ((y as usize * width + x as usize) * 4) as usize;
            if px[i..i + 4] != f(x, y) {
                out.push((x, y));
            }
        }
    }
    Ok(out)
}

fn cursor(k: &mut Helper, shape: u32, visible: bool, x: i32, y: i32) -> CaseResult {
    let r = k.ask(&format!("cursor {shape} {} {x} {y}", visible as u32)).cite("24")?;
    ensure(r == "ok", "18", format!("the compositor's SET_CURSOR({shape}, {visible}, {x}, {y}) answered `{r}`"))
}

fn composition_cursor_hidden() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let scene = [Placed::new(200, 200, 300, 200, 0, 22)];
    let w = W::new(300, 200, 2)?;
    show(&mut k, &w, &scene[0])?;
    cursor(&mut k, abi::cursor_shape::ARROW, false, 300, 300)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &scene, "the screen with the cursor hidden over a window")
}

fn composition_cursor_no_trail() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let scene = [Placed::new(100, 100, 600, 400, 0, 23)];
    let w = W::new(600, 400, 2)?;
    show(&mut k, &w, &scene[0])?;
    cursor(&mut k, abi::cursor_shape::ARROW, true, 300, 250)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    let first = differing(&px, &scene)?;
    ensure(!first.is_empty() && first.iter().all(|&(x, y)| x.abs_diff(300) < 32 && y.abs_diff(250) < 32), "18", format!(
        "with the arrow at (300, 250), {} pixels differ from the windows, the first at {:?}; the sprite is drawn there and nowhere else", first.len(), first.first()))?;
    for (x, y) in [(320, 260), (500, 400), (900, 700), (150, 120)] {
        cursor(&mut k, abi::cursor_shape::ARROW, true, x, y)?;
        compose(&mut k)?;
    }
    let (px, _) = screen()?;
    let trail: Vec<(u32, u32)> = differing(&px, &scene)?.into_iter().filter(|&(x, y)| x.abs_diff(150) >= 32 || y.abs_diff(120) >= 32).collect();
    ensure(trail.is_empty(), "18", format!("after the cursor moved on, {} pixels away from it still differ from the scene, the first at {:?}: a trail", trail.len(), trail.first()))?;
    cursor(&mut k, abi::cursor_shape::ARROW, false, 150, 120)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &scene, "the screen once the cursor is hidden again")
}

fn composition_cursor_shape() -> CaseResult {
    let (width,_)=screen_size()?;
    interface()?;
    let mut k = compositor()?;
    let mut sprites: Vec<Vec<(u32, u32, [u8; 4])>> = Vec::new();
    for shape in [abi::cursor_shape::ARROW, abi::cursor_shape::RESIZE_NS, abi::cursor_shape::RESIZE_EW,
        abi::cursor_shape::RESIZE_NWSE, abi::cursor_shape::RESIZE_NESW] {
        cursor(&mut k, shape, true, 640, 400)?;
        compose(&mut k)?;
        let (px, _) = screen()?;
        let drawn: Vec<(u32, u32, [u8; 4])> = differing(&px, &[])?.into_iter().map(|(x, y)| {
            let i = ((y as usize * width + x as usize) * 4) as usize;
            (x, y, [px[i], px[i + 1], px[i + 2], px[i + 3]])
        }).collect();
        ensure(!drawn.is_empty(), "18", format!("shape {shape} at (640, 400) drew no pixel over the black screen"))?;
        ensure(drawn.iter().all(|&(x, y, _)| x.abs_diff(640) < 32 && y.abs_diff(400) < 32), "18", format!(
            "shape {shape} drew pixels outside the 32-pixel box around the pointer, the first at {:?}", drawn.iter().find(|&&(x, y, _)| x.abs_diff(640) >= 32 || y.abs_diff(400) >= 32)))?;
        if let Some(same_as) = sprites.iter().position(|s| *s == drawn) {
            return at("18", format!("shape {shape} draws exactly what shape {same_as} draws; each shape is its own"));
        }
        sprites.push(drawn);
    }
    Ok(())
}

fn composition_cursor_stationary_repaint() -> CaseResult {
    let (width,_)=screen_size()?;
    interface()?;
    let mut k = compositor()?;
    let mut scene = [Placed::new(200, 150, 400, 300, 0, 24)];
    let w = W::new(400, 300, 2)?;
    show(&mut k, &w, &scene[0])?;
    cursor(&mut k, abi::cursor_shape::ARROW, true, 380, 290)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    let sprite: Vec<(u32, u32)> = differing(&px, &scene)?;
    ensure(!sprite.is_empty(), "18", "the visible arrow cursor at (380, 290) drew no pixel over the window")?;
    ensure(sprite.iter().all(|&(x, y)| x.abs_diff(380) < 32 && y.abs_diff(290) < 32), "18", format!(
        "the cursor at (380, 290) changed pixels outside the 32-pixel box around it, the first at {:?}",
        sprite.iter().find(|&&(x, y)| x.abs_diff(380) >= 32 || y.abs_diff(290) >= 32)))?;
    // New content under the stationary cursor.
    scene[0].seed = 25;
    w.frame(25).cite("17.3")?;
    compose(&mut k)?;
    let (px2, _) = screen()?;
    let now = differing(&px2, &scene)?;
    ensure(now == sprite, "18", format!(
        "after new content was committed under the stationary cursor, {} pixels differ from the new frame (the sprite covered {}); the content shows with the cursor redrawn above it",
        now.len(), sprite.len()))?;
    for &(x, y) in sprite.iter() {
        let i = ((y as usize * width + x as usize) * 4) as usize;
        ensure(px[i..i + 4] == px2[i..i + 4], "18", format!("the cursor sprite's pixel ({x}, {y}) changed when the content under it did"))?;
    }
    Ok(())
}

/// The GPU path gauge, including validation, translation, fences and readback copies.
fn irq_gauge() -> Result<u64, breenix_suite::suite::CaseError> {
    counter("gpu_irq_masked_max_us").cite("18")
}

// gpu-display: §2.2 keeps the calibrated 500 us bound over the entire GPU composition path.
fn composition_irq_bound() -> CaseResult {
    let (width,height)=screen_size()?;
    interface()?;
    let mut k = compositor()?;
    let p = Placed::new(0, 0, width as u32, height as u32, 0, 26);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    // Ruling B38: irq-hold is armed once this case's contexts and the compositor's are live,
    // their catalogue warm-up complete, so no warm-up step the device held back meanwhile
    // runs inside the measured intervals. As for gpu's textures/upload-visibility (ruling
    // 52), the case's 10 s window starts once they are; the runner bounds the warm-up.
    let start = now_ms();
    loop {
        let c = crate::gpu_checks::counters().cite("18")?;
        if c.warmed == c.contexts { break; }
        if since(start) >= 30_000 {
            return at("18", format!("gpu-display: {} of this case's {} GPU contexts finished their catalogue warm-up within 30 s", c.warmed, c.contexts));
        }
        let _ = time::sleep_ms(5);
    }
    let warm = k.ask_within("warm 30000", 31_000).cite("24")?;
    ensure(warm.starts_with("ok "), "18", format!("gpu-display: the compositor's GPU contexts did not finish their catalogue warm-up: kiosk_test answered `{warm}`"))?;
    breenix_suite::suite::start_case_window()?;
    libbreenix::test_control::arm(abi::tc_point::IRQ_HOLD, 0, 0).map_err(|e| format!("ARM irq-hold: {e}")).cite("21")?;
    w.frame(27).cite("17.3")?;
    let composed = compose(&mut k);
    let held = irq_gauge();
    let _ = libbreenix::test_control::disarm_all();
    composed?;
    let held = held?;
    value("irq-hold-gauge", held as i64, "us", Some((2_000, i64::MAX)));
    ensure(held >= 2_000, "18, 21", format!("with irq-hold armed the gauge reads {held} us; the 2 ms hold reads at least 2,000"))?;
    // DISARM_ALL restarted the gauge; now 20 composites of full-screen damage.
    let reset = irq_gauge()?;
    ensure(reset == 0, "21", format!("after DISARM_ALL the gauge reads {reset} us; it restarts at 0"))?;
    for i in 0..20 {
        w.frame(28 + i).cite("17.3")?;
        compose(&mut k)?;
        let _=screen()?;
    }
    let bound = irq_gauge()?;
    value("irq-masked-max", bound as i64, "us", Some((0, 500)));
    ensure(bound <= 500, "18", format!("over 20 composites of full-screen damage the longest IRQ-masked interval in GPU composition and readback was {bound} us; the bound is 500"))
}

fn composition_present_rate() -> CaseResult {
    interface()?;
    let (mut k, _) = kiosk().cite("24")?;
    let p = Placed::new(100, 100, 256, 256, 0, 0);
    let w = W::new(256, 256, 2)?;
    want_ok(w.register(b"present-rate"), "17.1", "REGISTER")?;
    place(&mut k, w.id(), &p, abi::geometry_flags::VISIBLE)?;
    let mut frame_times=Vec::new();
    let mut start = 0;
    let mut composites0 = 0;
    for i in 0..300u32 {
        if i == 30 {
            start = now_ms();
            composites0 = counter("composites").cite("18")?;
        }
        let began=time::now_monotonic()?.as_nanos();
        w.frame(i).cite("17.3")?;
        if i>=236 {frame_times.push(((time::now_monotonic()?.as_nanos()-began)/1000).min(u32::MAX as i128) as u32);}
    }
    let elapsed = since(start).max(1);
    let composed = counter("composites").cite("18")? - composites0;
    let rate = composed * 1_000 / elapsed;
    breenix_suite::suite::timing(&frame_times,16_667)?;
    value("composites-per-second", rate as i64, "fps", Some((60, i64::MAX)));
    ensure(rate >= 60, "18", format!("one 256x256 window committing as fast as its buffers are released was composed {composed} times in {elapsed} ms, {rate} a second; the contract requires at least 60"))
}

/// In a helper: say why it stops (on its standard output, which the case reads) and give the
/// exit status.
pub(crate) fn stop(why: String) -> i32 {
    let _ = io::write(Fd::from_raw(1), format!("failed: {why}\n").as_bytes());
    1
}

pub(crate) fn take_over() -> i64 {
    // SAFETY: take_over_display reads no memory.
    unsafe { raw::syscall0(SYS_TAKE_OVER_DISPLAY) as i64 }
}

pub(crate) fn give_back() -> i64 {
    // SAFETY: give_back_display reads no memory.
    unsafe { raw::syscall0(SYS_GIVE_BACK_DISPLAY) as i64 }
}

/// Draw and present a solid GPU target in the caller's display-owner context.
pub(crate) fn paint(colour: [u8; 4]) -> Result<(), String> {
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r)))?;
    let mode=modes.modes[modes.current as usize];
    let mut context=bgl::Context::new().map_err(|r|format!("gpu-display: context {}",outcome(r)))?;
    let target=libbreenix::gpu::scanout_target_create(0,mode.width,mode.height).map_err(|r|format!("gpu-display: scanout target {}",outcome(r)))?;
    context.begin(target,mode.width,mode.height);context.clear(u32::from_le_bytes([colour[0],colour[1],colour[2],255]));
    let result=context.submit().and_then(|f|f.wait(2_000_000_000)).and_then(|_|context.present(target)).map(|_|());
    libbreenix::gpu::resource_destroy(target);
    result.map_err(|r|format!("gpu-display: GPU draw/present {}",outcome(r)))
}

// gpu-display: B7 replaces retired fb_mmap/FLUSH painting with a GPU present; exact restoration stays.
fn composition_takeover_giveback() -> CaseResult {
    let (width,height)=screen_size()?;
    interface()?;
    let (before, _) = screen()?;
    let mut f = || {
        let took = take_over();
        if took < 0 {
            return stop(format!("take_over_display {}", outcome(took)));
        }
        if let Err(e) = paint([0x20, 0x80, 0xE0, 0]) {
            return stop(e);
        }
        let _ = io::write(Fd::from_raw(1), b"painted\n");
        let mut b = [0u8; 1];
        let _ = io::read(Fd::from_raw(0), &mut b);
        let gave = give_back();
        if gave < 0 {
            return stop(format!("give_back_display {}", outcome(gave)));
        }
        let _ = io::write(Fd::from_raw(1), b"given\n");
        0
    };
    let mut h = Helper::call(&mut f).cite("24")?;
    let said = h.line(5_000).cite("24")?;
    ensure(said == "painted", "18", format!("the display-taking child said `{said}`"))?;
    let (during, _) = screen()?;
    ensure(during[..4] == [0x20, 0x80, 0xE0, 0xFF], "18", format!("while the child owns the display, the screen's first pixel is {:02x?}, not the child's colour", &during[..4]))?;
    let given = h.ask("go").cite("24")?;
    ensure(given == "given", "18", format!("the child's give_back_display: `{given}`"))?;
    let (after, _) = screen()?;
    crate::remember_frame(&after,width,height,Some(before.clone()));
    match (0..width * height).find(|&i| before[i * 4..i * 4 + 4] != after[i * 4..i * 4 + 4]) {
        None => Ok(()),
        Some(i) => at("18", format!("after giveback the screen differs from the previous owner's frame at ({}, {}): {:02x?} against {:02x?}",
            i % width, i / width, &after[i * 4..i * 4 + 4], &before[i * 4..i * 4 + 4])),
    }
}

/// The case already owns one stack entry; three descendants fit and the fourth is EBUSY.
fn nested_takeover(depth: u32) -> i32 {
    let ret = take_over();
    if depth == 4 { return if ret == -(EBUSY as i64) { 0 } else { 1 }; }
    if ret != 0 { return 2; }
    let mut body = || nested_takeover(depth + 1);
    let Ok(pid) = breenix_suite::suite::spawn(&[], breenix_suite::suite::Run::Call(&mut body)) else { return 3; };
    let mut status = 0;
    if process::waitpid(pid.raw() as i32, &mut status, 0).is_err() { return 4; }
    if !process::wifexited(status) || process::wexitstatus(status) != 0 { return 5; }
    if give_back() != 0 { return 6; }
    0
}

fn ownership_refusals() -> CaseResult {
    let mut body = || nested_takeover(1);
    let pid = breenix_suite::suite::spawn(&[], breenix_suite::suite::Run::Call(&mut body))?;
    let mut status = 0;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    ensure(process::wifexited(status) && process::wexitstatus(status) == 0,
        "18", "display ownership nesting did not refuse depth four with EBUSY")?;
    child("uid-1000 display takeover", 3_000, Some(1000), || {
        let ret = take_over();
        if ret == -(EPERM as i64) { Ok(()) }
        else { Err(format!("unprivileged TAKE_OVER_DISPLAY {}; expected EPERM", outcome(ret))) }
    }).cite("18")
}

// gpu-display: B7 replaces the giver's framebuffer paint with GPU PRESENT; owner death restores that binding.
fn composition_ownership_stack() -> CaseResult {
    let (width,height)=screen_size()?;
    interface()?;
    ownership_refusals()?;
    // The giver: takes the display, paints it, and hands it to a compositor it names.
    let mut giver_body = || {
        let took = take_over();
        if took < 0 {
            return stop(format!("take_over_display {}", outcome(took)));
        }
        if let Err(e) = paint([0x11, 0x99, 0x33, 0]) {
            return stop(e);
        }
        let _ = io::write(Fd::from_raw(1), format!("painted {}\n", getpid()).as_bytes());
        let mut line = [0u8; 32];
        let n = io::read(Fd::from_raw(0), &mut line).unwrap_or(0);
        let pid: u32 = core::str::from_utf8(&line[..n]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut g = abi::Give { pid, reserved: 0 };
        let r = call(op::GIVE_DISPLAY, &mut g);
        let _ = io::write(Fd::from_raw(1), format!("gave {r}\n").as_bytes());
        let _ = io::read(Fd::from_raw(0), &mut line);
        0
    };
    let mut giver = Helper::call(&mut giver_body).cite("24")?;
    let said = giver.line(5_000).cite("24")?;
    ensure(said.starts_with("painted "), "18", format!("the giver said `{said}`"))?;
    let mut k = compositor()?;
    let gave = giver.ask(&k.pid.to_string()).cite("24")?;
    ensure(gave == "gave 0", "18", format!("GIVE_DISPLAY to the compositor answered `{gave}`"))?;
    let p = Placed::new(100, 100, 200, 200, 0, 28);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    compose(&mut k)?;
    let (ret,binding)=libbreenix::gpu::modes();
    ensure(ret==0 && binding.binding_known==1 && binding.scanout_resource!=0,"18","gpu-display: ownership stack has no completed GPU binding")?;
    let (px, _) = screen()?;
    crate::remember_frame(&px,width,height,None);
    ensure(px[(150 * width + 150) * 4..(150 * width + 150) * 4 + 3] == pattern(28, 50, 50)[..3], "18", "the compositor given the display does not show its composition")?;
    k.kill_checked().cite("24")?;
    let _ = time::sleep_ms(50);
    let (ret,restored)=libbreenix::gpu::modes();
    ensure(ret==0 && restored.binding_known==1 && restored.scanout_resource!=binding.scanout_resource,"18","gpu-display: compositor death did not restore the giver's GPU target")?;
    let (px, _) = screen()?;
    crate::remember_frame(&px,width,height,None);
    ensure(px[..4] == [0x11, 0x99, 0x33, 0xFF] && px[(150 * width + 150) * 4..(150 * width + 150) * 4 + 4] == [0x11, 0x99, 0x33, 0xFF], "18", format!(
        "after the compositor died the screen shows {:02x?} at (0, 0); the display returns to the giver and its frame is restored exactly", &px[..4]))
}


fn compositor_gpu_counts(k:&mut Helper)->Result<Vec<u64>,breenix_suite::suite::CaseError>{
    let line=k.ask("gpu-counters").cite("24")?;
    let nums=line.strip_prefix("ok ").ok_or_else(||format!("gpu-display: compositor COUNTERS: {line}"))?
        .split_whitespace().map(str::parse).collect::<Result<Vec<u64>,_>>().map_err(|e|e.to_string())?;
    ensure(nums.len()==6,"18","gpu-display: incomplete compositor counters")?;Ok(nums)
}
// gpu-display: §2.2 requires actual GPU draws, no client-pixel upload by the compositor, and exact composed pixels.
fn composition_gpu_composite()->CaseResult {
    interface()?;let mut k=compositor()?;
    let p=Placed::new(20,30,64,48,0,9);let w=W::new(64,48,2)?;show(&mut k,&w,&p)?;
    let before=compositor_gpu_counts(&mut k)?;compose(&mut k)?;let after=compositor_gpu_counts(&mut k)?;
    ensure(after[0]>before[0] && after[1]-before[1]<=4 && after[2]==before[2] && after[3]==before[3],"18",
        "gpu-display: composition did not use bounded GPU instances without CPU pixel uploads")?;
    let (pixels,_)=screen()?;screen_is(&pixels,&[p],"GPU composition")
}

pub(crate) struct GpuSnapshot {pub width:usize,pub height:usize,pub record:[u8;1024],pub pixels:Vec<u8>}
impl GpuSnapshot {
    pub(crate) fn word(&self,o:usize)->u32 {u32::from_le_bytes(self.record[o..o+4].try_into().unwrap())}
    pub(crate) fn quad(&self,o:usize)->u64 {u64::from_le_bytes(self.record[o..o+8].try_into().unwrap())}
}
/// Ask the actual display-owning helper to observe target 0; no foreign handle or CPU image is substituted.
pub(crate) fn gpu_observe(k:&mut Helper)->Result<(i64,u32,GpuSnapshot),breenix_suite::suite::CaseError>{
    static NEXT:AtomicU64=AtomicU64::new(0);
    let path=format!("/dev/shm/breengel-gpu-{}-{}",process::getpid()?.raw(),NEXT.fetch_add(1,Relaxed));
    struct Remove<'a>(&'a str);impl Drop for Remove<'_>{fn drop(&mut self){let _=std::fs::remove_file(self.0);}}
    let _remove=Remove(&path);
    let answer=k.ask(&format!("gpu-scanout {path}")).cite("24")?;
    let (status,reason)=if answer=="ok" {(0,0)}else{
        let fields:Vec<_>=answer.split_whitespace().collect();
        ensure(fields.len()==4 && fields[0]=="err" && fields[2]=="reason","20",format!("gpu-display: composed GPU scanout readback: {answer}"))?;
        (-fields[1].parse::<i64>().map_err(|e|e.to_string())?,fields[3].parse::<u32>().map_err(|e|e.to_string())?)
    };
    let bytes=std::fs::read(&path).map_err(|e|e.to_string())?;
    ensure(bytes.len()>=1032,"20","gpu-display: truncated GPU snapshot")?;
    let width=u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let height=u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    ensure(matches!((width,height),(1280,800)|(2560,1600)) && bytes.len()==1032+width*height*4,"20","gpu-display: snapshot does not cover its entire mode")?;
    let record=bytes[8..1032].try_into().unwrap();
    let snapshot=GpuSnapshot{width,height,record,pixels:bytes[1032..].to_vec()};
    Ok((status,reason,snapshot))
}
// gpu-display: R4 applies logical scale to placement and compares the entire device destination with §18's formula.
fn composition_scale_2()->CaseResult {
    interface()?;let mut k=compositor()?;
    // Unlike the legacy physical-pixel scenes, this fixture has logical geometry.
    // Clients allocate physical targets; SET_GEOMETRY also takes physical positions.
    let scene=[Placed::new(40,60,128,96,0,11),Placed::new(100,100,160,80,1,12),
        Placed::new(-20,780,100,40,2,13)];
    let wins:Vec<_>=scene.iter().map(|p|W::new(p.w*2,p.h*2,2)).collect::<Result<_,_>>()?;
    let mut context=bgl::Context::new().map_err(|r|format!("gpu-display: context {}",outcome(r))).cite("18")?;
    for (w,p) in wins.iter().zip(&scene){
        want_ok(w.register(b"scale-2"),"17.1","REGISTER")?;
        let acquired=w.acquire(2_000_000_000).map_err(|r|format!("ACQUIRE {}",outcome(r))).cite("17.3")?;
        // A source-window fixture, not an uploaded expected composite. Device scaling
        // makes its 2x pixel blocks; the compositor only receives physical targets.
        use libbreenix::gpu::{self,abi as ga};
        struct Texture(u64);
        impl Drop for Texture {fn drop(&mut self){gpu::resource_destroy(self.0);}}
        let texture=Texture(gpu::texture_create(ga::format::B8G8R8A8,p.w,p.h)
            .map_err(|r|format!("gpu-display: source texture {}",outcome(r))).cite("18")?);
        let source:Vec<u8>=(0..p.h).flat_map(|y|(0..p.w).flat_map(move|x|{
            let c=pattern(p.seed,x,y);[c[0],c[1],c[2],255]
        })).collect();
        let (uploaded,_)=gpu::upload(context.handle(),texture.0,[0,0,p.w,p.h],&source,p.w*4,
            ga::upload_source::B8G8R8A8,ga::purpose::FIXTURE);
        want_ok(uploaded,"18","gpu-display: source-window fixture upload")?;
        context.begin(w.c.buffer[acquired.buffer as usize],w.c.width,w.c.height);
        let mut params=[0;16];params[4]=2;
        context.draw(bgl::pipeline::COPY_SCALED,0,texture.0,0,0,
            &[bgl::Instance{rect:[0,0,w.c.width as i32,w.c.height as i32],params}]);
        context.submit().and_then(|f|f.wait(2_000_000_000)).map_err(|r|format!("gpu-display: scale-2 source draw {}",outcome(r))).cite("18")?;
        w.commit(acquired.buffer,&[full(&w.c)]).map_err(|r|format!("COMMIT {}",outcome(r))).cite("17.3")?;
        let physical=Placed::new(p.x*2,p.y*2,p.w*2,p.h*2,p.z,p.seed);
        place(&mut k,w.id(),&physical,abi::geometry_flags::VISIBLE)?;
    }
    compose(&mut k)?;
    let s=gpu_snapshot(&mut k)?;
    ensure((s.width,s.height)==(2560,1600),"18","gpu-display: scale-2 composition has the wrong destination size")?;
    same(&s.pixels,s.width as u32,s.height as u32,true,"scaled GPU composition","18",|x,y|formula(&scene)(x/2,y/2))
}

// gpu-display: §2.2 retained fronts feed composition, capture and replay after handle drop; scanout holds backing too.
fn composition_target_lifetime()->CaseResult {
    // The baseline comes before the case allocates anything: the fixture's own destroyed
    // targets keep their backing until the device answers their release (ruling B38). The
    // window fixture keeps its renderer and one decoded input pattern for the process's life;
    // they are made first, with the pattern the case ends on, so the baseline holds them.
    let warm=libbreenix::gpu::target_create(0,1,1).map_err(|r|format!("fixture warm-up target {}",outcome(r))).cite("18")?;
    let warmed=fill(FixtureTarget{handle:warm,width:1,height:1},1,1,17);
    libbreenix::gpu::resource_destroy(warm);
    warmed.cite("18")?;
    let before=crate::gpu_checks::counters().cite("18")?;
    super::handoff::snapshot_lifetime(false)?;
    interface()?;
    let mut k=compositor()?;let mut w=W::new(64,48,3)?;
    let p=Placed::new(30,40,64,48,0,17);show(&mut k,&w,&p)?;
    let front=listed(w.id()).cite("17.1")?.ok_or("window not listed")?;
    // The latest commit's index is returned by ACQUIRE/COMMIT; make one known front.
    let a=w.acquire(1_000_000_000).map_err(|r|format!("ACQUIRE {}",outcome(r))).cite("17.3")?;
    fill(w.target(a.buffer),64,48,17)?;
    let commit=w.commit(a.buffer,&[full(&w.c)]).map_err(|r|format!("COMMIT {}",outcome(r))).cite("17.3")?;
    let held=crate::gpu_checks::counters().cite("18")?;
    want_ok(libbreenix::gpu::resource_destroy(w.c.buffer[a.buffer as usize]),"18","gpu-display: drop committed target handle")?;
    ensure(crate::gpu_checks::counters().cite("18")?.resource_bytes==held.resource_bytes,"18","gpu-display: committed target backing freed before its holders")?;
    compose(&mut k)?;let (pixels,_)=screen()?;screen_is(&pixels,&[p],"composition after client target handle destruction")?;
    let (captured,pixels)=capture(w.id(),64,48,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
    ensure(captured.commit_id==commit.commit_id && commit.commit_id>front.commit_id,"18","gpu-display: capture lost its retained commit")?;
    same(&pixels,64,48,false,"capture of retained target","18",|x,y|pattern(17,x,y))?;
    let replay=w.acquire(1_000_000_000).map_err(|r|format!("ACQUIRE {}",outcome(r))).cite("17.3")?;
    ensure(replay.base==commit.commit_id,"18","gpu-display: replay lost the retained target's commit")?;
    let mut pixels=vec![0;64*48*4];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
    want_ok(libbreenix::gpu::readback(w.c.buffer[replay.buffer as usize],[0,0,64,48],&mut pixels,256,&mut record),"18","gpu-display: replayed target readback")?;
    same(&pixels,64,48,false,"replay from retained target","18",|x,y|pattern(17,x,y))?;
    want_ok(w.destroy(),"17.2","DESTROY")?;k.kill_checked().cite("24")?;
    let start=now_ms();loop {
        let now=crate::gpu_checks::counters().cite("18")?.resource_bytes;
        if now==before.resource_bytes { break; }
        if since(start)>=1_000 { return at("18",format!("gpu-display: released window/capture/replay/compositor holds leaked backing: {now} resource bytes 1 s after release, {} before the case",before.resource_bytes)); }
        let _=time::sleep_ms(5);
    }
    // A scanout binding retains its resource independently of its destroyed user handle.
    ensure(take_over()>=0,"18","gpu-display: take display for scanout lifetime")?;
    let result=(||->CaseResult{
        let mode=bgl::modes().map_err(|r|format!("MODES {}",outcome(r))).cite("18")?;let m=mode.modes[mode.current as usize];
        let mut c=bgl::Context::new().map_err(|r|format!("context {}",outcome(r))).cite("18")?;
        let target=libbreenix::gpu::scanout_target_create(0,m.width,m.height).map_err(|r|format!("scanout target {}",outcome(r))).cite("18")?;
        c.begin(target,m.width,m.height);c.clear(0xff112233);c.submit().and_then(|f|f.wait(2_000_000_000)).and_then(|_|c.present(target)).map_err(|r|format!("PRESENT {}",outcome(r))).cite("18")?;
        let held=crate::gpu_checks::counters().cite("18")?.resource_bytes;
        want_ok(libbreenix::gpu::resource_destroy(target),"18","RESOURCE_DESTROY bound scanout")?;
        ensure(crate::gpu_checks::counters().cite("18")?.resource_bytes==held,"18","gpu-display: scanout freed backing before replacement")?;
        let mut px=vec![0;m.width as usize*m.height as usize*4];let mut rec=[0;1024];
        want_ok(libbreenix::gpu::readback(0,[0,0,m.width,m.height],&mut px,m.width*4,&mut rec),"18","read bound target after handle destruction")?;
        ensure(px.chunks_exact(4).all(|p|p==[51,34,17,255]),"18","gpu-display: retained scanout pixels changed")
    })();
    let gave=give_back();result?;ensure(gave>=0,"18","gpu-display: release scanout hold")?;
    let start=now_ms();while crate::gpu_checks::counters().cite("18")?.resource_bytes!=before.resource_bytes {
        ensure(since(start)<1_000,"18","gpu-display: final scanout release leaked backing")?;let _=time::sleep_ms(5);
    }Ok(())
}

pub(crate) fn gpu_snapshot(k:&mut Helper)->Result<GpuSnapshot,breenix_suite::suite::CaseError>{
    let (status,reason,s)=gpu_observe(k)?;
    ensure(status==0,"20",format!("gpu-display: GPU readback {} reason {reason}",outcome(status)))?;
    ensure(s.word(0)==2 && s.word(4)==1024,"20","gpu-display: scanout did not return record v2")?;Ok(s)
}
