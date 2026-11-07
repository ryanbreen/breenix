//! Session core checks; helpers run through the real sibling supervisor.
use breenix_suite::suite::{check, CaseError, CaseResult};
use libbreenix::{process, signal};
use libuitest::{Session, ScenarioSpec, ScenarioOutcome, FailureClass, run_scenario};

const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
const FIXTURE: &[u8] = b"/usr/libexec/breenix/ui-runner-fixture_test";

fn monotonic_ns() -> Result<u64, String> {
    libbreenix::time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string())
}

fn first_window(session: &mut Session, title: &str, end: u64) -> Result<libuitest::WindowReady, CaseError> {
    let pid = session.launch_app(&[APP, title.as_bytes()], 0)?;
    let ready = session.wait_window_until(title, end)?;
    // Inspect the root observation interfaces independently of wait_window.
    let mut registry = vec![libbreenix::window::abi::WinInfo::default(); 64];
    let count = libbreenix::window::list(&mut registry).map_err(|e| e.to_string())?;
    check(count <= registry.len(), "test registry exceeds capacity")?;
    check(registry[..count].iter().any(|w| w.id == ready.id && u64::from(w.owner) == pid
        && w.title.get(..w.title_len as usize) == Some(title.as_bytes())), "returned window is absent or owned by another app")?;
    let records = std::fs::read_to_string("/proc/breenix/composites").map_err(|e| e.to_string())?;
    let expected = format!("{} {} {} {} {}", ready.composite_serial, ready.presented_ns, ready.id, ready.commit_id, ready.pixel_generation);
    check(ready.commit_id != 0 && records.lines().any(|line| line == expected), "returned commit has no completed composite record")?;
    Ok(ready)
}

fn start(kiosk: bool) -> CaseResult {
    let began = monotonic_ns()?;
    let mut session = if kiosk { Session::kiosk()? } else { Session::desktop()? };
    let result = first_window(&mut session, "startup app", began + 300_000_000);
    let elapsed = monotonic_ns()? - began;
    // Preserve the stage evidence for diagnosing a miss; keep the 300 ms bound.
    let result = result.map_err(|error| format!("{error:?}; startup_ns={elapsed}; stages={}",
        std::fs::read_to_string("/proc/breenix/stage-composites").unwrap_or_else(|e| e.to_string())));
    result?;
    check(elapsed <= 300_000_000, &format!("first composited app took {elapsed} ns; bound=300000000"))?;
    session.teardown()?;
    Ok(())
}
pub fn start_desktop() -> CaseResult { start(false) }
pub fn start_kiosk() -> CaseResult { start(true) }

pub fn wait_window() -> CaseResult {
    let mut session = Session::kiosk()?;
    let ready = first_window(&mut session, "wait for this title", monotonic_ns()? + 2_000_000_000)?;
    // Window discovery also works after excluding the window from clock participation.
    session.exclude(ready.id);
    check(session.is_excluded(ready.id), "excluded window was not retained")?;
    let again = session.wait_window("wait for this title")?;
    check(again.id == ready.id && again.commit_id != 0, "waiting for an already composited window differs")?;
    session.teardown()?;
    Ok(())
}

pub fn wait_window_timeout() -> CaseResult {
    let mut session = Session::kiosk()?;
    let began = monotonic_ns()?;
    let result = session.wait_window("a title that never appears");
    let elapsed = monotonic_ns()? - began;
    check(matches!(&result, Err(message) if message.contains("wait-window-timeout") && message.contains("a title that never appears")),
        &format!("wrong missing-window outcome: {result:?}"))?;
    check(elapsed >= 2_000_000_000, "missing-window wait ended before its bound")?;
    // Timeout leaves the session usable for a later, successful wait.
    first_window(&mut session, "after timeout", monotonic_ns()? + 2_000_000_000)?;
    session.teardown()?;
    Ok(())
}

pub fn launch_argv() -> CaseResult {
    let mut session = Session::connect()?;
    let argv: &[&[u8]] = &[APP, b"title with spaces\n=\\\"", b"--hang"];
    let env: &[&[u8]] = &[b"EMPTY=", b"VALUE=space newline\n=\\\"", b"TZ=UTC"];
    let pid = session.launch(argv, env)?;
    let echo = session.read_line(pid)?;
    // Independent literal wire expectation; do not call the app's echo encoder.
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let expected = format!("UITEST-ECHO 3 {} {} {} 3 {} {} {}", hex(APP), hex(argv[1]), hex(argv[2]), hex(env[0]), hex(env[1]), hex(env[2]));
    check(echo == expected, &format!("argv/env echo differs: {echo:?}"))?;
    session.teardown()?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "echo app survived teardown")
}

pub fn teardown_hung() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = session.launch(&[APP, b"hung", b"--hang"], &[])?;
    session.read_line(pid)?;
    check(signal::kill(pid as i32, 0).is_ok(), "hung app not alive before teardown")?;
    let exits = session.teardown()?;
    check(exits.iter().any(|e| e.pid == pid && process::wifsignaled(e.status) && process::wtermsig(e.status) == signal::SIGKILL), "hung app lacks reaped SIGKILL status")?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "hung app not reaped")
}

pub fn app_crash() -> CaseResult {
    let mut scenario = |_: &mut libuitest::RecordClient<'_>| {
        let result = (|| -> Result<(), String> {
            let mut session = Session::connect()?;
            let pid = session.launch(&[APP, b"crashing", b"--crash=11"], &[])?;
            session.wait(pid)?;
            Ok(())
        })();
        match result {
            Err(message) if message.contains("uitest-app_test") && message.contains("SIGSEGV") =>
                ScenarioOutcome::fail(FailureClass::AppCrashed, "launch", &message),
            Err(message) => ScenarioOutcome::fail(FailureClass::Infrastructure, "launch", &message),
            Ok(()) => ScenarioOutcome::pass(),
        }
    };
    let result = run_scenario(ScenarioSpec { suite: "ui-testing", case: "sessions/app-crash", checks: &[], limit_ms: 3000, run: &mut scenario });
    check(matches!(&result, ScenarioOutcome::Fail { class: FailureClass::AppCrashed, evidence, .. } if evidence.contains("uitest-app_test") && evidence.contains("SIGSEGV")), &format!("wrong crash outcome: {result:?}"))
}

pub fn orphan(session: &mut Session) -> Result<u64, String> {
    let supervisor = session.supervisor_pid().to_string();
    let app = session.launch(&[FIXTURE, b"--orphan-helper", supervisor.as_bytes()], &[])?;
    let line = session.read_line(app)?;
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "ORPHAN" || parts[2] != supervisor { return Err(format!("bad adoption witness: {line}")); }
    let pid: u64 = parts[1].parse().map_err(|_| "bad orphan pid")?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let parent = stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1));
    if parent != Some(supervisor.as_str()) { return Err("orphan parent is not supervisor".into()); }
    Ok(pid)
}

pub(crate) fn reaped_orphan() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = orphan(&mut session)?;
    // A release file gives the orphan a deterministic exit after the ppid assertion.
    std::fs::write(format!("/tmp/uitest-orphan-{pid}"), b"exit").map_err(|e| e.to_string())?;
    let exit = session.wait_exit(pid)?;
    check(process::wifexited(exit.status) && process::wexitstatus(exit.status) == 7, "orphan status is not 7")?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "orphan was not reaped")?;
    std::fs::remove_file(format!("/tmp/uitest-orphan-{pid}")).map_err(|e| e.to_string())?;
    session.teardown()?;
    Ok(())
}

pub fn subreaper() -> CaseResult {
    reaped_orphan()?;
    let mut scenario = |_: &mut libuitest::RecordClient<'_>| match reaped_orphan() {
        Ok(()) => ScenarioOutcome::pass(),
        Err(error) => ScenarioOutcome::fail(FailureClass::Infrastructure, "orphan", &format!("{error:?}")),
    };
    let result = run_scenario(ScenarioSpec { suite: "ui-testing", case: "runner/subreaper", checks: &[], limit_ms: 3000, run: &mut scenario });
    check(result == ScenarioOutcome::pass(), &format!("inner orphan: {result:?}"))?;
    for mode in ["subreaper", "supervisor-died"] {
        let (text, exit) = super::runner::fixture(mode)?;
        let failed = mode == "supervisor-died";
        check(exit == i32::from(failed), "fixture exit differs")?;
        let expected = format!("CASE fixture/{mode} {}", if failed { "FAIL" } else { "PASS" });
        check(text.contains(&expected), "fixture case verdict differs")?;
        check(text.contains(&format!("DONE passed={} failed={} skipped=0 total=1", usize::from(!failed), usize::from(failed))), "fixture DONE differs")?;
        if failed {
            check(text.contains("artifacts=none:supervisor-died") && text.contains("session supervisor died"), "missing dead supervisor failure")?;
            let prefix = "SUITE runner-fixture VALUE fixture/supervisor-died orphan-pid=";
            let pid = text.lines().find_map(|l| l.strip_prefix(prefix)).and_then(|s| s.strip_suffix("pid")).and_then(|s| s.parse::<u64>().ok()).ok_or("missing orphan pid")?;
            check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "runner did not reap orphan")?;
        }
    }
    Ok(())
}

pub fn app_stderr_capped() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = session.launch_app(&[APP, b"stderr", b"--stderr-flood=196645", b"--crash=15"], 0)?;
    let exit = session.wait_exit(pid)?;
    check(process::wifsignaled(exit.status) && process::wtermsig(exit.status) == signal::SIGTERM, "flood did not finish with its requested signal")?;
    let stderr = session.stderr(pid)?;
    check(stderr.bytes == vec![b'x'; 65_536], "stderr tail differs or exceeds 64 KiB")?;
    check(stderr.dropped == 131_109, "stderr dropped-byte count differs")?;
    session.teardown()?;
    Ok(())
}

pub fn unprivileged_app() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = session.launch_app(&[APP, b"unprivileged", b"--session-probe", b"--hang"], 1000)?;
    session.read_line(pid)?;
    let probe = session.read_line(pid)?;
    check(probe.starts_with("UITEST-SESSION uid=1000 euid=1000 "), &format!("app identity: {probe}"))?;
    for (who, expected) in [(pid, "1000"), (session.supervisor_pid(), "0"), (process::getpid().map_err(|e| e.to_string())?.raw(), "0")] {
        let status = std::fs::read_to_string(format!("/proc/{who}/status")).map_err(|e| e.to_string())?;
        let ids: Vec<_> = status.lines().find_map(|line| line.strip_prefix("Uid:")).ok_or("missing Uid")?.split_whitespace().collect();
        check(ids == vec![expected; 4], &format!("process {who} uid tuple {ids:?}"))?;
    }
    let root = session.config_root();
    let protected = std::path::Path::new(root).join("etc/fonts.conf");
    let bytes = std::fs::read(&protected).map_err(|e| e.to_string())?;
    std::fs::write(&protected, bytes).map_err(|e| e.to_string())?;
    check(probe.contains("config_writable=false"), "uid 1000 could write the root-owned config")?;
    session.teardown()?;
    Ok(())
}

pub fn environment() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = session.launch_app(&[APP, b"environment", b"--session-probe", b"--hang"], 0)?;
    let echo = session.read_line(pid)?;
    let case_pid = process::getpid().map_err(|e| e.to_string())?.raw();
    let mode = libbreenix::gpu::current_mode().map_err(|e| format!("mode: {e}"))?;
    let env = ["TZ=UTC".into(), "LANG=C.UTF-8".into(),
        format!("BREENIX_UI_CLOCK=/tmp/uitest/{case_pid}/ui-clock"), "BREENIX_UI_SEED=0".into(),
        format!("BREENIX_UI_MODE={}x{}@{}", mode.width, mode.height, mode.scale),
        format!("BREENIX_CONFIG_ROOT=/tmp/uitest/{case_pid}/")];
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let expected = format!("UITEST-ECHO 4 {} {} {} {} 6 {}", hex(APP), hex(b"environment"), hex(b"--session-probe"), hex(b"--hang"), env.iter().map(|s: &String| hex(s.as_bytes())).collect::<Vec<_>>().join(" "));
    check(echo == expected, &format!("session environment differs: {echo}"))?;
    let probe = session.read_line(pid)?;
    check(probe.contains("clock=0 epoch=0 seed=0") && probe.contains(&format!("mode={}x{}@{}", mode.width, mode.height, mode.scale)), &format!("app session inputs: {probe}"))?;
    check(probe.contains(&format!("fonts={}", libuitest::fixtures::FONT_CONFIG_SHA256)) && probe.contains(&format!("locale={}", libuitest::fixtures::LOCALE_SHA256)), "app read different fonts or locale")?;
    let root = session.config_root().to_owned();
    session.teardown()?;
    check(!std::path::Path::new(&root).exists(), "session root survived teardown")
}

pub fn fixtures_pinned() -> CaseResult {
    let mut session = Session::connect()?;
    libuitest::fixtures::verify(std::path::Path::new(session.config_root()))?;
    session.teardown()?;
    Ok(())
}


// Independent literal colors from diagnostics.rs and the GPU tile view.
const PANEL_COLORS: [[u8; 3]; 6] = [
    [31, 19, 12], [59, 39, 24], [77, 112, 26], [58, 50, 145],
    [106, 196, 69], [66, 77, 232],
];
pub(crate) fn screen_pixels() -> Result<Vec<u8>, String> {
    let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; mode.width as usize * mode.height as usize * 4];
    let mut record = [0; libbreenix::gpu::abi::record::SIZE];
    let r = libbreenix::gpu::readback(0, [0, 0, mode.width, mode.height], &mut pixels, mode.width * 4, &mut record);
    if r != 0 { return Err(format!("screen readback: {r}")); }
    Ok(pixels)
}
pub fn panel_not_in_case() -> CaseResult {
    let mut session = Session::kiosk()?;
    first_window(&mut session, "session without panel", monotonic_ns()? + 2_000_000_000)?;
    let pixels = screen_pixels()?;
    check(!pixels.chunks_exact(4).any(|p| PANEL_COLORS.iter().any(|c| p[..3] == *c)),
        "runner panel pixels leaked into session screen readback")?;
    session.teardown()?;
    Ok(())
}
fn display_fixture(mode: &str) -> CaseResult {
    let (text, exit) = super::runner::fixture(mode)?;
    let failed = mode == "session-panel-fail";
    check(exit == i32::from(failed), "display fixture exit differs")?;
    check(text.contains(&format!("CASE fixture/{mode} {} ms=", if failed { "FAIL" } else { "PASS" })), "display fixture verdict differs")?;
    check(text.contains("SESSION-DISPLAY reaped=true owned=true panel=true"), "missing post-case process, ownership and panel witness")?;
    Ok(())
}
pub fn teardown() -> CaseResult { display_fixture("session-teardown") }
pub fn panel_after_case() -> CaseResult {
    display_fixture("session-panel-pass")?;
    display_fixture("session-panel-fail")
}

use libbreenix::input::{abi::Event as InputEvent, key, kind, Batch, Capability, Injector, Reader};

struct InputProbe { reader: Reader, pending: Vec<InputEvent> }
impl InputProbe {
    fn new() -> Result<Self, String> {
        Ok(Self { reader: Reader::open(true).map_err(|e| e.to_string())?, pending: Vec::new() })
    }
    fn event(&mut self, device: u32, code: u16, value: i32) -> Result<InputEvent, String> {
        let end = monotonic_ns()? + 2_000_000_000;
        loop {
            if let Some(at) = self.pending.iter().position(|e| e.device == device && e.kind == kind::KEY && e.code == code && e.value == value) {
                return Ok(self.pending.remove(at));
            }
            let mut bytes = [0; 32 * 1024];
            match self.reader.read(&mut bytes) {
                Ok(records) => for record in records { self.pending.push(record.map_err(|e| e.to_string())?.event); },
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {},
                Err(e) => return Err(e.to_string()),
            }
            if monotonic_ns()? >= end { return Err(format!("input event timeout device={device} code={code} value={value}")); }
            libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
    }
    fn gone(&self, ids: [u32; 2]) -> CaseResult {
        let end = monotonic_ns()? + 2_000_000_000;
        loop {
            if ids.iter().all(|&id| matches!(self.reader.capabilities(id, &mut []),
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENODEV)))) { return Ok(()); }
            check(monotonic_ns()? < end, "session input device survived teardown")?;
            libbreenix::time::sleep_ms(1)?;
        }
    }
}
fn ledger_event(serial: u64, outcome: &str, window: u64, reason: &str) -> CaseResult {
    let expected = format!("{serial} {outcome} {window} {reason}");
    let end = monotonic_ns()? + 2_000_000_000;
    loop {
        let ledger = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
        if ledger.lines().any(|line| line == expected) { return Ok(()); }
        check(monotonic_ns()? < end, &format!("input ledger lacks {expected}; {ledger}"))?;
        libbreenix::time::sleep_ms(1)?;
    }
}
fn compositor_owner() -> Result<u64, String> {
    let mut status = libbreenix::window::abi::CompositorStatus::default();
    let result = unsafe { libbreenix::window::raw_call(libbreenix::window::abi::op::COMPOSITOR_STATUS,
        &mut status as *mut _ as *mut u8, core::mem::size_of_val(&status)) };
    if result != 0 { return Err(format!("compositor status {result}")); }
    Ok(status.owner as u64)
}
fn interfering_keyboard(probe: &mut InputProbe, keyboard: &Injector) -> CaseResult {
    let mut storage = [0; 96];
    let mut batch = Batch::new(&mut storage);
    batch.event(kind::KEY, key::A, 1, 0)?.event(kind::KEY, key::A, 0, 0)?.finish(0)?;
    keyboard.inject(&batch)?;
    for value in [1, 0] {
        let event = probe.event(keyboard.id(), key::A, value)?;
        ledger_event(event.serial, "rejected", 0, "not-selected")?;
    }
    Ok(())
}
pub(crate) fn device_death_round() -> Result<(u64, u64), CaseError> {
    let mut session = Session::desktop()?;
    let ready = first_window(&mut session, "selected keyboard", monotonic_ns()? + 2_000_000_000)?;
    let ids = session.input_devices();
    check(ids[0] != 0 && ids[1] != 0 && ids[0] != ids[1], "invalid session device pair")?;
    let mut probe = InputProbe::new()?;
    let third = Injector::create(&[Capability { kind: kind::KEY, code: key::A, minimum: 0, maximum: 1 }], false)?;
    interfering_keyboard(&mut probe, &third)?;
    session.raw_press(key::A)?;
    let before = probe.event(ids[0], key::A, 1)?;
    ledger_event(before.serial, "delivered", ready.id, "none")?;
    session.raw_release(key::A)?;
    probe.event(ids[0], key::A, 0)?;
    let compositor = compositor_owner()?;
    check(compositor != session.supervisor_pid(), "supervisor holds a live compositor's grab")?;
    signal::kill(compositor as i32, signal::SIGKILL)?;
    let end = monotonic_ns()? + 2_000_000_000;
    while compositor_owner().ok() == Some(compositor) {
        check(monotonic_ns()? < end, "killed compositor retained its claim")?;
        libbreenix::time::sleep_ms(1)?;
    }
    // Do not wait for Exited first: input requested at death must wait for re-grab.
    session.raw_press(key::B)?;
    let after = probe.event(ids[0], key::B, 1)?;
    let exit = session.wait_exit(compositor)?;
    check(process::wifsignaled(exit.status) && process::wtermsig(exit.status) == signal::SIGKILL, "compositor death not reaped")?;
    check(compositor_owner()? == session.supervisor_pid(), "display/compositor claim did not return to supervisor")?;
    ledger_event(after.serial, "wm-handled", 0, "none")?;
    interfering_keyboard(&mut probe, &third)?;
    session.raw_release(key::B)?;
    probe.event(ids[0], key::B, 0)?;
    session.raw_press(key::LEFT_SHIFT)?;
    session.raw_press(key::BUTTON_LEFT)?;
    session.raw_motion(-40, 25)?;
    session.raw_wheel(-1)?;
    probe.event(ids[0], key::LEFT_SHIFT, 1)?;
    probe.event(ids[1], key::BUTTON_LEFT, 1)?;
    session.teardown()?;
    probe.event(ids[0], key::LEFT_SHIFT, 0)?;
    probe.event(ids[1], key::BUTTON_LEFT, 0)?;
    probe.gone(ids)?;
    Ok((session.supervisor_pid(), compositor))
}
pub fn devices_reset() -> CaseResult { device_death_round().map(|_| ()) }
pub fn ownership_stack() -> CaseResult { display_fixture("session-ownership") }

pub fn teardown_releases() -> CaseResult {
    let mut session = Session::kiosk()?;
    first_window(&mut session, "release held inputs", monotonic_ns()? + 2_000_000_000)?;
    let ids = session.input_devices();
    let mut probe = InputProbe::new()?;
    let held = [key::LEFT_SHIFT, key::RIGHT_CTRL, key::A, key::BUTTON_LEFT, key::BUTTON_RIGHT, key::BUTTON_MIDDLE];
    for code in held {
        session.raw_press(code)?;
        probe.event(ids[usize::from(code >= key::BUTTON_LEFT)], code, 1)?;
    }
    session.raw_press(key::B)?;
    session.raw_release(key::B)?;
    probe.event(ids[0], key::B, 1)?;
    probe.event(ids[0], key::B, 0)?;
    session.teardown()?;
    for code in held { probe.event(ids[usize::from(code >= key::BUTTON_LEFT)], code, 0)?; }
    probe.gone(ids)
}

fn process_baseline() -> Result<Vec<u64>, String> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) { pids.push(pid); }
    }
    pids.sort_unstable();
    Ok(pids)
}
fn window_baseline() -> Result<Vec<u64>, String> {
    let mut windows = vec![libbreenix::window::abi::WinInfo::default(); 64];
    let n = libbreenix::window::list(&mut windows).map_err(|e| e.to_string())?;
    if n > windows.len() { return Err("window baseline exceeds capacity".into()); }
    let mut ids: Vec<_> = windows[..n].iter().map(|w| w.id).collect();
    ids.sort_unstable(); Ok(ids)
}
pub fn twenty_sessions() -> CaseResult {
    use breenix_suite::suite::memory_record;
    let pids = process_baseline()?;
    let windows = window_baseline()?;
    let before = memory_record::settled()?;
    for round in 0..20 {
        display_fixture("session-devices")?;
        check(process_baseline()? == pids, &format!("session {round} leaked processes"))?;
        check(window_baseline()? == windows, &format!("session {round} leaked windows"))?;
    }
    let after = memory_record::settled()?;
    let (record, passed) = memory_record::verdict("ui-testing", Some(&before), Some(&after));
    check(passed, &record)
}

/// A real-runner logic timeout keeps evidence inside its original limit.
pub fn case_timeout() -> CaseResult { super::runner::collection_evidence("collection-timeout", "timeout") }

/// Run a private two-suite sequence and judge its actual exec handoff (E22 D4).
pub fn sequence_compatible() -> CaseResult {
    use breenix_suite::suite::{spawn, Run};
    use libbreenix::{io, time, types::{Fd, Pid}};
    use std::{fs::File, io::Read, os::fd::FromRawFd};
    struct Child(Option<Pid>);
    impl Drop for Child {
        fn drop(&mut self) {
            if let Some(pid) = self.0.take() {
                let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
                let _ = process::waitpid(pid.raw() as i32, &mut 0, 0);
            }
        }
    }
    struct Directory(std::path::PathBuf);
    impl Drop for Directory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let own = process::getpid()?.raw();
    let directory = Directory(format!("/tmp/ui-sequence-{own}").into());
    std::fs::create_dir(&directory.0).map_err(|e| e.to_string())?;
    std::fs::write(directory.0.join("suite-sequence"), b"runner-fixture,sequence-sentinel\n").map_err(|e| e.to_string())?;
    // The scoped executable directory follows the usual suite-<id> lookup,
    // with private links to U7a's installed libexec fixtures.
    for (id, path) in [("runner-fixture", "/usr/libexec/breenix/ui-runner-fixture_test"),
                       ("sequence-sentinel", "/usr/libexec/breenix/ui-sequence-sentinel_test")] {
        std::os::unix::fs::symlink(path, directory.0.join(format!("suite-{id}"))).map_err(|e| e.to_string())?;
    }
    let mut hold = || -> i32 { loop { let _ = time::sleep_ms(100); } };
    let helper = Child(Some(spawn(&[], Run::Call(&mut hold))?));
    let (r, w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    let mut reader = unsafe { File::from_raw_fd(r.raw() as i32) };
    let writer = unsafe { File::from_raw_fd(w.raw() as i32) };
    io::fcntl_setfl(r, io::status_flags::O_NONBLOCK)?;
    let mut exec = || {
        std::env::set_var("BREENIX_SUITE_TRANSCRIPT_FD", "1");
        std::env::set_var("BREENIX_SUITE_SEQUENCE_DIR", &directory.0);
        std::env::set_var("BREENIX_UI_SEQUENCE_FIXTURE", "1");
        let path = b"/usr/libexec/breenix/ui-runner-fixture_test\0";
        let argv = [path.as_ptr(), core::ptr::null()];
        let _ = process::execv(path, argv.as_ptr());
        127
    };
    let mut fixture = Child(Some(spawn(&[(w, Fd::STDOUT), (w, Fd::STDERR)], Run::Call(&mut exec))?));
    let fixture_pid = fixture.0.unwrap().raw();
    drop(writer);
    let end = monotonic_ns()? + 8_000_000_000;
    let mut bytes = Vec::new();
    let mut status = None;
    let mut eof = false;
    while !eof || status.is_none() {
        let mut chunk = [0; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => eof = true,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
            Err(e) => return Err(e.to_string().into()),
        }
        check(bytes.len() <= 2 * 1024 * 1024, "sequence transcript exceeds bound")?;
        if status.is_none() {
            let mut value = 0;
            if process::waitpid(fixture_pid as i32, &mut value, 1)?.raw() != 0 {
                status = Some(value); fixture.0 = None;
            }
        }
        check(monotonic_ns()? < end, "sequence did not exit with transcript EOF")?;
        time::sleep_ms(1)?;
    }
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let lines: Vec<_> = text.lines().filter(|s| !s.is_empty()).collect();
    let expected = [
        "SUITE runner-fixture START cases=1",
        "SUITE runner-fixture CASE fixture/sequence-session PASS ms=",
        "SUITE runner-fixture DONE passed=1 failed=0 skipped=0 total=1",
        "SUITE_SEQUENCE READY runner-fixture",
        "SUITE_SEQUENCE RESET sequence-sentinel removed=",
        "SUITE sequence-sentinel START cases=1",
        "SUITE sequence-sentinel CASE reset/state PASS ms=0",
        "SUITE sequence-sentinel DONE passed=1 failed=0 skipped=0 total=1",
        "SUITE_SEQUENCE DONE suites=2",
    ];
    let mut after = 0;
    for record in expected {
        let matches: Vec<_> = lines.iter().enumerate().filter(|(_, line)| line.starts_with(record)).collect();
        check(matches.len() == 1 && matches[0].0 >= after, &format!("sequence missing, repeated or out of order: {record}; {text}"))?;
        after = matches[0].0 + 1;
    }
    check(!lines.iter().any(|l| l.starts_with("SUITE_SEQUENCE RESET ") && l.contains(" FAIL ")), "sequence reset failed")?;
    check(!text.contains("SUITE_SEQUENCE FAIL exec"), "sequence skipped an unexecutable suite")?;
    let reset = lines.iter().find(|l| l.starts_with("SUITE_SEQUENCE RESET sequence-sentinel ")).unwrap();
    check(reset.contains(" changed=0 tcp=0 "), &format!("sequence filesystem or TCP reset differs: {reset}"))?;
    let witness = lines.iter().find_map(|l| l.strip_prefix("SEQUENCE-DESCENDANT pid=")).ok_or("missing sequence sweep witness")?;
    let descendant: u64 = witness.parse().map_err(|_| "bad sequence descendant pid")?;
    check(descendant > 1 && !std::path::Path::new(&format!("/proc/{descendant}/stat")).exists(), "fixture descendant survived handoff")?;
    let status = status.unwrap();
    check(process::wifexited(status) && process::wexitstatus(status) == 0, &format!("sentinel failed: {text}"))?;
    // GIVE_BACK is EPERM unless this outer case still owns the display.
    libbreenix::graphics::give_back_display()?;
    libbreenix::graphics::take_over_display()?;
    check(signal::kill(helper.0.unwrap().raw() as i32, 0).is_ok(), "sequence killed outer helper")?;
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", helper.0.unwrap().raw())).map_err(|e| e.to_string())?;
    check(stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1)).and_then(|s| s.parse::<u64>().ok()) == Some(own),
        "outer case lost helper parentage")
}
