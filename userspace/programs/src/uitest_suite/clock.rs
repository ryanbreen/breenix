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

fn compositor_status() -> Result<window::abi::CompositorStatus, String> {
    let mut status = window::abi::CompositorStatus::default();
    window::call(window::abi::op::COMPOSITOR_STATUS, &mut status).map_err(|e| e.to_string())?;
    Ok(status)
}
fn compositor_epoch(stamp: Stamp) -> Result<window::abi::CompositorStatus, String> {
    let status = compositor_status()?;
    let none = status.flags & window::abi::compositor_ack_flags::NONE_NEEDED != 0;
    if status.epoch != stamp.epoch || none == (status.presented != 0) {
        return Err(format!("compositor epoch acknowledgment differs: {status:?} expected={stamp:?}"));
    }
    Ok(status)
}

pub fn epochs_acked() -> CaseResult {
    let mut session = Session::desktop()?;
    let (a, wa) = launch(&mut session, "desktop clock A")?;
    let (b, wb) = launch(&mut session, "desktop clock B")?;
    let page = libbreenix::ui_clock::ClockPage::open(&std::path::Path::new(session.config_root()).join("ui-clock"))?;
    check(page.participants().iter().any(|p| p.id == libbreenix::ui_clock::COMPOSITOR_ID
        && p.flags & libbreenix::ui_clock::COMPOSITOR != 0), "bwm is absent from required clock participants")?;
    let stamp = session.step(16)?;
    for (pid, ready) in [(a, wa), (b, wb)] {
        observed(&mut session, pid, ready.id, stamp)?;
        metadata(ready.id, stamp)?;
    }
    let before = compositor_epoch(stamp)?;
    let idle = session.step(0)?;
    let after = compositor_epoch(idle)?;
    check(after.claim_epoch == before.claim_epoch && after.composites == before.composites
        && after.presented == 0 && after.flags == window::abi::compositor_ack_flags::NONE_NEEDED,
        &format!("idle epoch composited or lacks none-needed: before={before:?} after={after:?}"))?;
    for id in [wa.id, wb.id] { metadata(id, idle)?; }
    session.teardown()?;
    Ok(())
}

/// Compare only the taskbar clock's region with literal expected text, using
/// the certified font renderer. This is not a whole-desktop chrome golden.
fn taskbar(expected: &[u8; 11]) -> Result<(), String> {
    use libgfx::{bitmap_font, color::Color, surface::{Format, Surface}, ttf_font::{self, CachedFont, Font, TextRun}};
    let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
    let large = bitmap_font::metrics_at(bitmap_font::RasterHeight::Size32);
    let scale = if mode.width as usize / large.char_width >= 128
        && mode.height as usize / large.line_height() >= 36 { 2 } else { 1 };
    let metrics = bitmap_font::metrics_at(if scale == 2 { bitmap_font::RasterHeight::Size32 }
        else { bitmap_font::RasterHeight::Size16 });
    let width = mode.width as usize;
    let height = 28 * scale;
    let mut reference = Surface::new(width, height, Format::BGRX).map_err(|e| e.to_string())?;
    reference.clear(Color::rgb(20, 22, 30));
    let mut font = CachedFont::new(Font::parse(&std::fs::read("/usr/share/fonts/DejaVuSans.ttf").map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?, 256);
    let x = width - expected.len() * metrics.char_width - 8 * scale;
    let y = (height - metrics.line_height()) / 2 + scale;
    let clip = reference.clip();
    ttf_font::draw_runs_subpixel(&mut reference, &mut font, &[TextRun {
        bytes: expected, x: x as i32, y: y as i32, size: 14.0 * scale as f32,
        fg: Color::rgb(180, 185, 195), clip,
    }]);
    let left = width.saturating_sub(320 * scale);
    let stride = (width - left) * 4;
    let mut want = vec![0; stride * height];
    reference.read_rect([left, 0, width-left, height], &mut want, stride).map_err(|e| e.to_string())?;
    let mut got = vec![0; width * mode.height as usize * 4];
    let mut record = [0; 1024];
    let mut readback = window::abi::Readback { dst: got.as_mut_ptr() as u64, dst_len: got.len() as u64,
        record: record.as_mut_ptr() as u64, record_len: 1024, ..Default::default() };
    window::call(window::abi::op::SCANOUT_READBACK, &mut readback).map_err(|e| e.to_string())?;
    if !window::validate_scanout_observation(&record, mode.width, mode.height, window::ObservationKind::Composed) {
        return Err("taskbar capture lacks a completed compositor observation".into());
    }
    for row in 0..height {
        for col in 0..width-left {
            let at = row*width*4 + (left+col)*4;
            let expected_at = row*stride + col*4;
            if got[at..at+3] != want[expected_at..expected_at+3] {
                return Err(format!("taskbar text={:?} pixel=({}, {}) got={:?} expected={:?}",
                    std::str::from_utf8(expected), left+col, row, &got[at..at+3], &want[expected_at..expected_at+3]));
            }
        }
    }
    Ok(())
}

fn wall_observed(session: &mut Session, pid: u64, id: u64, stamp: Stamp) -> Result<(), String> {
    let wall = libuitest::fixtures::WALL_BASE_NS + stamp.now_ns;
    let expected = format!("UITEST-WALL window={id} epoch={} wall_ns={wall}", stamp.epoch);
    let deadline = real_now()? + 2_000_000_000;
    loop {
        if real_now()? >= deadline { return Err(format!("wall-observation-timeout expected={expected}")); }
        let line = session.read_line(pid)?;
        if line == expected { return Ok(()); }
        if line.starts_with(&format!("UITEST-WALL window={id} epoch={} ", stamp.epoch)) {
            return Err(format!("wall time differs: got={line} expected={expected}"));
        }
    }
}

pub fn wall_time_fixed() -> CaseResult {
    let mut session = Session::desktop()?;
    let (pid, ready) = launch(&mut session, "fixed wall time")?;
    wall_observed(&mut session, pid, ready.id, Stamp::default())?;
    taskbar(b"19:00:00 ET")?; // Fixed session instant is 2024-01-01 00:00:00 UTC.
    let stamp = session.step(2000)?;
    wall_observed(&mut session, pid, ready.id, stamp)?;
    metadata(ready.id, stamp)?;
    compositor_epoch(stamp)?;
    // New scene work refreshes the taskbar; time alone adds no composite.
    session.launch_app(&[APP, b"wall taskbar redraw"], 0)?;
    session.wait_window("wall taskbar redraw")?;
    taskbar(b"19:00:02 ET")?;
    session.teardown()?;
    Ok(())
}

pub fn wm_clock() -> CaseResult {
    use libbreenix::input::key;
    use libuitest::input::{self, KeyboardBackend, LivePointer, Target};
    let mut session = Session::desktop()?;
    session.launch_app(&[APP, b"repeat UI clock", b"--widgets"], 0)?;
    let ready = session.wait_window("repeat UI clock")?;
    let mut live = LivePointer::new(&mut session)?;
    live.watch(ready.id)?;
    live.focus(&Target::new(ready.id, "entry"))?;
    let press = live.send_key(key::A, true)?;
    input::delivered(press.serial)?;
    let entry = |live: &LivePointer<'_>| -> Result<String, String> {
        let tree = live.tree(ready.id)?;
        crate::tree::node(&tree, "entry")?.value.as_str().map(str::to_owned).ok_or("entry is not text".into())
    };
    check(entry(&live)? == "a", "initial held key did not reach entry")?;
    time::sleep_ms(30).map_err(|e| e.to_string())?;
    check(entry(&live)? == "a", "frozen UI time repeated a key")?;
    let stamp = live.session().step(499)?;
    check(entry(&live)? == "a", "key repeated before 500 ms UI deadline")?;
    let status = compositor_epoch(stamp)?;
    check(status.next_deadline_ns == 500_000_000, "repeat deadline does not use UI time")?;
    live.session().step(1)?;
    // The repeat client is idle at the participant snapshot, so read its
    // committed result explicitly; the settle unit adds the general barrier.
    let end = real_now()? + 2_000_000_000;
    while entry(&live)? != "aa" {
        check(real_now()? < end, "UI repeat did not reach the committed field")?;
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    let release = live.send_key(key::A, false)?;
    input::delivered(release.serial)?;
    live.session().step(1500)?;
    check(entry(&live)? == "aa", "released key kept repeating on UI steps")?;
    drop(live);
    session.launch_app(&[APP, b"wm clock redraw"], 0)?;
    session.wait_window("wm clock redraw")?;
    taskbar(b"19:00:02 ET")?;
    session.teardown()?;
    Ok(())
}
