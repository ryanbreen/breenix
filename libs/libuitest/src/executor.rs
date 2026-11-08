//! A subreaper controller outlives the scenario logic so even a logic crash has
//! descendant-scoped teardown. Both forks use the suite's descriptor rule.
use std::{fs::File, io::{Read, Write}, os::fd::{AsRawFd, FromRawFd}};
use breenix_suite::suite::{descendants::sweep_descendants, spawn, Run};
use libbreenix::{error::Error, errno::Errno, io, process, signal, time, types::{Fd, Pid}};
use crate::{RecordClient, ScenarioOutcome, FailureClass, outcome::{decode, encode, MAX_OUTCOME_BYTES}};

pub struct ScenarioSpec<'a> {
    pub suite: &'a str,
    pub case: &'a str,
    pub checks: &'a [&'a str],
    /// Real time for the scenario logic. Teardown has the runner's separate 2 s bound.
    pub limit_ms: u64,
    pub run: &'a mut dyn FnMut(&mut RecordClient<'_>) -> ScenarioOutcome,
}

fn infrastructure(operation: &str, evidence: impl std::fmt::Display) -> ScenarioOutcome {
    ScenarioOutcome::fail(FailureClass::Infrastructure, operation, &evidence.to_string())
}

fn now_ms() -> Result<i128, Error> { Ok(time::now_monotonic()?.as_nanos() / 1_000_000) }

struct Child(Option<Pid>);
impl Drop for Child {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
            let mut status = 0;
            loop {
                match process::waitpid(pid.raw() as i32, &mut status, 0) {
                    Err(Error::Os(Errno::EINTR)) => continue,
                    _ => break,
                }
            }
        }
    }
}

fn pipe() -> Result<(File, File), String> {
    let (read, write) = io::pipe2(io::status_flags::O_CLOEXEC).map_err(|e| e.to_string())?;
    // Ownership of both raw descriptors moves into these Files.
    let reader = unsafe { File::from_raw_fd(read.raw() as i32) };
    let writer = unsafe { File::from_raw_fd(write.raw() as i32) };
    io::fcntl_setfl(read, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
    Ok((reader, writer))
}

fn send(outcome: &ScenarioOutcome) -> i32 {
    let fallback;
    let frame = match encode(outcome) {
        Ok(frame) => frame,
        Err(error) => {
            fallback = infrastructure("encode-outcome", format!("{error:?}"));
            match encode(&fallback) { Ok(frame) => frame, Err(_) => return 125 }
        }
    };
    // spawn moved the private outcome writer to 3. Keep it private on exec too.
    if io::fcntl_setfd(Fd::from_raw(3), io::fd_flags::FD_CLOEXEC).is_err() { return 125; }
    let mut writer = unsafe { File::from_raw_fd(3) };
    if writer.write_all(&frame).is_ok() { 0 } else { 125 }
}

/// Drain while polling, then to EOF. A full frame does not excuse a nonzero exit
/// or signal; a writer cannot turn a later crash into successful detection.
#[derive(Clone, Copy)]
enum WriterRole { Logic, Controller }

fn receive(reader: &mut File, child: &mut Child, limit_ms: u64, role: WriterRole) -> ScenarioOutcome {
    let start = match now_ms() { Ok(now) => now, Err(e) => return infrastructure("clock", e) };
    let mut bytes = Vec::new();
    let mut status = None;
    let mut eof = false;
    loop {
        if !eof {
            let mut chunk = [0u8; 8192];
            match reader.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if n > MAX_OUTCOME_BYTES.saturating_sub(bytes.len()) {
                        return infrastructure("outcome-pipe", "oversized frame");
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
                Err(error) => return infrastructure("outcome-pipe", error),
            }
        }
        if status.is_none() {
            let mut code = 0;
            match process::waitpid(child.0.unwrap().raw() as i32, &mut code, process::WNOHANG) {
                Ok(pid) if pid.raw() != 0 => { child.0 = None; status = Some(code); },
                Ok(_) | Err(Error::Os(Errno::EINTR)) => {},
                Err(error) => return infrastructure("wait-scenario", error),
            }
        }
        // A signaled or failed writer needs no EOF from any stray descriptor holder.
        if let Some(status) = status {
            if process::wifsignaled(status) {
                return match role {
                    WriterRole::Logic => ScenarioOutcome::fail(FailureClass::AppCrashed, "scenario", &format!("signal={}", process::wtermsig(status))),
                    WriterRole::Controller => infrastructure("executor-crash", format!("signal={}", process::wtermsig(status))),
                };
            }
            if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                return infrastructure("scenario-exit", format!("status={status}"));
            }
            if eof { return decode(&bytes).unwrap_or_else(|e| infrastructure("decode-outcome", format!("{e:?}"))); }
        }
        match now_ms() {
            Ok(now) if now - start < i128::from(limit_ms) => {},
            Ok(_) => return infrastructure("scenario-deadline", "logic or outcome pipe did not complete"),
            Err(error) => return infrastructure("clock", error),
        }
        if let Err(error) = time::sleep_ms(1) { return infrastructure("scenario-poll", error); }
    }
}

fn controller(spec: &mut ScenarioSpec<'_>) -> ScenarioOutcome {
    if let Err(error) = process::set_child_subreaper(true) { return infrastructure("set-subreaper", error); }
    let mut session = match breenix_suite::suite::session_runner::RunnerSession::start(crate::supervisor::serve) {
        Ok(session) => session,
        Err(error) => return infrastructure("session-start", error),
    };
    let result = (|| {
        let (mut reader, writer) = match pipe() { Ok(pair) => pair, Err(e) => return infrastructure("logic-pipe", e) };
        let control = session.case_fd();
        let mut logic = || {
            breenix_suite::suite::session_runner::install_session_control(Fd::from_raw(4));
            let outcome = match process::set_child_subreaper(true) {
                Err(error) => infrastructure("set-subreaper", error),
                Ok(()) => match RecordClient::contained(spec.suite, spec.case, spec.checks) {
                    Err(error) => infrastructure("record-client", error),
                    Ok(mut records) => {
                        let outcome = (spec.run)(&mut records);
                        records.finish(outcome)
                    }
                }
            };
            send(&outcome)
        };
        let pid = match spawn(&[(Fd::from_raw(writer.as_raw_fd() as u64), Fd::from_raw(3)), (control, Fd::from_raw(4))], Run::Call(&mut logic)) {
            Ok(pid) => pid,
            Err(error) => return infrastructure("spawn-logic", error),
        };
        session.in_parent();
        drop(writer);
        let mut child = Child(Some(pid));
        // Child's guard stops an unfinished logic process before the controller sweep.
        receive(&mut reader, &mut child, spec.limit_ms, WriterRole::Logic)
    })();
    let result = match session.finish(1500) {
        Ok(()) => result,
        Err(error) => match result {
            ScenarioOutcome::Pass { .. } => infrastructure("session-teardown", error),
            failure => failure,
        },
    };
    if let Some(error) = sweep_descendants() {
        return infrastructure("scenario-teardown", format!("{error}; primary={result:?}"));
    }
    result
}

/// Execute under an isolated subreaper, return a typed outcome and contained
/// records only after descendant teardown. This adds no architecture branches.
pub fn run_scenario(mut spec: ScenarioSpec<'_>) -> ScenarioOutcome {
    if spec.limit_ms == 0 || spec.limit_ms > 60_000 {
        return infrastructure("scenario-spec", "limit_ms outside 1..60000");
    }
    // Reject invalid declarations before creating processes.
    if let Err(error) = RecordClient::contained(spec.suite, spec.case, spec.checks) {
        return infrastructure("scenario-spec", error);
    }
    let (mut reader, writer) = match pipe() { Ok(pair) => pair, Err(e) => return infrastructure("outcome-pipe", e) };
    let limit = spec.limit_ms + 3000;
    let mut supervise = || { send(&controller(&mut spec)) };
    let pid = match spawn(&[(Fd::from_raw(writer.as_raw_fd() as u64), Fd::from_raw(3))], Run::Call(&mut supervise)) {
        Ok(pid) => pid,
        Err(error) => return infrastructure("spawn-scenario", error),
    };
    drop(writer);
    receive(&mut reader, &mut Child(Some(pid)), limit, WriterRole::Controller)
}
