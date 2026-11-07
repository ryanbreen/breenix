//! Intentionally misbehaving tables for docs/gui/plan.md section 8.4.
#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;
use breenix_suite::suite::{self, case, category, check, spawn, CaseResult, Run};
use libbreenix::{io, process, signal, time, types::Fd};

const PATH: &[u8] = b"/usr/libexec/breenix/ui-runner-fixture_test\0";

fn frame(check: &str, case: &str, mismatch: bool, wide: bool) -> String {
    let source = if wide { format!("window:{}", "a".repeat(3700)) } else { "surface".into() };
    let result = if mismatch { "mismatch compared=1 differing=1 first=0,0 got=#000000 want=#ffffff artifacts=none:planted" } else { "match" };
    format!("SUITE runner-fixture FRAME fixture/{case} {check} def=00000000 ref=formula source={source} commit=1 epoch=0 w=1 h=1 sha256={} result={result}", "0".repeat(64))
}
fn mode() -> String { suite::running_case().unwrap().strip_prefix("fixture/").unwrap().into() }
fn positive() -> CaseResult { suite::result_record(&frame("label", &mode(), false, false)) }
fn malformed() -> CaseResult { suite::result_record("broken record") }
fn duplicate() -> CaseResult { positive()?; positive() }
fn other_case() -> CaseResult { suite::result_record(&frame("label", "another", false, false)) }
fn undeclared() -> CaseResult { suite::result_record(&frame("other", &mode(), false, false)) }
fn mismatch() -> CaseResult { suite::result_record(&frame("label", &mode(), true, false)) }
fn never_ran() -> CaseResult { Ok(()) }

fn parent_pid() -> u64 {
    let text = std::fs::read_to_string("/proc/self/stat").unwrap();
    text.rsplit_once(") ").unwrap().1.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn wait(pid: u64) -> CaseResult {
    let mut status = 0;
    process::waitpid(pid as i32, &mut status, 0)?;
    check(process::wifexited(status) && process::wexitstatus(status) == 0, "fixture helper failed")
}
fn read_exact(fd: Fd, bytes: &mut [u8]) -> CaseResult {
    let mut at = 0;
    while at < bytes.len() { let n = io::read(fd, &mut bytes[at..])?; check(n > 0, "fixture helper EOF")?; at += n; }
    Ok(())
}

fn poll_byte(fd: Fd, expected: u8) -> CaseResult {
    let start = time::now_monotonic()?.as_nanos();
    loop {
        let mut byte = [0];
        match io::read(fd, &mut byte) {
            Ok(1) => return check(byte == [expected], "wrong fixture progress witness"),
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {},
            _ => return Err("fixture progress pipe failed".into()),
        }
        check(time::now_monotonic()?.as_nanos() - start < 1_000_000_000, "fixture progress timed out")?;
        time::sleep_ms(1)?;
    }
}

fn blocking() -> CaseResult {
    let runner = parent_pid();
    let writer_pid = process::getpid()?.raw();
    let (ready_r, ready_w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    let (done_r, done_w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    io::fcntl_setfl(done_r, io::status_flags::O_NONBLOCK)?;
    let mut stopper = || {
        let ready = Fd::from_raw(3);
        let done = Fd::from_raw(4);
        let result = (|| -> CaseResult {
            signal::kill(runner as i32, signal::SIGSTOP)?;
            let start = time::now_monotonic()?.as_nanos();
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{runner}/stat")).map_err(|e| e.to_string())?;
                if stat.rsplit_once(") ").is_some_and(|(_, r)| r.starts_with("T ")) { break; }
                check(time::now_monotonic()?.as_nanos() - start < 500_000_000, "runner did not stop")?;
                time::sleep_ms(1)?;
            }
            io::write(ready, &[1])?;
            // Progress then a sleeping writer: after this witness the case only
            // writes records, and its sole result reader is stopped.
            poll_byte(done, 1)?;
            let start = time::now_monotonic()?.as_nanos();
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{writer_pid}/stat")).map_err(|e| e.to_string())?;
                if stat.rsplit_once(") ").is_some_and(|(_, r)| r.starts_with("S ")) { break; }
                check(time::now_monotonic()?.as_nanos() - start < 500_000_000, "writer never blocked")?;
                time::sleep_ms(1)?;
            }
            let mut byte = [0];
            check(matches!(io::read(done, &mut byte), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN))),
                  "writer completed while its reader was stopped")?;
            Ok(())
        })();
        let resumed = signal::kill(runner as i32, signal::SIGCONT);
        let completed = if result.is_ok() && resumed.is_ok() { poll_byte(done, 2) } else { Ok(()) };
        i32::from(result.is_err() || resumed.is_err() || completed.is_err())
    };
    let helper = spawn(&[(ready_w,Fd::from_raw(3)), (done_r,Fd::from_raw(4))], Run::Call(&mut stopper))?;
    let _ = (io::close(ready_w), io::close(done_r));
    read_exact(ready_r, &mut [0])?;
    for i in 0..256 {
        suite::result_record(&frame(&format!("check-{i}"), &mode(), false, true))?;
        if i == 15 { io::write(done_w, &[1])?; }
    }
    io::write(done_w, &[2])?;
    let _ = (io::close(ready_r), io::close(done_w));
    wait(helper.raw())?;
    suite::value("blocked", 1, "bool", None);
    Ok(())
}

fn open_fds() -> Vec<u64> {
    (3..4096).filter(|&fd| io::fcntl(Fd::from_raw(fd), io::fcntl_cmd::F_GETFD, 0).is_ok()).collect()
}
fn probe(fds: &[u64]) -> CaseResult {
    // Probe the original descriptor numbers too: an inactive Rust client alone
    // would miss a leaked raw writer. fd 3 is the intentionally kept reply pipe.
    check(suite::result_record("forged").is_err(), "helper retained result authority")?;
    for &fd in fds {
        check(io::write(Fd::from_raw(fd), b"forged\n").is_err(), "helper retained a runner descriptor")?;
    }
    Ok(())
}
fn forgery(exec: bool) -> CaseResult {
    let inherited = open_fds();
    let (r,w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    let mut call = || {
        let ok = probe(&inherited).is_ok();
        let _ = io::write(Fd::from_raw(3), &[u8::from(ok)]);
        i32::from(!ok)
    };
    let list = inherited.iter().map(u64::to_string).collect::<Vec<_>>().join(",") + "\0";
    let args: &[&[u8]] = &[PATH, b"--probe-exec\0", list.as_bytes()];
    let pid = spawn(&[(w,Fd::from_raw(3))], if exec { Run::Exec(PATH,args) } else { Run::Call(&mut call) })?;
    let _ = io::close(w);
    let mut byte = [0];
    read_exact(r, &mut byte)?;
    let _ = io::close(r);
    wait(pid.raw())?;
    check(byte == [1], "forgery helper did not prove descriptor closure")?;
    positive()
}
fn forgery_exec() -> CaseResult { forgery(true) }
fn forgery_fork() -> CaseResult { forgery(false) }

fn hold() -> i32 { loop { let _ = time::sleep_ms(100); } }
fn descendants() -> CaseResult {
    let (r,w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    let mut orphan_parent = || {
        let mut orphan = || {
            let mut nested = hold;
            let pid = match spawn(&[], Run::Call(&mut nested)) { Ok(pid) => pid, Err(_) => return 1 };
            let own = process::getpid().unwrap().raw();
            let mut bytes = [0;16];
            bytes[..8].copy_from_slice(&own.to_le_bytes());
            bytes[8..].copy_from_slice(&pid.raw().to_le_bytes());
            let _ = io::write(Fd::from_raw(3), &bytes);
            hold()
        };
        i32::from(spawn(&[(Fd::from_raw(3),Fd::from_raw(3))], Run::Call(&mut orphan)).is_err())
    };
    let parent = spawn(&[(w,Fd::from_raw(3))], Run::Call(&mut orphan_parent))?;
    let _ = io::close(w);
    let mut pids = [0;16];
    read_exact(r, &mut pids)?;
    let _ = io::close(r);
    wait(parent.raw())?;
    let orphan = u64::from_le_bytes(pids[..8].try_into().unwrap());
    let nested = u64::from_le_bytes(pids[8..].try_into().unwrap());
    check(parent_pid_of(orphan)? == parent_pid(), "orphan did not reach the runner subreaper")?;
    let mut direct = hold;
    let direct = spawn(&[], Run::Call(&mut direct))?;
    suite::value("direct-pid", direct.raw() as i64, "pid", None);
    suite::value("orphan-pid", orphan as i64, "pid", None);
    suite::value("nested-pid", nested as i64, "pid", None);
    positive()
}
fn parent_pid_of(pid: u64) -> Result<u64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    text.rsplit_once(") ").and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|n| n.parse().ok()).ok_or("bad parent stat".into())
}

#[path = "uitest_suite/sessions.rs"]
pub mod sessions;
#[path = "uitest_suite/runner.rs"]
pub mod runner;
#[path = "uitest_suite/config.rs"]
pub mod config_cases;
fn settings_a() -> CaseResult { config_cases::settings_round(1) }
fn settings_b() -> CaseResult { config_cases::settings_round(2) }
fn config_baseline_path(pid: u64) -> String { format!("/tmp/ui-config-baseline-{pid}") }
fn config_crash_marker() -> String { format!("/tmp/ui-config-crash-{}", parent_pid()) }
fn settings_crash() -> CaseResult {
    let session = libuitest::Session::connect()?;
    let root = std::path::Path::new(session.config_root());
    std::fs::write(config_crash_marker(), session.config_root()).map_err(|e| e.to_string())?;
    for (name, data) in [("fonts.conf", "planted fonts"), ("bwm.conf", "font.size=12\nclock.visible=false\n"),
        ("hotkeys.conf", "Ctrl+a = focus_next\n"), ("app.conf", "theme=crashed\n")] {
        std::fs::write(root.join("etc").join(name), data).map_err(|e| e.to_string())?;
    }
    std::fs::write("/etc/fonts.conf", b"planted global config fault\n").map_err(|e| e.to_string())?;
    signal::kill(process::getpid()?.raw() as i32, signal::SIGKILL)?;
    Err("SIGKILL returned".into())
}
fn settings_defaults() -> CaseResult {
    let marker = config_crash_marker();
    let root = std::fs::read_to_string(&marker).map_err(|e| e.to_string())?;
    check(!std::path::Path::new(&root).exists(), "crashed session root survived into next case")?;
    std::fs::remove_file(marker).map_err(|e| e.to_string())?;
    let baseline_path = config_baseline_path(parent_pid());
    let before = std::fs::read(&baseline_path).map_err(|e| e.to_string())?;
    let after = match std::fs::read("/etc/fonts.conf") {
        Ok(bytes) => { let mut saved = vec![1]; saved.extend(bytes); saved },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![0],
        Err(e) => return Err(e.to_string().into()),
    };
    check(before == after, "global fonts were not restored before the next case")?;
    std::fs::remove_file(baseline_path).map_err(|e| e.to_string())?;
    config_cases::settings_round(0)
}
fn config_sequence(order: &str) -> ! {
    if order == "crash" {
        let bytes = match std::fs::read("/etc/fonts.conf") {
            Ok(bytes) => { let mut saved = vec![1]; saved.extend(bytes); saved },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![0],
            Err(e) => panic!("global font baseline: {e}"),
        };
        std::fs::write(config_baseline_path(process::getpid().expect("fixture pid").raw()), bytes).expect("save global baseline");
    }
    let a = case("settings-a", "First settings scenario", settings_a).session(libuitest::supervisor::serve);
    let b = case("settings-b", "Second settings scenario", settings_b).session(libuitest::supervisor::serve);
    let entries = match order {
        "ab" => vec![a, b], "ba" => vec![b, a],
        "crash" => vec![case("settings-crash", "Crash during settings write", settings_crash).session(libuitest::supervisor::serve),
            case("settings-defaults", "Read defaults after crash", settings_defaults).session(libuitest::supervisor::serve)],
        _ => panic!("unknown config sequence"),
    };
    let cases = Box::leak(entries.into_boxed_slice());
    let categories = Box::leak(vec![category("fixture", "Config isolation", cases)].into_boxed_slice());
    suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    suite::suite("runner-fixture", "Runner config fixture", categories).gui().defer_display().case_limit_ms(6000).run()
}


fn collection_case() -> CaseResult {
    let mut session = libuitest::Session::kiosk()?;
    let app = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"collection witness", b"--stderr-flood=4096"], 0)?;
    session.read_line(app)?;
    let window = session.wait_window("collection witness")?;
    session.hold_window(window.id)?;
    session.raw_press(libbreenix::input::key::A)?;
    session.raw_release(libbreenix::input::key::A)?;
    let stderr_end = time::now_monotonic()?.as_nanos() + 500_000_000;
    loop {
        let stderr = session.stderr(app)?;
        if stderr.bytes.len() == 4096 {
            check(stderr.bytes == vec![b'x'; 4096], "stderr precondition differs")?; break;
        }
        check(time::now_monotonic()?.as_nanos() < stderr_end, "stderr flood did not finish before the case fault")?;
        time::sleep_ms(1)?;
    }
    let pid = process::getpid()?.raw();
    suite::value("supervisor-pid", session.supervisor_pid() as i64, "pid", None);
    let marker = format!("/tmp/ui-collection-{}", parent_pid());
    std::fs::write(marker, format!("{} {} {}", session.supervisor_pid(), app, time::now_monotonic()?.as_nanos())).map_err(|e| e.to_string())?;
    // Deliberately leave a slot writing: no incomplete pixels may be exported.
    let mut slot = session.arena_mut().begin(libuitest::arena::Slot::Frame)?;
    slot.bytes_mut()[..4].copy_from_slice(&[1, 2, 3, 255]);
    drop(slot);
    if mode() == "collection-death" {
        signal::kill(pid as i32, signal::SIGKILL)?;
        return Err("SIGKILL returned".into());
    }
    loop { time::sleep_ms(10)?; }
}
fn collection_observer(name: &str) {
    let result = (|| -> CaseResult {
        let marker = format!("/tmp/ui-collection-{}", process::getpid()?.raw());
        let content = std::fs::read_to_string(&marker).map_err(|e| e.to_string())?;
        let fields: Vec<_> = content.split_whitespace().collect();
        for pid in &fields[..2] {
            check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "collection sweep left a process")?;
        }
        let began: u64 = fields[2].parse().map_err(|_| "invalid fixture start")?;
        let elapsed = (time::now_monotonic()?.as_nanos() as u64 - began) / 1_000_000;
        let path = std::path::Path::new("/var/uitest/runner-fixture").join(name);
        check(path.join("collection.txt").is_file() && path.join("screen.png").is_file(), "sweep preceded disk evidence")?;
        let mut status = 0;
        check(matches!(process::waitpid(-1, &mut status, process::WNOHANG), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ECHILD))), "collection left descendants")?;
        std::fs::remove_file(marker).map_err(|e| e.to_string())?;
        let _ = io::write(Fd::from_raw(3), format!("COLLECTION-SWEEP reaped=true evidence=true elapsed_ms={elapsed}\n").as_bytes());
        Ok(())
    })();
    if let Err(e) = result { let _ = io::write(Fd::from_raw(3), format!("COLLECTION-SWEEP failed {e:?}\n").as_bytes()); }
}

fn collection_fault() -> CaseResult {
    let mut session = libuitest::Session::connect()?;
    let orphan = sessions::orphan(&mut session)?;
    let supervisor = session.supervisor_pid();
    suite::value("supervisor-pid", supervisor as i64, "pid", None);
    suite::value("orphan-pid", orphan as i64, "pid", None);
    signal::kill(supervisor as i32, if mode() == "silent-supervisor" { signal::SIGSTOP } else { signal::SIGKILL })?;
    Err("planted collection failure".into())
}

fn supervisor_died() -> CaseResult {
    let mut session = libuitest::Session::connect()?;
    let orphan = sessions::orphan(&mut session)?;
    let supervisor = session.supervisor_pid();
    signal::kill(supervisor as i32, signal::SIGKILL)?;
    let start = time::now_monotonic()?.as_nanos();
    while parent_pid_of(orphan)? != parent_pid() {
        check(time::now_monotonic()?.as_nanos() - start < 500_000_000, "orphan did not reach runner after supervisor death")?;
        time::sleep_ms(1)?;
    }
    suite::value("orphan-pid", orphan as i64, "pid", None);
    // Return Ok: the real runner must convert it to FAIL for the dead supervisor.
    Ok(())
}

fn orphan_helper(expected: u64) -> i32 {
    let mut middle = || {
        let mut leaf = || {
            let pid = process::getpid().unwrap().raw();
            let path = format!("/tmp/uitest-orphan-{pid}");
            let start = time::now_monotonic().unwrap().as_nanos();
            while parent_pid() != expected {
                if time::now_monotonic().unwrap().as_nanos() - start > 1_000_000_000 { return 2; }
                let _ = time::sleep_ms(1);
            }
            println!("ORPHAN {pid} {expected}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            while !std::path::Path::new(&path).exists() { let _ = time::sleep_ms(1); }
            7
        };
        i32::from(spawn(&[], Run::Call(&mut leaf)).is_err())
    };
    match spawn(&[], Run::Call(&mut middle)) {
        Ok(pid) => i32::from(wait(pid.raw()).is_err()),
        Err(_) => 1,
    }
}

fn session_display() -> CaseResult {
    if mode() == "session-ownership" {
        let (supervisor, compositor) = sessions::device_death_round()?;
        std::fs::write(format!("/tmp/ui-display-{}", parent_pid()),
            format!("{supervisor} {compositor}\n")).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut session = libuitest::Session::kiosk()?;
    let app = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"panel fixture"], 0)?;
    session.wait_window("panel fixture")?;
    let mut status = libbreenix::window::abi::CompositorStatus::default();
    let result = unsafe { libbreenix::window::raw_call(libbreenix::window::abi::op::COMPOSITOR_STATUS,
        &mut status as *mut _ as *mut u8, core::mem::size_of_val(&status)) };
    check(result == 0 && status.owner != 0, "missing live session compositor")?;
    std::fs::write(format!("/tmp/ui-display-{}", parent_pid()),
        format!("{} {} {}\n{} {}", session.supervisor_pid(), app, status.owner, session.input_devices()[0], session.input_devices()[1])).map_err(|e| e.to_string())?;
    if mode() == "session-devices" {
        for code in [libbreenix::input::key::LEFT_SHIFT, libbreenix::input::key::BUTTON_LEFT] {
            session.raw_press(code)?;
        }
    }
    // Leave compositor and app running: the real runner must end the session.
    if mode() == "session-panel-fail" { return Err("planted panel result".into()); }
    Ok(())
}
fn display_observer(done: usize, _: usize, name: &str) {
    if done != 0 && name.starts_with("fixture/collection-") { collection_observer(name); return; }
    if done == 0 || !name.starts_with("fixture/session-") { return; }
    let result = (|| -> CaseResult {
        let path = format!("/tmp/ui-display-{}", process::getpid()?.raw());
        let pids = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        for pid in pids.lines().next().ok_or("missing session pids")?.split_whitespace() {
            check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "session process survived case end")?;
        }
        if let Some(ids) = pids.lines().nth(1) {
            let reader = libbreenix::input::Reader::open(true)?;
            for id in ids.split_whitespace() {
                let id: u32 = id.parse().map_err(|_| "invalid session device id")?;
                check(matches!(reader.capabilities(id, &mut []), Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENODEV))), "fixture session device survived")?;
            }
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        let mut status = 0;
        check(matches!(process::waitpid(-1, &mut status, process::WNOHANG),
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ECHILD))), "fixture still has descendants")?;
        let pixels = sessions::screen_pixels()?;
        let failed = name.ends_with("-fail");
        // The text panel and tile panel each have literal, distinct result fills.
        let colors = if failed { [[58, 50, 145], [66, 77, 232]] } else { [[77, 112, 26], [106, 196, 69]] };
        let opposite = if failed { [[77, 112, 26], [106, 196, 69]] } else { [[58, 50, 145], [66, 77, 232]] };
        check(pixels.chunks_exact(4).any(|p| colors.iter().any(|c| p[..3] == *c)), "case result color absent from panel")?;
        check(!pixels.chunks_exact(4).any(|p| opposite.iter().any(|c| p[..3] == *c)), "panel shows opposite case result")?;
        // GIVE_BACK is EPERM for a process that is not the current owner.
        libbreenix::graphics::give_back_display()?;
        libbreenix::graphics::take_over_display()?;
        Ok(())
    })();
    let line = match result {
        Ok(()) => "SESSION-DISPLAY reaped=true owned=true panel=true\n".to_owned(),
        Err(error) => format!("SESSION-DISPLAY failed {error:?}\n"),
    };
    let _ = io::write(Fd::from_raw(3), line.as_bytes());
}

fn main() {
    if let Ok(order) = std::env::var("BREENIX_CONFIG_SCENARIOS") { config_sequence(&order); }
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--orphan-helper") {
        process::exit(orphan_helper(args[2].parse().expect("supervisor pid")));
    }
    if args.get(1).is_some_and(|s| s == "--probe-exec") {
        let fds: Vec<u64> = args[2].split(',').filter_map(|s| s.parse().ok()).collect();
        let ok = probe(&fds).is_ok();
        let _ = io::write(Fd::from_raw(3), &[u8::from(ok)]);
        process::exit(i32::from(!ok));
    }
    let selected = args.get(2).and_then(|s| s.strip_prefix("fixture/")).expect("--case fixture/<mode>");
    let (body, checks): (fn() -> CaseResult, Vec<&'static str>) = match selected {
        "transport-blocking" => (blocking, (0..256).map(|i| &*Box::leak(format!("check-{i}").into_boxed_str())).collect()),
        "transport-capacity" => (never_ran, (0..257).map(|i| &*Box::leak(format!("check-{i}").into_boxed_str())).collect()),
        "record-malformed" => (malformed, vec!["label"]),
        "record-duplicate" => (duplicate, vec!["label"]),
        "other-case" => (other_case, vec!["label"]),
        "undeclared" => (undeclared, vec!["label"]),
        "never-ran" => (never_ran, vec!["label"]),
        "mismatch-is-fail" => (mismatch, vec!["label"]),
        "forgery-exec" => (forgery_exec, vec!["label"]),
        "forgery-fork" => (forgery_fork, vec!["label"]),
        "subreaper" => (sessions::reaped_orphan, vec![]),
        "session-teardown" | "session-panel-pass" | "session-panel-fail" | "session-ownership" | "session-devices" => (session_display, vec![]),
        "supervisor-died" => (supervisor_died, vec![]),
        "silent-supervisor" | "dead-supervisor" => (collection_fault, vec![]),
        "collection-death" | "collection-timeout" => (collection_case, vec![]),
        "sweep-descendants" => (descendants, vec!["label"]),
        "outer-positive" | "transport-always" | "transport-exit-race" => (positive, vec!["label"]),
        _ => panic!("unknown fixture mode"),
    };
    let id = Box::leak(selected.to_string().into_boxed_str());
    let checks = Box::leak(checks.into_boxed_slice());
    let entry = case(id, "Scoped runner control", body).checks(checks);
    let entry = if matches!(selected, "subreaper" | "supervisor-died" | "session-teardown" | "session-panel-pass" | "session-panel-fail" | "session-ownership" | "session-devices") { entry.session(libuitest::supervisor::serve) } else { entry };
    let entry = if matches!(selected, "silent-supervisor" | "dead-supervisor" | "collection-death" | "collection-timeout") {
        entry.session(libuitest::supervisor::serve).collects()
    } else { entry };
    let cases = Box::leak(vec![entry].into_boxed_slice());
    let categories = Box::leak(vec![category("fixture", "Scoped runner", cases)].into_boxed_slice());
    suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    suite::suite("runner-fixture", "Runner fixture", categories).gui().defer_display().with_case_observer(display_observer).case_limit_ms(3000).run()
}
