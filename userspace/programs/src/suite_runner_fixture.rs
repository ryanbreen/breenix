//! Intentionally misbehaving tables for docs/gui/plan.md section 8.4.
#[path = "suite_breengel/stage_painter.rs"]
mod stage_painter;
use breenix_suite::suite::{self, case, category, check, spawn, CaseResult, Run};
use libbreenix::{io, process, signal, time, types::Fd};

const PATH: &[u8] = b"/usr/local/test/bin/suite-runner-fixture_test\0";

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
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
        "sweep-descendants" => (descendants, vec!["label"]),
        "outer-positive" | "transport-always" | "transport-exit-race" => (positive, vec!["label"]),
        _ => panic!("unknown fixture mode"),
    };
    let id = Box::leak(selected.to_string().into_boxed_str());
    let checks = Box::leak(checks.into_boxed_slice());
    let cases = Box::leak(vec![case(id, "Scoped runner control", body).checks(checks)].into_boxed_slice());
    let categories = Box::leak(vec![category("fixture", "Scoped runner", cases)].into_boxed_slice());
    suite::set_painter(Box::new(stage_painter::StagePainter::new()));
    suite::suite("runner-fixture", "Runner fixture", categories).gui().defer_display().case_limit_ms(3000).run()
}
