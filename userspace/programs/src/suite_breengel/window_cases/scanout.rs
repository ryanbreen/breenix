//! scanout: display observation on every target (contract section 20).

use crate::*;
use crate::show::fbinfo;

pub(crate) const SCANOUT: &[Case] = &[
    pictured!("mode-set", "A GUI suite boot runs this target's display at 1280x800, and fbinfo agrees", scanout_mode_set),
    pictured!("device-path", "Readback uses this backend's observation path from the contract and names it", scanout_device_path),
    pictured!("changing-scene", "Three successive known scenes each read back equal to their formulas", scanout_changing_scene),
    pictured!("full-destination", "Readback covers the whole destination, and a change in its last row is seen", scanout_full_destination),
    pictured!("binding", "The record names the scanout resource or address the device is bound to, and it is the composed one", scanout_binding),
    pictured!("completion", "The record carries answered completion for every damaged rectangle of its composite serial", scanout_completion),
    pictured!("atomic-record", "The pixels and the record always belong to the same composite serial under continuous commits", scanout_atomic_record),
    pictured!("window-commits", "The record lists the commit id of each window composed", scanout_window_commits),
    pictured!("format-canonical", "RGB and BGR destinations read back as the same canonical BGRX pixels", scanout_format_canonical),
    pictured!("root-only", "A non-root, non-compositor process cannot read back the scanout", scanout_root_only),
    pictured!("fault-skipped-transfer", "A skipped transfer or update is reported as missing completion, and the next composite recovers", scanout_fault_skipped_transfer),
    pictured!("fault-wrong-resource", "A flush or scanout bound to the wrong resource is reported as a binding mismatch", scanout_fault_wrong_resource),
    pictured!("fault-stale-fence", "A fence answered for another serial is reported as a serial mismatch", scanout_fault_stale_fence),
    pictured!("fault-outside-damage", "Corruption written outside any damage is found by comparing the full destination", scanout_fault_outside_damage),
    pictured!("boundary-stated", "The record states what lies outside the guest's view for this backend", scanout_boundary_stated),
];

// ---------------------------------------------------------------------------------------
// scanout: display observation on every target (contract section 20)
// ---------------------------------------------------------------------------------------

use super::composition::{compose, compositor, formula, place, screen, screen_is, show, Placed};
use super::kit::*;
use abi::{backend, op, Plain};

/// The boundary code section 20.2 gives each backend.
fn boundary_of(code: u32) -> Option<u32> {
    match code {
        backend::VIRTIO_GPU_2D => Some(1),
        backend::GOP_FRAMEBUFFER => Some(2),
        backend::VIRGL | backend::SVGA3_SCREEN_TARGET => Some(3),
        _ => None,
    }
}

/// A one-window scene, composed: the compositor, the window and its composite serial.
fn one_window(seed: u32) -> Result<(Helper, W, Placed, u64), libbreenix::suite::CaseError> {
    let mut k = compositor()?;
    let p = Placed::new(320, 200, 200, 120, 0, seed);
    let w = W::new(p.w, p.h, 2)?;
    show(&mut k, &w, &p)?;
    let (serial, _) = compose(&mut k)?;
    Ok((k, w, p, serial))
}

fn readback_raw() -> (i64, Vec<u8>, abi::ObservationRecord) {
    let mut px = vec![0u8; 1280 * 800 * 4];
    let mut rec = abi::ObservationRecord::ZERO;
    let mut rb = abi::Readback::ZERO;
    rb.dst = px.as_mut_ptr() as u64;
    rb.dst_len = px.len() as u64;
    rb.record = &mut rec as *mut abi::ObservationRecord as u64;
    rb.record_len = core::mem::size_of::<abi::ObservationRecord>() as u32;
    let r = call(op::SCANOUT_READBACK, &mut rb);
    (r, px, rec)
}

fn scanout_mode_set() -> CaseResult {
    let i = fbinfo().cite("18")?;
    ensure((i.width, i.height) == (1280, 800), "18, 20.2", format!("fbinfo reports {}x{}; the contract requires 1280x800 in a GUI suite boot", i.width, i.height))?;
    interface()?;
    let (_k, _w, _p, _) = one_window(1)?;
    let (_, rec) = screen()?;
    ensure((rec.width, rec.height) == (1280, 800) && rec.stride >= 1280 * 4, "20.1", format!(
        "the observation record reports {}x{} with stride {}; the display runs at 1280x800", rec.width, rec.height, rec.stride))
}

fn scanout_device_path() -> CaseResult {
    interface()?;
    let (_k, w, _p, _) = one_window(2)?;
    let (_, rec) = screen()?;
    let listed = listed(w.id()).cite("17.1")?.ok_or("the window is not listed").cite("17.2")?;
    value("backend", rec.backend as i64, "", Some((1, 4)));
    ensure(rec.backend == listed.backend && boundary_of(rec.backend).is_some(), "20.2", format!(
        "the observation record names backend {} ({}) and WinInfo names {}; one of the four backends of section 20.2 observes this target",
        rec.backend, backend::name(rec.backend), listed.backend))?;
    let arm64 = [backend::VIRTIO_GPU_2D, backend::SVGA3_SCREEN_TARGET, backend::VIRGL];
    ensure(arm64.contains(&rec.backend), "20.2", format!("backend {} ({}) is not one of this architecture's", rec.backend, backend::name(rec.backend)))
}

fn scanout_changing_scene() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let scenes = [
        vec![Placed::new(10, 10, 300, 200, 0, 31)],
        vec![Placed::new(400, 300, 500, 300, 0, 32), Placed::new(600, 100, 200, 400, 1, 33)],
        vec![Placed::new(0, 0, 1280, 800, 0, 34), Placed::new(900, 500, 300, 250, 1, 35)],
    ];
    for (n, scene) in scenes.iter().enumerate() {
        let wins: Vec<W> = scene.iter().map(|p| W::new(p.w, p.h, 2)).collect::<Result<_, _>>()?;
        for (w, p) in wins.iter().zip(scene.iter()) {
            show(&mut k, w, p)?;
        }
        compose(&mut k)?;
        let (px, _) = screen()?;
        screen_is(&px, scene, &format!("scene {}", n + 1))?;
        drop(wins);
        compose(&mut k)?;
    }
    Ok(())
}

fn scanout_full_destination() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let mut strip = Placed::new(0, 799, 1280, 1, 0, 36);
    let w = W::new(1280, 1, 2)?;
    show(&mut k, &w, &strip)?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &[strip], "the screen with a strip in its last row")?;
    strip.seed = 37;
    w.frame(37).cite("17.3")?;
    compose(&mut k)?;
    let (px, _) = screen()?;
    screen_is(&px, &[strip], "the screen after the last row changed")
}

fn scanout_binding() -> CaseResult {
    interface()?;
    let (mut k, w, _p, s1) = one_window(3)?;
    let (_, r1) = screen()?;
    ensure(r1.serial == s1 && r1.resource != 0, "20.1", format!("the record after composite {s1} names serial {} and resource {:#x}", r1.serial, r1.resource))?;
    w.frame(4).cite("17.3")?;
    let (s2, _) = compose(&mut k)?;
    let (_, r2) = screen()?;
    ensure(r2.serial == s2 && r2.resource == r1.resource, "20.1", format!(
        "the record after composite {s2} names serial {} and resource {:#x}; the display stays bound to the composed resource {:#x}",
        r2.serial, r2.resource, r1.resource))
}

fn scanout_completion() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let a = Placed::new(100, 100, 100, 100, 0, 5);
    let b = Placed::new(800, 500, 100, 100, 0, 6);
    let wa = W::new(100, 100, 2)?;
    let wb = W::new(100, 100, 2)?;
    show(&mut k, &wa, &a)?;
    show(&mut k, &wb, &b)?;
    compose(&mut k)?;
    // Two disjoint damaged rectangles in one composite.
    wa.frame(7).cite("17.3")?;
    wb.frame(8).cite("17.3")?;
    let (serial, _) = compose(&mut k)?;
    let (_, rec) = screen()?;
    let done = &rec.completions[..(rec.completion_count as usize).min(12)];
    ensure(rec.serial == serial && !done.is_empty() && done.iter().all(|c| c.fence != 0), "20.1", format!(
        "the record of composite {serial} (serial {}) carries completions {done:?}; each answered completion names its fence", rec.serial))?;
    let fences: std::collections::BTreeSet<u64> = done.iter().map(|c| c.fence).collect();
    ensure(fences.len() == done.len(), "20.1", format!("the record's completions repeat a fence: {done:?}"))?;
    ensure(done.iter().all(|c| c.kind != 0 && c.status != 0), "20.1", format!(
        "the record's completions {done:?} include one that names no command or no answer"))?;
    if rec.backend == backend::VIRTIO_GPU_2D {
        use abi::completion::{RESOURCE_FLUSH, RESP_OK_NODATA, TRANSFER_TO_HOST_2D};
        let kinds: Vec<u32> = done.iter().map(|c| c.kind).collect();
        ensure(kinds == [TRANSFER_TO_HOST_2D, TRANSFER_TO_HOST_2D, RESOURCE_FLUSH], "20.2", format!(
            "virtio-gpu 2D answered commands {kinds:#06x?} for a composite of two damaged rectangles; it answers a TRANSFER_TO_HOST_2D (0x0105) per rectangle, then one RESOURCE_FLUSH (0x0104)"))?;
        ensure(done.iter().all(|c| c.status == RESP_OK_NODATA), "20.2", format!(
            "virtio-gpu 2D answered {:#06x?}; each command is answered OK_NODATA (0x1100)", done.iter().map(|c| c.status).collect::<Vec<_>>()))?;
    }
    if rec.backend == backend::SVGA3_SCREEN_TARGET {
        use abi::completion::{SVGA3_CB_COMPLETED, SVGA3_READBACK_GB_SURFACE, SVGA3_UPDATE_GB_SCREENTARGET};
        let kinds: Vec<u32> = done.iter().map(|c| c.kind).collect();
        ensure(kinds == [SVGA3_UPDATE_GB_SCREENTARGET, SVGA3_UPDATE_GB_SCREENTARGET, SVGA3_READBACK_GB_SURFACE], "20.2", format!(
            "SVGA3 answered commands {kinds:?} for a readback of a composite of two damaged rectangles; it answers an UPDATE_GB_SCREENTARGET (1127) per rectangle, then the readback's READBACK_GB_SURFACE (1104)"))?;
        ensure(done.iter().all(|c| c.status == SVGA3_CB_COMPLETED), "20.2", format!(
            "SVGA3 answered {:?}; each command buffer completes with status COMPLETED (1)", done.iter().map(|c| c.status).collect::<Vec<_>>()))?;
    }
    Ok(())
}

fn scanout_atomic_record() -> CaseResult {
    interface()?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = k.ask(&format!("cursor {} 0 0 0", abi::cursor_shape::HIDDEN)).cite("24")?;
    let w = std::sync::Arc::new(W::new(64, 64, 2)?);
    want_ok(w.register(b"atomic-record"), "17.1", "REGISTER")?;
    place(&mut k, w.id(), &Placed::new(600, 400, 64, 64, 0, 0), abi::geometry_flags::VISIBLE)?;
    // Commit n shows pattern n, so the pixels name the commit they belong to. Commit 1 is
    // composed before the continuous commits start, so every readback names a commit.
    w.frame(1).cite("17.3")?;
    let start = now_ms();
    loop {
        let (r, _, rec) = readback_raw();
        let listed = rec.windows[..(rec.window_count as usize).min(16)].iter().any(|o| o.id == w.id() && o.commit_id >= 1);
        if r == 0 && listed {
            break;
        }
        if since(start) > 1_000 {
            return at("20.1", "no readback listed the window's first commit within 1 s");
        }
        let _ = time::sleep_ms(5);
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client = {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || -> Result<u32, String> {
            let mut n = 2;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                w.frame(n).map_err(|e| format!("commit {n}: {e}"))?;
                n += 1;
            }
            Ok(n - 2)
        })
    };
    let (mut checked, mut last, mut seen) = (0, 0u64, Vec::new());
    let mut result = Ok(());
    for _ in 0..20 {
        let _ = time::sleep_ms(10);
        let (r, px, rec) = readback_raw();
        if r == -(EAGAIN as i64) {
            continue;
        }
        if r < 0 {
            result = at("20.1", format!("SCANOUT_READBACK under continuous commits {}", outcome(r)));
            break;
        }
        crate::remember_frame(&px,1280,800,None);
        let listed = rec.windows[..(rec.window_count as usize).min(16)].iter().find(|o| o.id == w.id()).map(|o| o.commit_id);
        let Some(commit) = listed else {
            result = at("20.1", format!("the record of serial {} does not list the committing window", rec.serial));
            break;
        };
        if commit < last.max(1) {
            result = at("20.1", format!("readback serial {} names commit {commit} after a readback named commit {last}", rec.serial));
            break;
        }
        // The window is at (600, 400); every one of its pixels must be the named commit's.
        let differ = (0..64u32).flat_map(|y| (0..64u32).map(move |x| (x, y))).find(|&(x, y)| {
            let at = ((400 + y) * 1280 + 600 + x) as usize * 4;
            px[at..at + 3] != pattern(commit as u32, x, y)[..3]
        });
        if let Some((x, y)) = differ {
            result = at("20.1", format!("readback serial {} names commit {commit}, but the window's pixel ({x},{y}) is not commit {commit}'s", rec.serial));
            break;
        }
        last = commit;
        if !seen.contains(&commit) {
            seen.push(commit);
        }
        checked += 1;
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let made = client.join().map_err(|_| String::from("the committing thread panicked")).cite("17.3")?;
    result?;
    let made = made.cite("17.3")?;
    value("distinct-commits", seen.len() as i64, "", Some((5, 20)));
    ensure(checked >= 10, "20.1", format!("only {checked} of 20 readbacks under continuous commits returned pixels"))?;
    ensure(seen.len() >= 5, "20.1", format!(
        "{checked} readbacks named only {} distinct commits ({seen:?}) while the client made {made}; under continuous commits the scanout moves on", seen.len()))
}

fn scanout_window_commits() -> CaseResult {
    interface()?;
    let mut k = compositor()?;
    let scene = [Placed::new(50, 50, 100, 100, 0, 9), Placed::new(300, 50, 100, 100, 1, 10), Placed::new(550, 50, 100, 100, 2, 11)];
    let wins: Vec<W> = scene.iter().map(|p| W::new(p.w, p.h, 2)).collect::<Result<_, _>>()?;
    for (w, p) in wins.iter().zip(scene.iter()) {
        show(&mut k, w, p)?;
    }
    wins[1].frame(12).cite("17.3")?;
    compose(&mut k)?;
    let (_, rec) = screen()?;
    let observed = &rec.windows[..(rec.window_count as usize).min(16)];
    for w in wins.iter() {
        let i = listed(w.id()).cite("17.1")?.ok_or("a window is not listed").cite("17.2")?;
        ensure(observed.iter().any(|o| o.id == w.id() && o.commit_id == i.commit_id), "20.1", format!(
            "the record lists {observed:?}; window {} was composed at commit {}", w.id(), i.commit_id))?;
    }
    Ok(())
}

fn scanout_format_canonical() -> CaseResult {
    interface()?;
    let (_k, _w, p, _) = one_window(13)?;
    let (px, rec) = screen()?;
    ensure(rec.source_format <= 1, "20.1", format!("the record names source format {}; it is 0 (RGB) or 1 (BGR)", rec.source_format))?;
    // Canonical BGRX with X 0xFF, whichever the destination's order.
    same(&px, 1280, 800, true, &format!("the readback of a {} destination", if rec.source_format == 0 { "RGB" } else { "BGR" }), "20.1", formula(&[p]))
}

fn scanout_root_only() -> CaseResult {
    interface()?;
    let (_k, _w, _p, _) = one_window(14)?;
    step("17.4", child("SCANOUT_READBACK as uid 1000", 5_000, Some(1000), || {
        let (r, px, _) = readback_raw();
        if r != -(EPERM as i64) {
            return Err(format!("a uid 1000 process that is not the compositor: SCANOUT_READBACK {}; the contract gives EPERM", outcome(r)));
        }
        if px.iter().any(|&b| b != 0) {
            return Err(String::from("a refused SCANOUT_READBACK wrote pixels"));
        }
        Ok(())
    }))?;
    let (r, _, _) = readback_raw();
    ensure(r == 0, "17.4", format!("root's SCANOUT_READBACK {}; root may read back", outcome(r)))
}

/// Arm `point`, commit and compose, read back; then disarm, compose again and require a clean
/// readback of the new frame. Returns the faulted readback's return, pixels and record.
fn fault(point: u32, seed: u32) -> Result<(i64, Vec<u8>, abi::ObservationRecord, u64, abi::ObservationRecord), libbreenix::suite::CaseError> {
    let (mut k, w, mut p, _) = one_window(seed)?;
    let (_, good) = screen()?;
    p.seed = seed + 1;
    w.frame(p.seed).cite("17.3")?;
    libbreenix::test_control::arm(point, 0, 0).map_err(|e| format!("ARM point {point}: {e}")).cite("21")?;
    let (serial, _) = compose(&mut k)?;
    let (r, px, rec) = readback_raw();
    let _ = libbreenix::test_control::disarm_all();
    let fired = libbreenix::test_control::fired(point).unwrap_or(0);
    ensure(fired >= 1, "21", format!("test-control point {point} never fired"))?;
    p.seed = seed + 2;
    w.frame(p.seed).cite("17.3")?;
    let (recovered, _) = compose(&mut k)?;
    let (clean, rec2) = screen()?;
    ensure(rec2.serial == recovered, "20.3", format!("after the fault was disarmed, the readback names serial {}, not the next composite {recovered}", rec2.serial))?;
    screen_is(&clean, &[p], "the composite after the fault was disarmed")?;
    let _ = serial;
    Ok((r, px, rec, serial, good))
}

// gpu-display: B6/B7 uses core point 19; a skipped readback transfer must leave poison, then recover.
fn scanout_fault_skipped_transfer() -> CaseResult {
    interface()?;
    let (r, px, _, serial, _) = fault(libbreenix::gpu::abi::test_point::SKIP_TRANSFER, 15)?;
    ensure(r == -(abi::errno::EIO as i64), "20.3", format!("gpu-display: readback of composite {serial} with core point 19 {}; record v2 must refuse unwritten pixels with EIO", outcome(r)))?;
    ensure(px.iter().all(|&b| b == 0), "20.1", "a refused readback returned pixels without its record")
}

// gpu-display: B6/B7 uses WRONG_SCANOUT 26; a wrong transfer resource is not device-observable.
fn scanout_fault_wrong_resource() -> CaseResult {
    interface()?;
    let (r, _, rec, serial, good) = fault(libbreenix::gpu::abi::test_point::WRONG_SCANOUT, 18)?;
    ensure(r == -(abi::errno::EIO as i64), "20.3", format!("readback of composite {serial} with the flush bound to another resource {}; the contract gives EIO, binding", outcome(r)))?;
    ensure(rec.serial == serial && rec.resource != 0 && rec.resource != good.resource, "20.3", format!(
        "the binding failure's record names serial {} and resource {:#x}; it names composite {serial} and the resource the device is bound to, not the composed {:#x}",
        rec.serial, rec.resource, good.resource))
}

// gpu-display: B6/B7 uses core STALE_FENCE 20; another serial must never certify this composite.
fn scanout_fault_stale_fence() -> CaseResult {
    interface()?;
    let (r, _, rec, serial, good) = fault(libbreenix::gpu::abi::test_point::STALE_FENCE, 21)?;
    ensure(r == -(abi::errno::EIO as i64), "20.3", format!("readback of composite {serial} with a fence answered for another serial {}; the contract gives EIO, serial", outcome(r)))?;
    ensure(rec.serial == serial && rec.resource == good.resource, "20.3", format!(
        "the serial failure's record names serial {} and resource {:#x}; it names composite {serial} and the composed resource {:#x}, the binding being right",
        rec.serial, rec.resource, good.resource))
}

fn scanout_fault_outside_damage() -> CaseResult {
    interface()?;
    let (r, px, _, serial, _) = fault(abi::tc_point::CORRUPT_OUTSIDE_DAMAGE, 24)?;
    ensure(r == 0, "20.3", format!("readback of composite {serial} with corruption outside its damage {}", outcome(r)))?;
    crate::remember_frame(&px,1280,800,None);
    let scene = [Placed::new(320, 200, 200, 120, 0, 25)];
    let f = formula(&scene);
    let found = (0..1280u32).find(|&x| {
        let i = ((799 * 1280 + x) * 4) as usize;
        px[i..i + 4] != f(x, 799)
    });
    ensure(found.is_some(), "20.3", "with corrupt-outside-damage armed, the full destination's last row equals the formula: the planted corruption was not found")
}

fn scanout_boundary_stated() -> CaseResult {
    interface()?;
    let (_k, _w, _p, _) = one_window(26)?;
    let (_, rec) = screen()?;
    let want = boundary_of(rec.backend);
    ensure(want == Some(rec.boundary), "20.2", format!(
        "the record of backend {} ({}) states boundary {}; section 20.2 gives {want:?}", rec.backend, backend::name(rec.backend), rec.boundary))?;
    // Section 20.2: exactly DEVICE_COPY_OUTSIDE for virtio-gpu 2D, 0 for GOP, SVGA3 and VirGL.
    let flags = match rec.backend {
        backend::VIRTIO_GPU_2D => abi::OBSERVATION_DEVICE_COPY_OUTSIDE,
        _ => 0,
    };
    ensure(rec.flags == flags, "20.2", format!(
        "the record of backend {} ({}) has flags {:#x}; section 20.2 gives exactly {flags:#x}", rec.backend, backend::name(rec.backend), rec.flags))
}
