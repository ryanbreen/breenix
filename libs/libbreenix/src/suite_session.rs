//! Runner ownership of a sibling session supervisor. Non-session cases bypass it.
use std::{fs::File, os::fd::{AsRawFd, FromRawFd, IntoRawFd}, sync::atomic::{AtomicI32, Ordering}};
use libbreenix::{error::Error, errno::Errno, io, process, signal, socket, time, types::{Fd, Pid}};
use super::{spawn, Run};
use super::control::{Watchdog, Wait};
use super::collection::{Handshake, Request, State};

static CONTROL: AtomicI32 = AtomicI32::new(-1);

pub(super) fn clear_session_control() { CONTROL.store(-1, Ordering::Relaxed); }

pub fn take_session_control() -> Option<Fd> {
    let fd = CONTROL.swap(-1, Ordering::Relaxed);
    (fd >= 0).then(|| Fd::from_raw(fd as u64))
}
/// Used after the inner executor's descriptor arrangement, exactly like the
/// outer runner's child handoff. Helpers still go through spawn's close rule.
pub fn install_session_control(fd: Fd) {
    CONTROL.store(fd.raw() as i32, Ordering::Relaxed);
}
fn fd(file: &File) -> Fd { Fd::from_raw(file.as_raw_fd() as u64) }
fn pair() -> Result<(File, File), String> {
    let (a, b) = socket::socketpair(socket::AF_UNIX, socket::SOCK_STREAM | socket::SOCK_CLOEXEC, 0).map_err(|e| e.to_string())?;
    Ok(unsafe { (File::from_raw_fd(a.raw() as i32), File::from_raw_fd(b.raw() as i32)) })
}
fn now() -> Result<i128, String> { time::now_monotonic().map(|t| t.as_nanos()/1_000_000).map_err(|e| e.to_string()) }

// SENDTO with MSG_NOSIGNAL: the supervisor can exit between waitpid and
// the command write. A peer close must report supervisor-died, not kill us.
fn send_collector(collector: Fd, bytes: &[u8]) -> Result<usize, Error> {
    use libbreenix::syscall::{nr, raw};
    let ret = unsafe { raw::syscall6(nr::SENDTO, collector.raw(), bytes.as_ptr() as u64, bytes.len() as u64, 0x4000, 0, 0) as i64 };
    Error::from_syscall(ret).map(|n| n as usize)
}

pub struct RunnerSession {
    pid: Option<Pid>,
    control: Option<File>,
    collector: Option<File>,
}
impl RunnerSession {
    pub fn start(serve: fn(Fd, Fd) -> i32) -> Result<Self, String> {
        Self::start_named(serve, None)
    }
    pub fn start_collecting(serve: fn(Fd, Fd) -> i32, suite: &str, case: &str) -> Result<Self, String> {
        Self::start_named(serve, Some((suite, case)))
    }
    fn start_named(serve: fn(Fd, Fd) -> i32, names: Option<(&str, &str)>) -> Result<Self, String> {
        let (case, supervisor) = pair()?;
        let (runner, collect) = pair()?;
        io::fcntl_setfl(fd(&runner), io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        let mut call = || {
            if let Some((suite, case)) = names {
                std::env::set_var("BREENIX_COLLECT_SUITE", suite);
                std::env::set_var("BREENIX_COLLECT_CASE", case);
            }
            serve(Fd::from_raw(3), Fd::from_raw(4))
        };
        let pid = spawn(&[(fd(&supervisor), Fd::from_raw(3)), (fd(&collect), Fd::from_raw(4))], Run::Call(&mut call)).map_err(|e| e.to_string())?;
        Ok(Self { pid: Some(pid), control: Some(case), collector: Some(runner) })
    }
    pub fn case_fd(&self) -> Fd { fd(self.control.as_ref().expect("case control")) }
    pub fn in_child(&mut self) {
        // The case is the supervisor's sibling, and owns no collector endpoint.
        self.pid = None;
        self.collector.take();
        let control = self.control.take().expect("case control").into_raw_fd();
        install_session_control(Fd::from_raw(control as u64));
    }
    pub fn in_parent(&mut self) { self.control.take(); }
    /// Collection uses one absolute hang watchdog, including command writes and
    /// fragmented reads. A done reply authorizes the sweep without an exit wait.
    pub fn collect_until(&mut self, deadline_ns: Option<i128>, request: Request, suite: &str, case: &str,
        mut record: impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
        self.in_parent();
        let watchdog = Watchdog::new(Wait::CollectorReply)?;
        let deadline = deadline_ns.ok_or("collector monotonic clock unavailable")?.min(i128::from(watchdog.deadline_ns()));
        let collector = fd(self.collector.as_ref().ok_or("missing collector")?);
        let mut handshake = Handshake::new(suite, case, request);
        let command = handshake.request().command();
        let mut sent = 0;
        loop {
            let now = time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
            if now >= deadline { return Err("collector-timeout".into()); }
            let mut dead = self.pid.is_none();
            if let Some(pid) = self.pid {
                let mut status = 0;
                match process::waitpid(pid.raw() as i32, &mut status, process::WNOHANG) {
                    Ok(reaped) if reaped.raw() != 0 => { self.pid = None; dead = true; }
                    Ok(_) | Err(Error::Os(Errno::EINTR)) => {},
                    Err(Error::Os(Errno::ECHILD)) => { self.pid = None; dead = true; }
                    Err(error) => return Err(format!("session supervisor wait: {error}")),
                }
            }
            if sent < command.len() && !dead {
                match send_collector(collector, &command.as_bytes()[sent..]) {
                    Ok(0) => dead = true,
                    Ok(n) => sent += n,
                    Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                    Err(_) => dead = true,
                }
            }
            // Drain after waitpid as well: an exit cannot erase queued done.
            // Exhausting this batch is not EOF; a fragmented reply may remain.
            let mut drained = false;
            for _ in 0..16 {
                let mut bytes = [0; 4096];
                match io::read(collector, &mut bytes) {
                    Ok(0) => { dead = true; drained = true; break; }
                    Ok(n) => { for line in handshake.feed(&bytes[..n])? { record(&line)?; } }
                    Err(Error::Os(Errno::EINTR)) => continue,
                    Err(Error::Os(Errno::EAGAIN)) => { drained = true; break; }
                    Err(_) => { dead = true; drained = true; break; }
                }
            }
            let now = time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
            match handshake.state(now, deadline, dead && drained) {
                State::Done => return Ok(()),
                State::Died => return Err("session supervisor died".into()),
                State::Timeout => return Err("collector-timeout".into()),
                State::Waiting => {},
            }
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
    }
    /// The runner's bounded sweep now owns kill/reap. Do not retain its pid in
    /// Drop after that sweep, when it might already have been reaped and reused.
    pub(super) fn handoff_to_sweep(&mut self) { self.pid = None; }
    /// Existing discard-only teardown for cases that do not declare collection
    /// and for the inner executor. Keep its exit-before-sweep order.
    pub fn finish(&mut self, reserve_ms: u64) -> Result<(), String> {
        self.in_parent();
        let watchdog = Watchdog::new(Wait::DiscardReply)?;
        let end = (now()? + i128::from(reserve_ms)).min(watchdog.deadline_ms());
        let collector = fd(self.collector.as_ref().ok_or("missing collector")?);
        let mut command = &b"end discard\n"[..];
        let mut reply = Vec::new();
        loop {
            let mut status = 0;
            match process::waitpid(self.pid.ok_or("missing supervisor")?.raw() as i32, &mut status, process::WNOHANG) {
                Ok(pid) if pid.raw() != 0 => {
                    self.pid = None;
                    // Drain after exit too: done may have arrived between this
                    // poll's read and waitpid, and must not be lost to that race.
                    let mut bytes = [0; 16];
                    if let Ok(n) = io::read(collector, &mut bytes) { reply.extend_from_slice(&bytes[..n]); }
                    return if reply == b"done\n" && process::wifexited(status) && process::wexitstatus(status) == 0 { Ok(()) }
                        else { Err("session supervisor died".into()) };
                }
                Ok(_) | Err(Error::Os(Errno::EINTR)) => {},
                Err(error) => return Err(format!("session supervisor wait: {error}")),
            }
            if !command.is_empty() {
                match io::write(collector, command) {
                    Ok(n) if n != 0 => command = &command[n..],
                    Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                    _ => return Err("session supervisor died".into()),
                }
            }
            let mut bytes = [0; 16];
            match io::read(collector, &mut bytes) {
                Ok(n) => {
                    reply.extend_from_slice(&bytes[..n]);
                    if reply.len() > 5 || !b"done\n".starts_with(&reply) { return Err("invalid collector reply".into()); }
                }
                Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                Err(_) => return Err("session supervisor died".into()),
            }
            if now()? >= end { return Err("collector-timeout".into()); }
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
    }
}
impl Drop for RunnerSession {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
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
