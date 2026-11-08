//! Canonical client pixels, authored on the host rather than captured from a run.
use breenix_suite::suite::{check, CaseResult};
use libuitest::{arena::{Slot, SlotRead}, capture, Session};
#[path = "../uitest_canonical.rs"]
mod scene;
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
const ROOT: &str = "/usr/share/breenix/uitest/frames/ui-testing";

fn pixels(session: &mut Session, slot: Slot) -> Result<Vec<u8>, String> {
    match session.arena_mut().read(slot)? {
        SlotRead::Final { record, bytes } if !record.oversize => Ok(bytes),
        other => Err(format!("canonical capture is incomplete: {other:?}")),
    }
}
fn launch(session: &mut Session, title: &str) -> Result<libuitest::WindowReady, String> {
    session.launch_app(&[APP, title.as_bytes(), b"--canonical"], 0)?;
    session.wait_window(title)
}
fn digest(bytes: &[u8], width: usize, height: usize) -> Result<String, String> {
    use libuitest::compare::{Frame, Format};
    let frame = Frame::new(bytes,width,height,width*4,Format::Bgrx).map_err(|e| format!("canonical frame: {e:?}"))?;
    Ok(frame.sha256().iter().map(|b| format!("{b:02x}")).collect())
}

/// The test app's canonical scene hashes to the pinned value on this target.
pub fn cross_target() -> CaseResult {
    // Pins are target data; ARM intentionally has no attested pin yet (U1).
    let target = match std::env::consts::ARCH {
        "x86_64" => "x86-metal",
        other => return Err(format!("canonical scene pin gap: target={other}; ARM pin remains unrecorded").into()),
    };
    let pin = std::fs::read_to_string(format!("{ROOT}/canonical-{target}.sha256")).map_err(|e| e.to_string())?;
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session,"canonical hash")?;
    let record = capture::window(session.arena_mut(),"canonical",ready.id)?;
    check((record.width as usize,record.height as usize) == (scene::WIDTH,scene::HEIGHT) && record.commit == ready.commit_id,
        "canonical window dimensions or commit differ")?;
    let got = digest(&pixels(&mut session,Slot::Frame)?,scene::WIDTH,scene::HEIGHT)?;
    check(got == pin.trim(), &format!("canonical target={target} got={got} want={}",pin.trim()))?;
    session.teardown()?;
    Ok(())
}

/// Captures taken with the cursor hidden contain no cursor pixels.
pub fn cursor_excluded() -> CaseResult {
    let mut session = Session::desktop()?;
    let pid = session.launch_app(&[APP,b"canonical cursor",b"--canonical"],0)?;
    let ready = session.wait_window("canonical cursor")?;
    let mut registry = vec![libbreenix::window::abi::WinInfo::default();64];
    let count = libbreenix::window::list(&mut registry).map_err(|e| e.to_string())?;
    check(count <= registry.len(),"cursor registry overflow")?;
    let info = registry[..count].iter().find(|w| w.id == ready.id).ok_or("cursor window missing")?;
    let (x,y) = (info.x,info.y);
    check(x >= 0 && y >= 0,"canonical client is off screen")?;
    capture::window(session.arena_mut(),"cursor-client",ready.id)?;
    let expected = pixels(&mut session,Slot::Frame)?;
    let before = capture::screen(session.arena_mut(),"before-motion",libbreenix::window::ObservationKind::Composed)?;
    // Home the relative device, then put its arrow over the center of the client.
    session.raw_motion(i32::MIN/2,i32::MIN/2)?;
    session.raw_motion(x+160,y+100)?;
    // The app re-commits its identical scene after receiving the center motion.
    // A composite listing that commit necessarily follows the pointer placement.
    let commit = loop {
        let line = session.read_line(pid)?;
        if let Some(id) = line.strip_prefix("UITEST-CURSOR x=160 y=100 commit=") {
            break id.parse::<u64>().map_err(|e| e.to_string())?;
        }
    };
    check(commit > ready.commit_id,"cursor witness did not commit after motion")?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let record = loop {
        let record = capture::screen(session.arena_mut(),"cursor-screen",libbreenix::window::ObservationKind::Composed)?;
        if record.composite > before.composite && capture::screen_windows(&record)?.contains(&(ready.id,commit)) { break record; }
        check(std::time::Instant::now() < end,"cursor motion did not reach a later composite")?;
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    };
    check(capture::screen_windows(&record)?.contains(&(ready.id,commit)),"cursor screen omits client commit")?;
    let screen = pixels(&mut session,Slot::Screen)?;
    check(x as usize+scene::WIDTH <= record.width as usize && y as usize+scene::HEIGHT <= record.height as usize,"cursor client clipped")?;
    for row in 0..scene::HEIGHT {
        let at = ((y as usize+row)*record.width as usize+x as usize)*4;
        // X is padding in scanout; compare the canonical colors.
        for col in 0..scene::WIDTH {
            check(screen[at+col*4..at+col*4+3] == expected[(row*scene::WIDTH+col)*4..(row*scene::WIDTH+col)*4+3],
                &format!("hidden cursor changed client pixel {col},{row}"))?;
        }
    }
    session.teardown()?;
    Ok(())
}
