//! Live capture checks; device pixels and tree must describe the returned commit.
use breenix_suite::suite::{check, CaseResult};
use libuitest::{arena::{Slot, SlotRead}, capture, capture_marker::{self, Generation}, Session};
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";

fn final_slot(session: &mut Session, slot: Slot) -> Result<(libuitest::arena::Record, Vec<u8>), String> {
    match session.arena_mut().read(slot)? {
        SlotRead::Final { record, bytes } if !record.oversize => Ok((record, bytes)),
        other => Err(format!("capture slot is not complete: {other:?}")),
    }
}
fn launch(session: &mut Session, title: &str) -> Result<libuitest::WindowReady, String> {
    session.launch_app(&[APP, title.as_bytes()], 0)?;
    session.wait_window(title)
}

/// A window capture equals the committed buffer of that commit.
pub fn window_capture() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session, "capture window")?;
    let record = capture::window(session.arena_mut(), "window", ready.id)?;
    let (_, pixels) = final_slot(&mut session, Slot::Frame)?;
    let (tree_record, tree) = final_slot(&mut session, Slot::Tree)?;
    let marker = capture::root_marker(&tree)?;
    check(capture_marker::whole(&pixels, record.width as usize * 4, record.height as usize, &marker, record.commit), "window capture is torn or tree mismatched")?;
    check(record.commit == ready.commit_id && tree_record.commit == record.commit && tree_record.window == ready.id, "capture identity differs from committed window")?;
    let generation = capture_marker::decode(&marker).ok_or("invalid root marker")?;
    check(generation.producer != 0, "producer thread is absent")?;
    let expected = capture_marker::frame(record.width as usize, record.height as usize, generation)?;
    check(pixels == expected, "window capture differs from the committed pattern")?;
    session.teardown()?;
    Ok(())
}

/// A screen capture shows the window at its position, with its observation record.
pub fn screen_capture() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = launch(&mut session, "capture screen")?;
    let record = capture::screen(session.arena_mut(), "screen", libbreenix::window::ObservationKind::Composed)?;
    let windows = capture::screen_windows(&record)?;
    check(windows.contains(&(ready.id, ready.commit_id)), "screen observation omits the committed window")?;
    let (_, pixels) = final_slot(&mut session, Slot::Screen)?;
    // The session kiosk places its windows at (0,0), without decoration.
    let marker = capture_marker::decode(&pixels[..capture_marker::MARKER_BYTES]).ok_or("screen lacks row marker at window position")?;
    check(marker.commit == ready.commit_id, "screen marker differs from observation commit")?;
    let expected = capture_marker::frame(320, 200, marker)?;
    for y in 0..200 {
        let at = y * record.width as usize * 4;
        check(pixels[at..at + 320*4] == expected[y*320*4..(y+1)*320*4], &format!("screen differs at window row {y}"))?;
    }
    session.teardown()?;
    Ok(())
}

/// A screen capture lists the commit of each window it contains.
pub fn capture_commits() -> CaseResult {
    let mut session = Session::desktop()?;
    let a = launch(&mut session, "capture commits A")?;
    let b = launch(&mut session, "capture commits B")?;
    let record = capture::screen(session.arena_mut(), "commits", libbreenix::window::ObservationKind::Composed)?;
    let windows = capture::screen_windows(&record)?;
    for ready in [a, b] {
        check(windows.iter().filter(|&&(id, _)| id == ready.id).count() == 1, "screen must list each app once")?;
        check(windows.contains(&(ready.id, ready.commit_id)), "screen lists a different commit")?;
    }
    session.teardown()?;
    Ok(())
}

/// Judge a real comparison; the planted pixel fault must survive inner transport.
pub fn inner_expected_failure() -> CaseResult {
    use libuitest::{classify, run_scenario, Classification, Expected, FailureClass, ScenarioOutcome, ScenarioSpec};
    for planted in [false, true] {
        let mut body = |_: &mut libuitest::RecordClient<'_>| {
            let attempt = (|| -> Result<ScenarioOutcome, String> {
                let generation = Generation { commit: 1, producer: 1 };
                let want = capture_marker::frame(16, 4, generation)?;
                let mut got = want.clone();
                if planted { got[(2*16+12)*4] ^= 1; }
                let dir = std::env::temp_dir().join(format!("uitest-inner-capture-{}", libbreenix::process::getpid().map_err(|e| e.to_string())?.raw()));
                let mut arena = libuitest::arena::Arena::create(&dir, libuitest::arena::Layout::for_mode(16,4)?)?;
                let result = (|| {
                    capture::pixels(&mut arena, "marker", 16, 4, &got)?;
                    let SlotRead::Final { bytes, .. } = arena.read(Slot::Frame)? else { return Err("surface capture incomplete".into()); };
                    capture::compare_outcome("marker", &want, &bytes, 16, 4)
                })();
                drop(arena);
                let _ = std::fs::remove_file(dir);
                result
            })();
            attempt.unwrap_or_else(|e| ScenarioOutcome::fail(FailureClass::Infrastructure, "capture", &e))
        };
        let outcome = run_scenario(ScenarioSpec { suite: "inner", case: "scenario/pixel", checks: &[], limit_ms: 1000, run: &mut body });
        let expected = if planted { Expected::Fault { class: FailureClass::PixelMismatch, operation: "marker", evidence: "first=12,2 got=#182031 want=#182030" } } else { Expected::Pass };
        check(classify(&expected, &outcome) == if planted { Classification::ExpectedFault } else { Classification::CleanPass }, &format!("pixel scenario: {outcome:?}"))?;
    }
    Ok(())
}
