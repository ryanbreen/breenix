//! Timer checks observe captured app state and kernel commit metadata.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{test_control, time, ui_clock::Stamp, window::{self, abi}};
use libuitest::{Session, WindowReady};
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";

fn launch(session: &mut Session, title: &str, flags: &[&[u8]]) -> Result<WindowReady, String> {
    let mut args = vec![APP, title.as_bytes()];
    args.extend_from_slice(flags);
    session.launch_app(&args, 0)?;
    session.wait_window(title)
}
fn state(id: u64, stamp: Stamp, ticks: u64, fires: u64, missed: u64) -> Result<(u64, u64), breenix_suite::suite::CaseError> {
    let mut pixels = vec![0; 320*200*4];
    let mut tree = vec![0; abi::limits::DEFAULT_TREE_KIB as usize * 1024];
    let captured = window::capture(id, &mut pixels, Some(&mut tree)).map_err(|e| e.to_string())?;
    let end = tree.iter().position(|&b| b == 0).ok_or("timer tree lacks terminator")?;
    let tree: serde_json::Value = serde_json::from_slice(&tree[..end]).map_err(|e| e.to_string())?;
    let value = &tree["value"];
    let mut position = 0_i64;
    let mut velocity = 0_i64;
    for _ in 0..ticks {
        velocity += (1000-position)/16;
        position += velocity;
        velocity = velocity*31/32;
    }
    check(value["ticks"] == ticks && value["fires"] == fires && value["missed"] == missed
        && value["position"] == position && value["velocity"] == velocity,
        &format!("timer state window={id} got={value} ticks={ticks} fires={fires} missed={missed} position={position} velocity={velocity}"))?;
    let want = [0x60, (fires % 251) as u8, (ticks % 251) as u8];
    check(pixels.chunks_exact(4).all(|pixel| pixel[..3] == want), "animation pixels differ from independent timer state")?;
    let (commit, meta) = window::read_meta(id).map_err(|e| e.to_string())?;
    check(commit == captured.commit_id && meta.epoch == stamp.epoch && meta.ui_time_ms == stamp.now_ns/1_000_000,
        "timer callback was not published before step acknowledged")?;
    let mut windows = vec![abi::WinInfo::default(); 64];
    let count = window::list(&mut windows).map_err(|e| e.to_string())?;
    let info = windows[..count.min(windows.len())].iter().find(|w| w.id == id).ok_or("timer window missing from registry")?;
    check(info.commit_id == commit, "timer registry changed after capture")?;
    Ok((commit, info.pixel_gen))
}

pub fn timers_virtual() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session, "virtual timer", &[b"--timer"])?;
    let initial = state(ready.id, Stamp::default(), 0, 0, 0)?;
    time::sleep_ms(1100).map_err(|e| e.to_string())?;
    check(state(ready.id, Stamp::default(), 0, 0, 0)? == initial, "real time fired the UI timer")?;
    let stamp = session.step(1000)?;
    state(ready.id, stamp, 0, 1, 0)?;
    let stamp = session.step(10_000)?;
    state(ready.id, stamp, 0, 2, 9)?;
    session.teardown()?;
    Ok(())
}

pub fn continuous_animation() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session, "continuous animation", &[b"--animate"])?;
    let mut previous = state(ready.id, Stamp::default(), 0, 0, 0)?;
    // Each step must finish while the animation remains registered and active.
    for epoch in 1..=4 {
        let stamp = session.step(16)?;
        check(stamp.epoch == epoch, "animation skipped an epoch")?;
        let next = state(ready.id, stamp, epoch*16, 0, 0)?;
        check(next.0 == previous.0+1 && next.1 == previous.1+1, "animation must draw one frame per epoch")?;
        previous = next;
    }
    let zero = session.step(0)?;
    let idle = state(ready.id, zero, 64, 0, 0)?;
    check(idle.0 == previous.0+1 && idle.1 == previous.1, "zero step must acknowledge without new pixels")?;
    let stamp = session.step(1000)?;
    state(ready.id, stamp, 1064, 1, 0)?;
    session.teardown()?;
    Ok(())
}

pub fn real_clock_counter() -> CaseResult {
    let mut session = Session::kiosk()?;
    let clean = launch(&mut session, "clean timer clock", &[b"--animate"])?;
    let planted = launch(&mut session, "deliberate real clock", &[b"--animate", b"--real-clock"])?;
    for id in [clean.id, planted.id] {
        check(window::read_meta(id).map_err(|e| e.to_string())?.1.real_clock_reads == 0, "real read before deliberate callback")?;
    }
    let stamp = session.step(16)?;
    for (id, reads) in [(clean.id, 0), (planted.id, 1)] {
        state(id, stamp, 16, 0, 0)?;
        let meta = window::read_meta(id).map_err(|e| e.to_string())?.1;
        check(meta.real_clock_reads == reads, &format!("real-clock-counter window={id} reads={} expected={reads}", meta.real_clock_reads))?;
    }
    session.teardown()?;
    Ok(())
}

fn real_ns() -> Result<u64, String> {
    time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string())
}
struct ReleaseStall;
impl Drop for ReleaseStall {
    fn drop(&mut self) { let _ = test_control::arm(abi::tc_point::RELEASE_STALL, 0, 0); }
}

pub fn backpressure_stepped() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session, "backpressure animation", &[b"--animate"])?;
    let id = ready.id;
    let before = test_control::fired(abi::tc_point::CAPTURE_STALL).map_err(|e| e.to_string())?;
    test_control::arm(abi::tc_point::CAPTURE_STALL, id, 0).map_err(|e| e.to_string())?;
    let release = ReleaseStall;
    let capture = std::thread::spawn(move || {
        let mut pixels = vec![0; 320*200*4];
        let mut tree = vec![0; abi::limits::DEFAULT_TREE_KIB as usize * 1024];
        window::capture(id, &mut pixels, Some(&mut tree)).map_err(|e| e.to_string())
    });
    let result = (|| -> CaseResult {
        let deadline = real_ns()? + 1_000_000_000;
        while test_control::fired(abi::tc_point::CAPTURE_STALL).map_err(|e| e.to_string())? == before {
            check(real_ns()? < deadline, "capture-stall never pinned the front buffer")?;
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
        let first = session.step(16)?; // B replaces pinned A as front.
        state(id, first, 16, 0, 0)?;
        let marker = std::path::Path::new(session.config_root()).join("timer-backpressure");
        let releaser = std::thread::spawn(move || -> Result<(), String> {
            let result = (|| {
                let deadline = real_ns()? + 1_500_000_000;
                loop {
                    match std::fs::read_to_string(&marker) {
                        Ok(epoch) => {
                            if epoch != "2" { return Err(format!("unexpected backpressure epoch {epoch}")); }
                            let meta = window::read_meta(id).map_err(|e| e.to_string())?.1;
                            if meta.epoch != 1 { return Err("blocked timer acknowledged before acquiring a buffer".into()); }
                            return Ok(());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.to_string()),
                    }
                    if real_ns()? >= deadline { return Err("animation never witnessed buffer backpressure".into()); }
                    time::sleep_ms(1).map_err(|e| e.to_string())?;
                }
            })();
            drop(release);
            result
        });
        let second = session.step(16);
        releaser.join().map_err(|_| "stall releaser panicked")??;
        state(id, second?, 32, 0, 0)?;
        let recovery = session.step(16)?;
        state(id, recovery, 48, 0, 0)?;
        Ok(())
    })();
    // The guard releases the capture on both early errors and the witnessed path.
    let captured = capture.join().map_err(|_| "stalled capture panicked")?;
    result?;
    check(captured.map_err(|e| e.to_string())?.commit_id == ready.commit_id, "stalled capture did not retain the original front")?;
    session.teardown()?;
    Ok(())
}
