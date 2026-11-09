//! Intentionally misbehaving tables for docs/gui/plan.md section 8.4.
#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;
use breenix_suite::suite::{self, case, category, check, spawn, CaseResult, Run};
use libbreenix::{io, process, signal, time, types::Fd};

fn mode() -> String { suite::running_case().unwrap().strip_prefix("fixture/").unwrap().into() }

fn parent_pid() -> u64 {
    let text = std::fs::read_to_string("/proc/self/stat").unwrap();
    text.rsplit_once(") ").unwrap().1.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn wait(pid: u64) -> CaseResult {
    let mut status = 0;
    process::waitpid(pid as i32, &mut status, 0)?;
    check(process::wifexited(status) && process::wexitstatus(status) == 0, "fixture helper failed")
}
fn parent_pid_of(pid: u64) -> Result<u64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    text.rsplit_once(") ").and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|n| n.parse().ok()).ok_or("bad parent stat".into())
}

#[path = "uitest_suite/sessions.rs"]
pub mod sessions;
#[path = "uitest_suite/runner.rs"]
pub mod runner;
#[path = "uitest_suite/capture_app.rs"]
mod capture_app;
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



/// Fixture logic returns a failure after the app exit so the real runner collects.
fn app_death_case() -> CaseResult {
    let mut session = libuitest::Session::desktop()?;
    let mut app = capture_app::App::launch(&mut session)?;
    let initial = app.command("create")?;
    session.wait_window("capture races")?;
    session.hold_window(initial.id)?;
    let final_commit = app.command("commit 0")?;
    check(final_commit.id == initial.id && final_commit.generation.commit == initial.generation.commit + 1,
        "app death did not follow the commanded commit")?;
    signal::kill(app.pid as i32, signal::SIGKILL)?;
    let exit = session.wait_exit(app.pid)?;
    check(process::wifsignaled(exit.status) && process::wtermsig(exit.status) == signal::SIGKILL,
        &format!("app death status differs: {}", exit.status))?;
    let mut live = [libbreenix::window::abi::WinInfo::default(); 64];
    let count = libbreenix::window::list(&mut live).map_err(|e| e.to_string())?;
    check(count <= live.len() && !live[..count].iter().any(|w| w.id == final_commit.id),
        "dead app window still live before collection")?;
    // The independent oracle travels through the fixture's existing VALUE
    // transport; only the collector writes the remnant artifact files.
    for (name, value) in [("remnant-window", final_commit.id), ("remnant-commit", final_commit.generation.commit),
        ("remnant-producer", u64::from(final_commit.generation.producer)),
        ("remnant-width", u64::from(final_commit.width)), ("remnant-height", u64::from(final_commit.height))] {
        suite::value(name, value as i64, "count", None);
    }
    Err("app-death witness".into())
}

fn collection_case() -> CaseResult {
    let mut session = libuitest::Session::kiosk()?;
    let app = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"collection witness", b"--stderr-flood=4096"], 0)?;
    session.read_line(app)?;
    let window = session.wait_window("collection witness")?;
    session.hold_window(window.id)?;
    session.raw_press(libbreenix::input::key::A)?;
    session.raw_release(libbreenix::input::key::A)?;
    let input_end = time::now_monotonic()?.as_nanos() + 500_000_000;
    loop {
        let ledger = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
        if ledger.lines().any(|l| l.contains(" wm-handled ")) { break; }
        check(time::now_monotonic()?.as_nanos() < input_end, "kiosk did not dispose the collection input")?;
        time::sleep_ms(1)?;
    }
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
    let selected = args.get(2).and_then(|s| s.strip_prefix("fixture/")).expect("--case fixture/<mode>");
    let (body, checks): (fn() -> CaseResult, Vec<&'static str>) = match selected {
        "subreaper" => (sessions::reaped_orphan, vec![]),
        "session-teardown" | "session-ownership" | "session-devices" => (session_display, vec![]),
        "supervisor-died" => (supervisor_died, vec![]),
        "collection-death" | "collection-timeout" => (collection_case, vec![]),
        "app-death" => (app_death_case, vec![]),
        _ => panic!("unknown fixture mode"),
    };
    let id = Box::leak(selected.to_string().into_boxed_str());
    let checks = Box::leak(checks.into_boxed_slice());
    let entry = case(id, "Scoped runner control", body).checks(checks);
    let entry = if matches!(selected, "subreaper" | "supervisor-died" | "session-teardown" | "session-ownership" | "session-devices") { entry.session(libuitest::supervisor::serve) } else { entry };
    let entry = if matches!(selected, "collection-death" | "collection-timeout" | "app-death") {
        entry.session(libuitest::supervisor::serve).collects()
    } else { entry };
    let cases = Box::leak(vec![entry].into_boxed_slice());
    let categories = Box::leak(vec![category("fixture", "Scoped runner", cases)].into_boxed_slice());
    suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    suite::suite("runner-fixture", "Runner fixture", categories).gui().defer_display().with_case_observer(display_observer)
        .case_limit_ms(if selected == "app-death" { 6000 } else { 3000 }).run()
}
