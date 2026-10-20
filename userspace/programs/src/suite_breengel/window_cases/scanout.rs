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
    pictured!("present-flip", "Every flip records its actual target and completed present fence", scanout_present_flip),
    pictured!("hidpi-mode", "The complete 2560x1600 scanout is read back and changes in its final row are observed", scanout_hidpi_mode),
];

// ---------------------------------------------------------------------------------------
// scanout: display observation on every target (contract section 20)
// ---------------------------------------------------------------------------------------

use super::composition::{compose, compositor, formula, place, screen, screen_is, show, Placed};
use super::kit::*;
use abi::{backend, op, Plain};

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

// gpu-display: §2.2/R4 replaces fb_info with MODES and full physical-size GPU readback.
fn scanout_mode_set() -> CaseResult {
let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("18")?;
    let mode=modes.modes[modes.current as usize];
    ensure((mode.width,mode.height,mode.scale)==(2560,1600,2),"18,20.2","gpu-display: suite mode is not 2560x1600 at scale 2")?;
    interface()?;let (mut k,_w,_p,_)=one_window(1)?;
    let s=super::composition::gpu_snapshot(&mut k)?;
    ensure((s.width,s.height)==(2560,1600) && s.word(56)==2560*4,"20.1","gpu-display: record/readback dimensions or stride disagree with MODES")
}

// gpu-display: §2.2 uses record version 2 and ruling 47 native backend-specific completion identities.
fn scanout_device_path() -> CaseResult {
interface()?;let (mut k,w,_p,serial)=one_window(2)?;
    let s=super::composition::gpu_snapshot(&mut k)?;
    let (ret,caps)=libbreenix::gpu::caps();let backend=s.word(16);
    ensure(ret==0 && [4,5].contains(&backend) && backend==caps.backend,"20.2","gpu-display: record backend differs from admitted GPU backend")?;
    ensure(listed(w.id()).cite("17.1")?.ok_or("window not listed")?.backend==backend,"20.2","gpu-display: window backend differs from readback backend")?;
    let (status,render,readback)=if backend==4 {(0x1100,0x207,0x206)}else{(1,1152,1104)};
    ensure(s.word(112)==status && s.word(116)==render && s.word(128)==status && s.word(132)==readback,"20.2","gpu-display: record does not contain native render and readback completions")?;
    ensure(s.quad(104)!=0 && s.quad(120)!=0 && s.quad(184)!=0 && s.word(192)==status && s.quad(176)==serial,
        "20.2","gpu-display: record lacks this composite's render/readback/present fences")?;
    if backend==4 {ensure(s.quad(136)==0 && s.quad(144)==0 && s.word(152)==0 && s.word(156)==0,"20.2","gpu-display: VirGL record invented VMware copy fields")?;}
    else {ensure(s.quad(136)!=0 && s.quad(136)!=s.quad(80) && s.quad(144)!=0 && s.word(152)==1042 && s.word(156)==1,"20.2","gpu-display: VMware record lacks its dedicated readback surface/MOB and completed copy")?;}
    ensure(s.word(164)==0 && s.word(20)==4,"20.2","gpu-display: readback has unwritten bytes or the wrong boundary")
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

// gpu-display: B7 replaces the legacy fixed-resource present assumption with each GPU present's actual binding identity.
fn scanout_binding() -> CaseResult {
interface()?;let (mut k,w,mut p,first)=one_window(3)?;
    let mut serial=first;
    for n in 0..2 {
        if n==1 {w.frame(4).cite("17.3")?;p.seed=4;serial=compose(&mut k)?.0;}
        let s=super::composition::gpu_snapshot(&mut k)?;
        let (ret,m)=libbreenix::gpu::modes();
        ensure(ret==0 && m.binding_known==1 && s.quad(80)!=0 && s.quad(80)==m.scanout_resource,"20.1",
            "gpu-display: readback resource is not the device's confirmed scanout binding")?;
        ensure(s.quad(176)==serial && s.word(24)==3 && s.quad(184)!=0,"20.1",
            "gpu-display: binding record belongs to a different composite or incomplete present")?;
    }
    let s=super::composition::gpu_snapshot(&mut k)?;
    same(&s.pixels,s.width,s.height,true,"bound GPU composition","20.1",formula(&[p]))
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

// gpu-display: B6/B7 uses core point 19; a skipped readback transfer must leave poison, then recover.
fn scanout_fault_skipped_transfer() -> CaseResult {
    let (status,reason,s,_)=gpu_fault(libbreenix::gpu::abi::test_point::SKIP_TRANSFER,15)?;
    ensure(status== -5 && reason==1,"20.3","gpu-display: skipped transfer must be EIO, unwritten (1)")?;
    ensure(s.pixels.iter().chain(s.record.iter()).all(|&b|b==0xA5),"20.3","gpu-display: failed readback published partial pixels or a record")
}

fn scanout_fault_wrong_resource() -> CaseResult {
    let (status,reason,s,serial)=gpu_fault(libbreenix::gpu::abi::test_point::WRONG_SCANOUT,18)?;
    ensure(status==0 && reason==0,"20.3","gpu-display: wrong-scanout must report the actual completed binding, not invent a device error")?;
    ensure(s.quad(80)!=0 && s.quad(176)!=serial && s.word(164)==0,"20.3","gpu-display: wrong-scanout was not exposed by the observed resource/composite identity")?;
    let scene=[Placed::new(320,200,200,120,0,19)];let want=formula(&scene);
    ensure((0..s.height).any(|y|(0..s.width).any(|x|s.pixels[(y*s.width+x)*4..(y*s.width+x+1)*4]!=want(x as u32,y as u32))),"20.3",
        "gpu-display: wrong-scanout did not expose a destination different from the intended composite")
}

// gpu-display: B6/B7 core point 20 must report the observed fence mismatch; clean recovery remains mandatory.
fn scanout_fault_stale_fence() -> CaseResult {
let (status,reason,s,_)=gpu_fault(libbreenix::gpu::abi::test_point::STALE_FENCE,21)?;
    ensure(status== -5 && reason==2,"20.3","gpu-display: stale fence must be EIO, fence-mismatch (2)")?;
    ensure(s.pixels.iter().chain(s.record.iter()).all(|&b|b==0xA5),"20.3","gpu-display: stale fence published partial pixels or record")
}

// gpu-display: B6/B7 uses core CORRUPT_PIXEL 22; compare the complete destination, outside damage too.
// gpu-display: B6/B7 core point 22 corrupts device pixel (0,0); every destination pixel is checked, outside damage included.
fn scanout_fault_outside_damage() -> CaseResult {
let (status,reason,s,_)=gpu_fault(libbreenix::gpu::abi::test_point::CORRUPT_PIXEL,24)?;
    ensure(status==0 && reason==0,"20.3","gpu-display: corrupt-pixel readback did not complete normally")?;
    let scene=[Placed::new(320,200,200,120,0,25)];let want=formula(&scene);
    let mut outside=0usize;
    for y in 0..s.height {for x in 0..s.width {
        let different=s.pixels[(y*s.width+x)*4..(y*s.width+x+1)*4]!=want(x as u32,y as u32);
        if different {
            ensure(!(320..520).contains(&x) || !(200..320).contains(&y),"20.3","gpu-display: corruption was inside the declared damage")?;
            outside+=1;
        }
    }}
    ensure(outside>0,"20.3","gpu-display: full-destination comparison did not detect planted corruption outside damage")
}

// gpu-display: §2.2 record v2 uses boundary 4 on both GPU backends; no legacy device-copy boundary remains.
fn scanout_boundary_stated() -> CaseResult {
interface()?;let (mut k,_w,_p,_)=one_window(26)?;
    let s=super::composition::gpu_snapshot(&mut k)?;
    ensure([4,5].contains(&s.word(16)) && s.word(20)==4,"20.2","gpu-display: record must state that the host renderer and host window lie outside guest observation")?;
    ensure(s.word(24)==3,"20.2","gpu-display: composed scanout must set exactly the scanout and composite bits")
}
fn gpu_fault(point:u32,seed:u32)->Result<(i64,u32,super::composition::GpuSnapshot,u64),libbreenix::suite::CaseError>{
    let (mut k,w,mut p,_)=one_window(seed)?;
    let fired=libbreenix::test_control::fired(point).cite("21")?;
    p.seed=seed+1;w.frame(p.seed).cite("17.3")?;
    libbreenix::test_control::arm(point,0,0).cite("21")?;
    let (fault_serial,_)=compose(&mut k)?;
    let result=super::composition::gpu_observe(&mut k);
    libbreenix::test_control::disarm_all().cite("21")?;
    ensure(libbreenix::test_control::fired(point).cite("21")?==fired+1,"21",format!("gpu-display: core fault point {point} did not fire exactly once"))?;
    let observed=result?;
    p.seed=seed+2;w.frame(p.seed).cite("17.3")?;let (serial,_)=compose(&mut k)?;
    let clean=super::composition::gpu_snapshot(&mut k)?;
    ensure(clean.quad(176)==serial,"20.3","gpu-display: recovery record names another composite")?;
    same(&clean.pixels,clean.width,clean.height,true,"GPU fault recovery","20.3",formula(&[p]))?;
    Ok((observed.0,observed.1,observed.2,fault_serial))
}

// gpu-display: §2.2 every completed flip records the actual new scanout resource and that present's fence.
fn scanout_present_flip()->CaseResult {
    interface()?;let (mut k,w,mut p,first)=one_window(40)?;let mut serial=first;
    let mut previous=(0,0);
    for n in 0..3 {
        if n!=0 {p.seed=40+n;w.frame(p.seed).cite("17.3")?;serial=compose(&mut k)?.0;}
        let s=super::composition::gpu_snapshot(&mut k)?;let resource=s.quad(80);let fence=s.quad(184);
        let (ret,m)=libbreenix::gpu::modes();
        ensure(ret==0 && m.binding_known==1 && m.scanout_resource==resource && resource!=0 && fence!=0 && s.quad(176)==serial,
            "20","gpu-display: flipped target, binding, composite serial or completed present fence disagree")?;
        ensure(n==0 || (resource!=previous.0 && fence!=previous.1),"20","gpu-display: successive frames did not flip to a different completed target")?;
        same(&s.pixels,s.width,s.height,true,"flipped scanout","20",formula(&[p]))?;previous=(resource,fence);
    }Ok(())
}

// gpu-display: R4 reads every pixel of 2560x1600, including a changed final row, with physical dimensions in record v2.
fn scanout_hidpi_mode()->CaseResult {
    interface()?;let mut k=compositor()?;let mut p=Placed::new(0,1599,2560,1,0,50);
    let w=W::new(2560,1,2)?;show(&mut k,&w,&p)?;compose(&mut k)?;
    for n in 0..2 {
        if n==1 {p.seed=51;w.frame(51).cite("17.3")?;compose(&mut k)?;}
        let s=super::composition::gpu_snapshot(&mut k)?;
        ensure((s.width,s.height)==(2560,1600) && (s.word(40),s.word(44),s.word(48),s.word(52))==(2560,1600,2560,1600),"20",
            "gpu-display: HiDPI readback or record omitted part of the physical destination")?;
        same(&s.pixels,2560,1600,true,"full HiDPI scanout including its last row","20",formula(&[p]))?;
    }Ok(())
}
