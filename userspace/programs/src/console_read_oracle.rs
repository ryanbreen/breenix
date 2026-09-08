//! Generic devfs input arms run by the blocking-I/O driver in a quiet fixture.
use super::{check, cv, errno, mode, now, reap, receive, send, TestResult, Witness};
use libbreenix::syscall::{nr, raw::syscall3};
use libbreenix::types::Fd;
use libbreenix::{
    fs, io,
    process::{self, ForkResult},
    signal,
};
const QUERY: u64 = 0xB8130003;
const INJECT: u64 = 0xB8130004;
pub const ARMS: [&str; 6] = [
    "blocking",
    "nonblock_open",
    "nonblock_fcntl",
    "readiness_partial",
    "eintr",
    "immediate",
];
fn observe(fd: Fd, tid: u64) -> TestResult<Witness> {
    let mut w = Witness {
        tid,
        target_fd: fd.raw(),
        ..Witness::default()
    };
    check(
        unsafe { syscall3(nr::IOCTL, fd.raw(), QUERY, &mut w as *mut _ as u64) } == 0,
        "input query",
    )?;
    Ok(w)
}
fn inject(fd: Fd, tid: u64, byte: u8) -> TestResult<()> {
    let w = Witness {
        tid,
        target_fd: fd.raw(),
        identity: byte as u64,
        ..Witness::default()
    };
    check(
        unsafe { syscall3(nr::IOCTL, fd.raw(), INJECT, &w as *const _ as u64) } == 0,
        "input insertion failed",
    )
}
fn clean(fd: Fd, tid: u64) -> TestResult<()> {
    let w = observe(fd, tid)?;
    check(
        w.queued == 0 && w.blocked == 0 && w.blocked_in_syscall == 0,
        "residual input waiter/state",
    )
}
fn readiness(fd: Fd, input: bool) -> TestResult<()> {
    let mut fds = [io::PollFd::new(
        fd,
        io::poll_events::POLLIN | io::poll_events::POLLOUT,
    )];
    check(cv(io::poll(&mut fds, 0))? == 1, "POLLOUT lost")?;
    check(
        fds[0].revents
            == io::poll_events::POLLOUT | if input { io::poll_events::POLLIN } else { 0 },
        "input readiness mismatch",
    )
}
fn parked(fd: Fd, tid: u64) -> TestResult<()> {
    let deadline = now() + 5000;
    loop {
        let w = observe(fd, tid)?;
        check(w.occupancy == 0, "ring not empty")?;
        if w.queued == 1 && w.blocked == 1 && w.blocked_in_syscall == 1 {
            return Ok(());
        }
        check(now() < deadline, "reader never parked")?;
        cv(process::yield_now())?;
    }
}
fn blocking(fd: Fd, interrupted: bool) -> TestResult<()> {
    cv(signal::sigaction(
        signal::SIGUSR1,
        Some(&signal::Sigaction::new(super::caught_signal)),
        None,
    ))?;
    let (ack_r, ack_w) = cv(io::pipe())?;
    let (go_r, go_w) = cv(io::pipe())?;
    mode(ack_r, true)?;
    mode(go_r, true)?;
    let pid = match cv(process::fork())? {
        ForkResult::Child => {
            let run = || -> TestResult<()> {
                cv(io::close(ack_r))?;
                cv(io::close(go_w))?;
                send(ack_w, cv(process::gettid())?.raw() as i64)?;
                let mut byte = [0xa5];
                let result = errno(io::read(fd, &mut byte));
                send(ack_w, result)?;
                send(ack_w, byte[0] as i64)?;
                receive(go_r)?;
                if interrupted {
                    check(
                        cv(io::read(fd, &mut byte))? == 1 && byte == [b'8'],
                        "EINTR recovery read",
                    )?;
                    send(ack_w, 1)?;
                    receive(go_r)?;
                }
                Ok(())
            };
            process::exit(if run().is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };
    cv(io::close(ack_w))?;
    cv(io::close(go_r))?;
    let run = || -> TestResult<()> {
        let tid = receive(ack_r)? as u64;
        parked(fd, tid)?;
        if interrupted {
            cv(signal::kill(pid, signal::SIGUSR1))?;
        } else {
            inject(fd, tid, b'8')?;
        }
        check(
            receive(ack_r)? == if interrupted { -4 } else { 1 },
            "victim read result",
        )?;
        check(
            receive(ack_r)? == if interrupted { 0xa5 } else { b'8' as i64 },
            "victim exact byte",
        )?;
        clean(fd, tid)?;
        if interrupted {
            inject(fd, cv(process::gettid())?.raw(), b'8')?;
            send(go_w, 1)?;
            check(receive(ack_r)? == 1, "recovery ack")?;
            clean(fd, tid)?;
        }
        send(go_w, 1)?;
        reap(pid)
    };
    let result = run();
    if result.is_err() {
        let _ = signal::kill(pid, signal::SIGKILL);
        let _ = reap(pid);
    }
    cv(io::close(ack_r))?;
    cv(io::close(go_w))?;
    result
}
fn arm(path: &str, name: &str) -> TestResult<usize> {
    let fd = cv(fs::open(
        path,
        fs::O_RDONLY
            | if name == "nonblock_open" {
                fs::O_NONBLOCK
            } else {
                0
            },
    ))?;
    let tid = cv(process::gettid())?.raw();
    let w = observe(fd, tid)?;
    check(
        w.kind == if path == "/dev/console" { 3 } else { 4 },
        "not generic devfs type",
    )?;
    check(w.occupancy == 0 && w.queued == 0, "fixture not quiet")?;
    match name {
        "blocking" | "eintr" => blocking(fd, name == "eintr")?,
        "nonblock_open" | "nonblock_fcntl" => {
            if name == "nonblock_fcntl" {
                mode(fd, true)?;
            }
            check(
                errno(io::read(fd, &mut [0; 1])) == -11,
                "empty nonblock must be EAGAIN",
            )?;
            clean(fd, tid)?;
        }
        "readiness_partial" => {
            readiness(fd, false)?;
            inject(fd, tid, b'8')?;
            inject(fd, tid, b'9')?;
            readiness(fd, true)?;
            let mut bytes = [0xa5; 8];
            check(
                cv(io::read(fd, &mut bytes))? == 2
                    && bytes[..2] == *b"89"
                    && bytes[2..] == [0xa5; 6],
                "partial exact bytes/count",
            )?;
            readiness(fd, false)?;
        }
        "immediate" => {
            check(cv(io::read(fd, &mut []))? == 0, "zero count")?;
            for path in ["/dev/null", "/dev/zero"] {
                let control = cv(fs::open(path, fs::O_RDONLY))?;
                let mut bytes = [0xa5; 8];
                let n = cv(io::read(control, &mut bytes))?;
                check(
                    if path == "/dev/null" {
                        n == 0 && bytes == [0xa5; 8]
                    } else {
                        n == 8 && bytes == [0; 8]
                    },
                    "immediate device",
                )?;
                cv(io::close(control))?;
            }
            clean(fd, tid)?;
        }
        _ => return Err("unknown console arm".into()),
    }
    check(observe(fd, tid)?.occupancy == 0, "input left behind")?;
    cv(io::close(fd))?;
    Ok(match name {
        "blocking" | "eintr" => 1,
        "readiness_partial" => 2,
        "immediate" => 8,
        _ => 0,
    })
}
pub fn run(arch: &str) -> bool {
    // Driver is an init child; a new session cannot own an existing controlling PTY.
    if cv(process::setsid()).is_err() {
        super::emit("[CONSOLE_READ_SETUP:FAIL:setsid]");
        return false;
    }
    let mut passed = 0;
    for path in ["/dev/console", "/dev/tty"] {
        for name in ARMS {
            match arm(path, name) {
                Ok(bytes) => {
                    passed += 1;
                    super::emit(&format!(
                        "[CONSOLE_READ_ORACLE:{}:{}:{}:verdict=PASS:bytes={}]",
                        arch, path, name, bytes
                    ));
                }
                Err(e) => {
                    super::emit(&format!(
                        "[CONSOLE_READ_ORACLE:{}:{}:{}:verdict=FAIL:{}]",
                        arch, path, name, e
                    ));
                    return false;
                }
            }
        }
    }
    super::emit(&format!(
        "[CONSOLE_READ_SUMMARY:{}:passed={}:failed=0]",
        arch, passed
    ));
    passed == 2 * ARMS.len()
}
