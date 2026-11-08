//! Runner self-tests: judge the real scoped runner's transcript, not an inner outcome.
use breenix_suite::suite::{check, fail, spawn, CaseResult, Run};
use libbreenix::{io, process, signal, time, types::{Fd, Pid}};
use std::{fs::File, io::Read, os::fd::{AsRawFd, FromRawFd}};

const FIXTURE: &[u8] = b"/usr/libexec/breenix/ui-runner-fixture_test\0";

struct Child(Option<Pid>);
impl Drop for Child {
    fn drop(&mut self) {
        let Some(pid) = self.0 else { return };
        let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
        let mut status = 0;
        let _ = process::waitpid(pid.raw() as i32, &mut status, 0);
    }
}

pub(crate) fn fixture(mode: &str) -> Result<(String, i32), breenix_suite::suite::CaseError> {
    capture(mode, false)
}
pub(crate) fn config_fixture(order: &str) -> Result<(String, i32), breenix_suite::suite::CaseError> {
    capture(order, true)
}
fn capture(mode: &str, config_sequence: bool) -> Result<(String, i32), breenix_suite::suite::CaseError> {
    let (r, w) = io::pipe2(io::status_flags::O_CLOEXEC)?;
    let mut reader = unsafe { File::from_raw_fd(r.raw() as i32) };
    let writer = unsafe { File::from_raw_fd(w.raw() as i32) };
    io::fcntl_setfl(r, io::status_flags::O_NONBLOCK)?;
    let selection = format!("fixture/{mode}\0").into_bytes();
    let args: &[&[u8]] = &[FIXTURE, b"--case\0", &selection];
    // Environment is changed in the child only; the outer case keeps its own result pipe.
    let mut exec = || {
        std::env::set_var("BREENIX_SUITE_TRANSCRIPT_FD", "3");
        if config_sequence { std::env::set_var("BREENIX_CONFIG_SCENARIOS", mode); }
        let argv = if config_sequence { [args[0].as_ptr(), core::ptr::null(), core::ptr::null(), core::ptr::null()] }
            else { [args[0].as_ptr(), args[1].as_ptr(), args[2].as_ptr(), core::ptr::null()] };
        let _ = process::execv(FIXTURE, argv.as_ptr());
        127
    };
    let mut child = Child(Some(spawn(&[(Fd::from_raw(writer.as_raw_fd() as u64), Fd::from_raw(3))], Run::Call(&mut exec))?));
    drop(writer);
    let start = time::now_monotonic()?.as_nanos();
    let mut bytes = Vec::new();
    let mut status = None;
    let mut eof = false;
    while !eof || status.is_none() {
        let mut chunk = [0u8; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => eof = true,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
            Err(e) => return fail(format!("fixture transcript read: {e}")).map(|_| unreachable!()),
        }
        check(bytes.len() <= 2 * 1024 * 1024, "fixture transcript exceeds bound")?;
        if status.is_none() {
            let mut s = 0;
            let pid = process::waitpid(child.0.unwrap().raw() as i32, &mut s, 1)?;
            if pid.raw() != 0 { status = Some(s); child.0 = None; }
        }
        check(time::now_monotonic()?.as_nanos() - start < 8_000_000_000, "fixture did not exit with transcript EOF")?;
        if !eof && status.is_none() { time::sleep_ms(1)?; }
    }
    // Already reaped: do not signal a pid which could later be reused.
    let status = status.unwrap();
    check(process::wifexited(status), "fixture crashed instead of producing a verdict")?;
    let text = String::from_utf8(bytes).map_err(|_| "fixture transcript is not UTF-8")?;
    Ok((text, process::wexitstatus(status)))
}

pub(crate) fn collection_evidence(mode: &str, trigger: &str) -> CaseResult {
    let (text, exit) = fixture(mode)?;
    check(exit == 1, "killed fixture did not fail")?;
    let collect = format!("SUITE runner-fixture COLLECT fixture/{mode} trigger={trigger} screen=at-collection");
    let at = text.find(&collect).ok_or("collection did not save a screen")?;
    let done = text.find(" collection-done=1bool").ok_or("runner never received done")?;
    let swept = text.find("COLLECTION-SWEEP reaped=true evidence=true").ok_or("evidence or sweep witness missing")?;
    check(at < done && done < swept, "COLLECT/done did not arrive before sweep")?;
    let case = text.find(&format!("CASE fixture/{mode} FAIL")).ok_or("fixture case did not fail")?;
    check(done < case && text.contains("DONE passed=0 failed=1 skipped=0 total=1"), "fixture result ordering differs")?;
    let path = std::path::Path::new("/var/uitest/runner-fixture/fixture").join(mode);
    let manifest = std::fs::read_to_string(path.join("collection.txt")).map_err(|e| e.to_string())?;
    check(manifest.contains(&format!("trigger={trigger}")) && manifest.contains("frame=partial:case-died"), "missing death/timeout or partial-slot evidence")?;
    check(std::fs::read(path.join("screen.png")).map_err(|e| e.to_string())?.starts_with(b"\x89PNG\r\n\x1a\n"), "screen is not a saved PNG")?;
    for name in ["screen.record", "input.log", "composites.log"] {
        check(std::fs::metadata(path.join(name)).map_err(|e| e.to_string())?.len() != 0, &format!("collection evidence is empty: {name}"))?;
    }
    check(std::fs::read_dir(&path).map_err(|e| e.to_string())?.any(|p| p.is_ok_and(|p| p.file_name().to_string_lossy().starts_with("window-"))), "no live window capture saved")?;
    let stderr = std::fs::read_dir(&path).map_err(|e| e.to_string())?.filter_map(Result::ok)
        .filter(|p| p.file_name().to_string_lossy().starts_with("stderr-"))
        .map(|p| std::fs::read(p.path()).unwrap_or_default()).collect::<Vec<_>>();
    check(stderr.iter().any(|b| b.len() == 4096 && b.iter().all(|&v| v == b'x')), "app stderr was not saved")?;
    if trigger == "timeout" {
        check(text.contains("timed out after 1500 ms"), "timeout did not use the logic bound")?;
        let elapsed: u64 = text.split(" collection-ms=").nth(1).ok_or("collection elapsed missing")?
            .split("ms").next().unwrap().parse().map_err(|_| "invalid collection elapsed")?;
        check(elapsed < 3000, "evidence missed the reserve")?;
    }
    std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    Ok(())
}
