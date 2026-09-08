//! Unix stream count, backpressure, mode, close, signal and readiness oracle.
use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::socket::{self, SockAddrUn, AF_UNIX, SOCK_NONBLOCK, SOCK_STREAM};
use libbreenix::syscall::{nr, raw::syscall3};
use libbreenix::types::Fd;
use libbreenix::{
    io,
    process::{self, ForkResult},
    signal, time,
};
const C: usize = 65536;
const A: usize = 4096;
const QUERY: u64 = 0xB8130001;
const ARMS: [&str; 6] = [
    "backpressure",
    "mode",
    "poll",
    "peer_close",
    "signal",
    "partial",
];
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Witness {
    tid: u64,
    target_fd: u64,
    kind: u64,
    identity: u64,
    queued: u64,
    blocked: u64,
    blocked_in_syscall: u64,
    occupancy: u64,
    capacity: u64,
    atomic_limit: u64,
}
type TestResult<T> = Result<T, String>;
fn check(ok: bool, detail: &str) -> TestResult<()> {
    if ok {
        Ok(())
    } else {
        Err(detail.into())
    }
}
fn cv<T>(result: Result<T, Error>) -> TestResult<T> {
    result.map_err(|e| format!("{}", e))
}
fn now() -> u64 {
    time::now_monotonic().expect("monotonic clock").as_millis() as u64
}
fn mode(fd: Fd, nonblock: bool) -> TestResult<()> {
    cv(io::fcntl_setfl(
        fd,
        if nonblock {
            io::status_flags::O_NONBLOCK
        } else {
            0
        },
    ))?;
    Ok(())
}
fn errno(result: Result<usize, Error>) -> i64 {
    match result {
        Ok(n) => n as i64,
        Err(Error::Os(e)) => -(e as i64),
    }
}
fn query(fd: Fd, target_fd: Fd, tid: u64) -> TestResult<Witness> {
    let mut witness = Witness {
        tid,
        target_fd: target_fd.raw(),
        ..Witness::default()
    };
    let result =
        unsafe { syscall3(nr::IOCTL, fd.raw(), QUERY, &mut witness as *mut _ as u64) } as i64;
    check(result == 0, &format!("query returned {}", result))?;
    check(
        witness.kind == 3 && witness.capacity == C as u64 && witness.atomic_limit == 0,
        "not a Unix stream witness",
    )?;
    check(witness.identity != 0, "missing pair identity")?;
    Ok(witness)
}
fn read_exact(fd: Fd, len: usize) -> TestResult<Vec<u8>> {
    let deadline = now() + 5000;
    let mut bytes = vec![0; len];
    let mut offset = 0;
    while offset < len {
        match io::read(fd, &mut bytes[offset..]) {
            Ok(0) => return Err("unexpected EOF".into()),
            Ok(n) => offset += n,
            Err(Error::Os(Errno::EAGAIN)) if now() < deadline => {
                cv(process::yield_now())?;
            }
            Err(e) => return Err(format!("read offset={}:{}", offset, e)),
        }
        check(now() <= deadline, "read deadline")?;
    }
    Ok(bytes)
}
fn send(fd: Fd, value: i64) -> TestResult<()> {
    check(
        cv(io::write(fd, &value.to_ne_bytes()))? == 8,
        "short control write",
    )
}
fn receive(fd: Fd) -> TestResult<i64> {
    Ok(i64::from_ne_bytes(
        read_exact(fd, 8)?.try_into().expect("eight bytes"),
    ))
}
fn reap(pid: i32) -> TestResult<()> {
    let deadline = now() + 5000;
    loop {
        let mut status = -1;
        let got = cv(process::waitpid(pid, &mut status, 1))?;
        if got.raw() != 0 {
            return check(
                got.raw() == pid as u64
                    && process::wifexited(status)
                    && process::wexitstatus(status) == 0,
                &format!("child status={}", status),
            );
        }
        check(now() < deadline, "reap deadline")?;
        cv(process::yield_now())?;
    }
}
fn fill(fd: Fd, amount: usize, byte: u8) -> TestResult<()> {
    check(
        cv(io::write(fd, &vec![byte; amount]))? == amount,
        "short initial fill",
    )
}
fn payload(amount: usize) -> Vec<u8> {
    (0..amount)
        .map(|index| ((index * 37 + index / 251) % 251) as u8)
        .collect()
}
fn wait_witness(peer: Fd, writer: Fd, tid: u64) -> TestResult<()> {
    let deadline = now() + 2000;
    loop {
        let w = query(peer, writer, tid)?;
        if w.queued == 1 && w.blocked == 1 && w.blocked_in_syscall == 1 {
            return check(w.occupancy == C as u64, "queued occupancy");
        }
        check(now() < deadline, "no queued writer witness")?;
        cv(process::yield_now())?;
    }
}
extern "C" fn caught_signal(_: i32) {}
fn clean(fd: Fd) -> TestResult<()> {
    let w = query(fd, fd, cv(process::gettid())?.raw())?;
    check(
        w.queued == 0 && w.blocked == 0 && w.blocked_in_syscall == 0,
        "residual waiter/blocked flag",
    )
}
fn poll_out(fd: Fd, expected: bool) -> TestResult<()> {
    let mut fds = [io::PollFd::new(fd, io::poll_events::POLLOUT)];
    cv(io::poll(&mut fds, 0))?;
    check(
        (fds[0].revents & io::poll_events::POLLOUT != 0) == expected,
        "POLLOUT predicate",
    )
}
fn construction_control() -> TestResult<()> {
    let (writer, reader) = cv(socket::socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0))?;
    fill(writer, C, 0xee)?;
    check(
        errno(io::write(writer, &[7])) == -(Errno::EAGAIN as i64),
        "construction mode",
    )?;
    clean(writer)?;
    check(
        read_exact(reader, C)? == vec![0xee; C],
        "construction data changed",
    )?;
    cv(io::close(writer))?;
    cv(io::close(reader))
}
// The child is the writer in both direction variants. Choosing which process listens reverses A/B without
// ever inheriting connected endpoints across fork.
fn connection(arm: &str, reverse: bool, variant: usize) -> TestResult<usize> {
    let (ack_r, ack_w) = cv(io::pipe())?;
    let (go_r, go_w) = cv(io::pipe())?;
    mode(ack_r, true)?;
    mode(go_r, true)?;
    let name = format!(
        "unix813_{}_{}_{}_{}",
        cv(process::getpid())?.raw(),
        arm,
        reverse,
        variant
    );
    let addr = SockAddrUn::abstract_socket(name.as_bytes());
    let pid = match cv(process::fork())? {
        ForkResult::Child => {
            let run = || -> TestResult<()> {
                cv(io::close(ack_r))?;
                cv(io::close(go_w))?;
                let setup = cv(socket::socket(AF_UNIX, SOCK_STREAM, 0))?;
                let writer = if reverse {
                    cv(socket::bind_unix(setup, &addr))?;
                    cv(socket::listen(setup, 1))?;
                    send(ack_w, 1)?;
                    receive(go_r)?;
                    let fd = cv(socket::accept(setup, None))?;
                    cv(io::close(setup))?;
                    fd
                } else {
                    receive(go_r)?;
                    cv(socket::connect_unix(setup, &addr))?;
                    send(ack_w, 1)?;
                    setup
                };
                send(ack_w, writer.raw() as i64)?;
                send(ack_w, cv(process::gettid())?.raw() as i64)?;
                mode(writer, true)?;
                let partial = arm == "partial";
                fill(writer, if partial { C - A } else { C }, 0x83)?;
                if arm == "mode" {
                    check(
                        errno(io::write(writer, &[9])) == -(Errno::EAGAIN as i64),
                        "descriptor mode",
                    )?;
                    clean(writer)?;
                }
                let action = if arm == "signal" && variant == 1 {
                    signal::Sigaction::ignore()
                } else {
                    signal::Sigaction::new(caught_signal)
                };
                cv(signal::sigaction(signal::SIGUSR1, Some(&action), None))?;
                if !partial || variant == 1 {
                    mode(writer, false)?;
                }
                if arm == "poll" {
                    poll_out(writer, false)?;
                }
                send(ack_w, 1)?;
                receive(go_r)?;
                let request = if arm == "poll" { 1 } else { A + 1 };
                if arm == "poll" {
                    poll_out(writer, true)?;
                }
                let got = errno(io::write(writer, &payload(request)));
                let expected = if arm == "peer_close" {
                    -(Errno::EPIPE as i64)
                } else if arm == "signal" && variant == 0 {
                    -(Errno::EINTR as i64)
                } else if arm == "poll" {
                    1
                } else {
                    A as i64
                };
                check(
                    got == expected,
                    &format!("write={} expected={}", got, expected),
                )?;
                clean(writer)?;
                send(ack_w, got)?;
                receive(go_r)?;
                if arm == "signal" && variant == 0 {
                    check(
                        cv(io::write(writer, &[0x7d]))? == 1,
                        "signal recovery write",
                    )?;
                    clean(writer)?;
                    send(ack_w, 1)?;
                    receive(go_r)?;
                }
                if arm == "poll" {
                    let mut fds = [io::PollFd::new(writer, io::poll_events::POLLOUT)];
                    cv(io::poll(&mut fds, 0))?;
                    check(
                        fds[0].revents & io::poll_events::POLLHUP != 0
                            && fds[0].revents & io::poll_events::POLLOUT == 0,
                        "close HUP",
                    )?;
                }
                cv(io::close(writer))
            };
            let result = run();
            if let Err(ref e) = result {
                println!("UNIX_CHILD_FAIL:{}", e);
            }
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };
    cv(io::close(ack_w))?;
    cv(io::close(go_r))?;
    let run = || -> TestResult<usize> {
        let setup = cv(socket::socket(AF_UNIX, SOCK_STREAM, 0))?;
        let peer = if reverse {
            receive(ack_r)?;
            cv(socket::connect_unix(setup, &addr))?;
            send(go_w, 1)?;
            setup
        } else {
            cv(socket::bind_unix(setup, &addr))?;
            cv(socket::listen(setup, 1))?;
            send(go_w, 1)?;
            receive(ack_r)?;
            let fd = cv(socket::accept(setup, None))?;
            cv(io::close(setup))?;
            fd
        };
        let writer = Fd::from_raw(receive(ack_r)? as u64);
        let tid = receive(ack_r)? as u64;
        receive(ack_r)?;
        let mut drained = 0;
        if arm == "poll" {
            check(read_exact(peer, 1)? == [0x83], "poll drain")?;
            drained = 1;
        }
        send(go_w, 1)?;
        if !matches!(arm, "poll" | "partial") {
            wait_witness(peer, writer, tid)?;
            if arm == "peer_close" {
                cv(io::close(peer))?;
            } else {
                if arm == "signal" {
                    cv(signal::kill(pid, signal::SIGUSR1))?;
                    if variant == 1 {
                        let deadline = now() + 40;
                        while now() < deadline {
                            check(
                                matches!(
                                    io::read(ack_r, &mut [0; 8]),
                                    Err(Error::Os(Errno::EAGAIN))
                                ),
                                "ignored signal completed write",
                            )?;
                            cv(process::yield_now())?;
                        }
                        wait_witness(peer, writer, tid)?;
                    }
                }
                if arm != "signal" || variant == 1 {
                    check(read_exact(peer, A)? == vec![0x83; A], "prescribed drain")?;
                    drained = A;
                }
            }
        }
        let got = receive(ack_r)?;
        let bytes = if arm == "peer_close" {
            check(got == -(Errno::EPIPE as i64), "peer EPIPE")?;
            0
        } else if arm == "signal" && variant == 0 {
            check(got == -(Errno::EINTR as i64), "caught EINTR")?;
            check(read_exact(peer, 1)? == [0x83], "recovery drain")?;
            send(go_w, 1)?;
            check(receive(ack_r)? == 1, "recovery count")?;
            let actual = read_exact(peer, C)?;
            check(
                actual[..C - 1] == vec![0x83; C - 1] && actual[C - 1] == 0x7d,
                "recovery wire",
            )?;
            C + 1
        } else {
            let count = if arm == "poll" { 1 } else { A };
            check(got == count as i64, "parent count")?;
            let filled = if arm == "partial" { C - A } else { C };
            let actual = read_exact(peer, filled - drained + count)?;
            check(
                actual[..filled - drained] == vec![0x83; filled - drained],
                "filler corruption",
            )?;
            check(
                actual[filled - drained..]
                    == payload(if arm == "poll" { 1 } else { A + 1 })[..count],
                "wire prefix",
            )?;
            filled + count
        };
        if arm != "peer_close" {
            cv(io::close(peer))?;
        }
        send(go_w, 1)?;
        reap(pid)?;
        Ok(bytes)
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
fn main() {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let mut passed = 0;
    for arm in ARMS {
        println!("[UNIX_WRITE_START:{}:{}]", arch, arm);
        let result = (|| -> TestResult<usize> {
            if arm == "mode" {
                construction_control()?;
            }
            let mut bytes = 0;
            for reverse in [false, true] {
                for variant in 0..if matches!(arm, "signal" | "partial") {
                    2
                } else {
                    1
                } {
                    bytes += connection(arm, reverse, variant)?;
                }
            }
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => {
                passed += 1;
                println!(
                    "[UNIX_WRITE_ORACLE:{}:{}:verdict=PASS:bytes={}]",
                    arch, arm, bytes
                );
            }
            Err(e) => {
                println!("[UNIX_WRITE_ORACLE:{}:{}:verdict=FAIL:{}]", arch, arm, e);
                process::exit(1);
            }
        }
    }
    println!("[UNIX_WRITE_SUMMARY:{}:passed={}:failed=0]", arch, passed);
    process::exit(0);
}
