//! Clock stepping cases over live BreenGL windows and kernel commit metadata.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{signal, time, ui_clock::Stamp, window};
use libuitest::{Session, WindowReady};
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
fn real_now() -> Result<u64, String> {
    time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string())
}
fn launch(session: &mut Session, title: &str) -> Result<(u64, WindowReady), String> {
    let pid = session.launch_app(&[APP, title.as_bytes(), b"--clock-probe"], 0)?;
    let ready = session.wait_window(title)?;
    observed(session, pid, ready.id, Stamp::default())?;
    Ok((pid, ready))
}
fn observed(session: &mut Session, pid: u64, window: u64, stamp: Stamp) -> Result<(), String> {
    let expected = format!("UITEST-CLOCK window={window} epoch={} now_ns={} real_reads=0", stamp.epoch, stamp.now_ns);
    let deadline = real_now()? + 2_000_000_000;
    loop {
        if real_now()? >= deadline { return Err(format!("clock-observation-timeout window={window} expected={stamp:?}")); }
        let line = session.read_line(pid)?;
        if line == expected { return Ok(()); }
        if line.starts_with("UITEST-CLOCK ") && !line.contains("epoch=0 ") {
            return Err(format!("clock observation differs: {line:?}, expected {expected:?}"));
        }
    }
}
fn metadata(id: u64, stamp: Stamp) -> CaseResult {
    let (_, meta) = window::read_meta(id).map_err(|e| e.to_string())?;
    check(meta.epoch == stamp.epoch && meta.ui_time_ms == stamp.now_ns / 1_000_000
        && meta.real_clock_reads == 0, &format!("window={id} metadata epoch={} time_ms={} reads={} expected={stamp:?}",
        meta.epoch, meta.ui_time_ms, meta.real_clock_reads))
}
pub fn frozen() -> CaseResult {
    let mut session = Session::kiosk()?;
    let (pid, ready) = launch(&mut session, "clock frozen")?;
    let before = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    time::sleep_ms(30).map_err(|e| e.to_string())?;
    // The app's wait loop reports again after its real input wait, while UI
    // time remains frozen. This is app-observed time rather than a driver read.
    observed(&mut session, pid, ready.id, Stamp::default())?;
    let after = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    check(before.0 == after.0 && after.1.epoch == 0 && after.1.ui_time_ms == 0,
        "frozen clock produced a new commit or advanced time")?;
    session.teardown()?;
    Ok(())
}
pub fn step() -> CaseResult {
    let mut session = Session::kiosk()?;
    let (pid, ready) = launch(&mut session, "clock step")?;
    let before = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    let stamp = session.step(16)?;
    check(stamp == Stamp { epoch: 1, now_ns: 16_000_000 }, "step(16) stamp differs")?;
    observed(&mut session, pid, ready.id, stamp)?;
    metadata(ready.id, stamp)?;
    let after = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    check(after.0 > before.0, "idle step lacks a no-change commit")?;
    // A step(0) still needs a new acknowledgment, and does not advance UI time.
    let zero = session.step(0)?;
    check(zero == Stamp { epoch: 2, now_ns: 16_000_000 }, "zero step stamp differs")?;
    metadata(ready.id, zero)?;
    session.teardown()?;
    Ok(())
}
pub fn shared() -> CaseResult {
    let mut session = Session::kiosk()?;
    let (a, wa) = launch(&mut session, "shared clock A")?;
    let (b, wb) = launch(&mut session, "shared clock B")?;
    let stamp = session.step(16)?;
    for (pid, ready) in [(a, wa), (b, wb)] {
        observed(&mut session, pid, ready.id, stamp)?;
        metadata(ready.id, stamp)?;
    }
    session.teardown()?;
    Ok(())
}
pub fn stopped() -> CaseResult {
    let mut session = Session::kiosk()?;
    let (pid, ready) = launch(&mut session, "stopped clock participant")?;
    signal::kill(pid as i32, signal::SIGSTOP).map_err(|e| e.to_string())?;
    // Witness the stop before starting the epoch; kill returning alone does not
    // establish that the app cannot have acknowledged it.
    let deadline = real_now()? + 2_000_000_000;
    loop {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
        let state = stat.rsplit_once(") ").and_then(|(_, tail)| tail.split_whitespace().next());
        if state == Some("T") { break; }
        check(real_now()? < deadline, "clock participant never entered stopped state")?;
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    let stale = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    let began = real_now()?;
    let result = session.step(16);
    let elapsed = real_now()? - began;
    check(matches!(&result, Err(message) if message.contains("clock-step-timeout") && message.contains(&format!("window={}",ready.id))),
        &format!("wrong stopped outcome: {result:?}"))?;
    check(elapsed >= 2_000_000_000, "stopped step failed before its real bound")?;
    println!("CLOCK-STOP window={} elapsed_ns={elapsed} result={result:?}", ready.id);
    session.exclude(ready.id);
    let next = session.step(16)?;
    check(next == Stamp { epoch: 2, now_ns: 32_000_000 }, "excluded step did not advance")?;
    let after = window::read_meta(ready.id).map_err(|e| e.to_string())?;
    check(after.0 == stale.0 && after.1.epoch == stale.1.epoch, "excluded stopped window acknowledged")?;
    signal::kill(pid as i32, signal::SIGCONT).map_err(|e| e.to_string())?;
    session.teardown()?;
    Ok(())
}
