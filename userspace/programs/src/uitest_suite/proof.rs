//! Planted faults are accepted only after the same scenario passes cleanly.
use breenix_suite::suite::{check, CaseResult};
use libuitest::{arena::{Slot, SlotRead}, capture, classify, compare::{self, Format, Frame, Options, Rect},
    input::LivePointer, Classification, Expected, FailureClass, ScenarioOutcome, ScenarioSpec, Session};

const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
const WRONG_OPERATION: &str = "expect_pixels(submit)";

fn pixels(session: &mut Session, id: u64) -> Result<(u32, u32, Vec<u8>), String> {
    let record = capture::window(session.arena_mut(), "submit", id)?;
    match session.arena_mut().read(Slot::Frame)? {
        SlotRead::Final { bytes, .. } => Ok((record.width, record.height, bytes)),
        other => Err(format!("incomplete fault capture: {other:?}")),
    }
}
fn submit_color(session: &mut Session, id: u64, scale: usize) -> Result<ScenarioOutcome, String> {
    let (width, height, got) = pixels(session, id)?;
    // A literal flat patch inside the button, away from its border and text.
    let want = [60, 60, 60, 255].repeat(width as usize * height as usize);
    let regions = [Rect { x: 282 * scale, y: 44 * scale, w: 4 * scale, h: 4 * scale }];
    let verdict = compare::compare(
        Frame::new(&want, width as usize, height as usize, width as usize * 4, Format::Bgrx).map_err(|e|format!("reference: {e:?}"))?,
        Frame::new(&got, width as usize, height as usize, width as usize * 4, Format::Bgrx).map_err(|e|format!("capture: {e:?}"))?,
        Options { regions: &regions, ..Options::default() }).map_err(|e|format!("comparison: {e:?}"))?;
    Ok(match verdict.comparison.first {
        None => ScenarioOutcome::pass(),
        Some(first) => ScenarioOutcome::fail(FailureClass::PixelMismatch, WRONG_OPERATION,
            &libuitest::messages::pixel_details(first.x, first.y, first.actual, first.expected)),
    })
}
fn wrong_color_scenario(fault: bool) -> Result<ScenarioOutcome, String> {
    let mut session = Session::desktop()?;
    let mut argv = vec![APP, b"planted color".as_slice(), b"--widgets".as_slice()];
    if fault { argv.push(b"--wrong-color"); }
    let pid = session.launch_app(&argv, 0)?;
    let result = (|| {
        let id = session.wait_window("planted color")?.id;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        let scale = libbreenix::gpu::current_mode().map_err(|e|e.to_string())?.scale as usize;
        submit_color(live.session(), id, scale)
    })();
    let exits = session.teardown()?;
    for exit in exits.iter().filter(|e| e.pid == pid) {
        if !libbreenix::process::wifsignaled(exit.status)
            || libbreenix::process::wtermsig(exit.status) != libbreenix::signal::SIGKILL {
            return Ok(ScenarioOutcome::fail(FailureClass::AppCrashed, "widget-app", &format!("status={}",exit.status)));
        }
    }
    result
}

pub fn fault_wrong_color() -> CaseResult {
    let scale = libbreenix::gpu::current_mode()?.scale as usize;
    for fault in [false, true] {
        let outcome = libuitest::run_scenario(ScenarioSpec { suite: "ui-testing", case: "proof/fault-wrong-color",
            checks: &[], limit_ms: 6500, run: &mut |_| wrong_color_scenario(fault)
                .unwrap_or_else(|e| ScenarioOutcome::fail(FailureClass::Infrastructure,"wrong-color-scenario",&e)) });
        let evidence = format!("first={},{} got=#d02040 want=#3c3c3c",282*scale,44*scale);
        let expected = if fault { Expected::Fault { class: FailureClass::PixelMismatch, operation: WRONG_OPERATION, evidence: &evidence } }
            else { Expected::Pass };
        check(classify(&expected,&outcome) == if fault {Classification::ExpectedFault} else {Classification::CleanPass},
            &format!("wrong-color fault={fault}: {outcome:?}"))?;
    }
    Ok(())
}
