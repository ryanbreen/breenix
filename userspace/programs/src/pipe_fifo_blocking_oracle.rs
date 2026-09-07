//! #813 pipe/FIFO write oracle. Helper reads are sequenced by real queue witnesses,
//! before the helper releases a writer parked in write().
use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::syscall::{nr, raw::syscall3};
use libbreenix::types::Fd;
use libbreenix::{
    fs, io,
    process::{self, ForkResult},
    signal, time,
};

const C: usize = 65536;
const A: usize = 4096;
const QUERY: u64 = 0xB8130001;
#[cfg(target_arch = "aarch64")]
const FIFO_FIXTURE: u64 = 0xB8130002;
const ARMS: [&str; 15] = [
    "full_block",
    "atomic4096",
    "atomic4095",
    "large_block",
    "nonblock_full",
    "nonblock_atomic",
    "nonblock_large",
    "poll",
    "last_reader",
    "duplicate_reader",
    "signal_before",
    "signal_after",
    "ignored_signal",
    "writev_atomic",
    "no_writer_eof",
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
fn query(fd: Fd, tid: u64) -> TestResult<Witness> {
    let mut witness = Witness {
        tid,
        target_fd: fd.raw(),
        ..Witness::default()
    };
    let result =
        unsafe { syscall3(nr::IOCTL, fd.raw(), QUERY, &mut witness as *mut _ as u64) } as i64;
    check(result == 0, &format!("query={}", result))?;
    check(
        witness.capacity == C as u64 && witness.atomic_limit == A as u64,
        "kernel limits differ",
    )?;
    check(witness.identity != 0, "missing object identity")?;
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
fn object(kind: &str, ordinal: usize) -> TestResult<(Fd, Fd)> {
    let pair = if kind == "pipe" {
        cv(io::pipe())?
    } else {
        let path = format!(
            "/tmp/pipe_fifo_oracle_{}_{}",
            cv(process::getpid())?.raw(),
            ordinal
        );
        #[cfg(target_arch = "x86_64")]
        cv(fs::mkfifo(&path, 0o600))?;
        #[cfg(target_arch = "aarch64")]
        {
            let path_c = format!("{}\0", path);
            let result = unsafe {
                syscall3(
                    nr::IOCTL,
                    Fd::STDOUT.raw(),
                    FIFO_FIXTURE,
                    path_c.as_ptr() as u64,
                )
            } as i64;
            check(result == 0, "FIFO fixture create failed")?;
        }
        check(
            matches!(
                fs::open(&path, fs::O_WRONLY | fs::O_NONBLOCK),
                Err(Error::Os(Errno::ENXIO))
            ),
            "FIFO open without reader must be ENXIO",
        )?;
        let reader = cv(fs::open(&path, fs::O_RDONLY | fs::O_NONBLOCK))?;
        check(
            cv(io::read(reader, &mut [0; 1]))? == 0,
            "FIFO reader before first writer must return EOF",
        )?;
        let writer = cv(fs::open(&path, fs::O_WRONLY | fs::O_NONBLOCK))?;
        (reader, writer)
    };
    mode(pair.0, true)?;
    mode(pair.1, true)?;
    let witness = query(pair.1, cv(process::gettid())?.raw())?;
    check(
        witness.kind == if kind == "pipe" { 1 } else { 2 },
        "wrong descriptor kind",
    )?;
    Ok(pair)
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
fn verify(fd: Fd, filler: usize, byte: u8, data: &[u8]) -> TestResult<usize> {
    let actual = read_exact(fd, filler + data.len())?;
    check(
        actual[..filler].iter().all(|value| *value == byte),
        "filler corruption",
    )?;
    check(
        actual[filler..] == *data,
        "payload mismatch/duplicate prefix",
    )?;
    check(
        matches!(io::read(fd, &mut [0; 1]), Err(Error::Os(Errno::EAGAIN))),
        "unexpected trailing bytes",
    )?;
    Ok(actual.len())
}
fn wait_witness(fd: Fd, tid: u64, occupancy: usize) -> TestResult<Witness> {
    let deadline = now() + 2000;
    loop {
        let witness = query(fd, tid)?;
        if witness.queued == 1 && witness.blocked == 1 && witness.blocked_in_syscall == 1 {
            check(
                witness.occupancy == occupancy as u64,
                "pre-drain occupancy/atomic prefix",
            )?;
            return Ok(witness);
        }
        check(now() < deadline, "no genuine writer wait witness")?;
        cv(process::yield_now())?;
    }
}
fn stays_waiting(fd: Fd, tid: u64, occupancy: usize) -> TestResult<()> {
    let deadline = now() + 40;
    while now() < deadline {
        let witness = query(fd, tid)?;
        check(
            witness.queued == 1
                && witness.blocked == 1
                && witness.blocked_in_syscall == 1
                && witness.occupancy == occupancy as u64,
            "writer escaped without progress",
        )?;
        cv(process::yield_now())?;
    }
    Ok(())
}
// Signal delivery can wake BlockedOnIO even for SIG_IGN. Observe the syscall
// result channel throughout that wake/recheck/repark interval; a transient
// scheduler state is not a userspace write completion.
fn ignored_stays_blocked(fd: Fd, tid: u64, occupancy: usize, ack: Fd) -> TestResult<()> {
    let deadline = now() + 40;
    while now() < deadline {
        check(
            matches!(io::read(ack, &mut [0; 8]), Err(Error::Os(Errno::EAGAIN))),
            "ignored signal returned before drain",
        )?;
        check(
            query(fd, tid)?.occupancy == occupancy as u64,
            "ignored signal changed occupancy before drain",
        )?;
        cv(process::yield_now())?;
    }
    wait_witness(fd, tid, occupancy)?;
    check(
        matches!(io::read(ack, &mut [0; 8]), Err(Error::Os(Errno::EAGAIN))),
        "ignored signal returned instead of reparking",
    )
}

extern "C" fn caught_signal(_: i32) {}

fn controls(reader: Fd, writer: Fd, arm: &str, byte: u8) -> TestResult<usize> {
    let (filled, request, expected) = match arm {
        "nonblock_full" => (C, 1, -(Errno::EAGAIN as i64)),
        "nonblock_atomic" => (C - (A - 1), A, -(Errno::EAGAIN as i64)),
        "nonblock_large" => (C - A, 2 * A, A as i64),
        "poll" => (C, 1, 1),
        _ => return Err("unknown control".into()),
    };
    fill(writer, filled, byte)?;
    let data = payload(request);
    let mut drained = 0;
    if arm == "poll" {
        let mut polls = [io::PollFd::new(writer, io::poll_events::POLLOUT)];
        cv(io::poll(&mut polls, 0))?;
        check(
            polls[0].revents & io::poll_events::POLLOUT == 0,
            "full POLLOUT",
        )?;
        check(read_exact(reader, 1)? == [byte], "poll drain byte")?;
        drained = 1;
        cv(io::poll(&mut polls, 0))?;
        check(
            polls[0].revents & io::poll_events::POLLOUT != 0,
            "one-byte POLLOUT missing",
        )?;
    }
    let got = errno(io::write(writer, &data));
    check(
        got == expected,
        &format!("return={} expected={}", got, expected),
    )?;
    let transferred = got.max(0) as usize;
    let witness = query(writer, cv(process::gettid())?.raw())?;
    check(
        witness.queued == 0
            && witness.blocked_in_syscall == 0
            && witness.occupancy == (filled - drained + transferred) as u64,
        "nonblocking state or occupancy",
    )?;
    if arm == "poll" {
        let mut polls = [io::PollFd::new(writer, io::poll_events::POLLOUT)];
        cv(io::poll(&mut polls, 0))?;
        check(
            polls[0].revents & io::poll_events::POLLOUT == 0,
            "readiness did not clear",
        )?;
    }
    let bytes = verify(reader, filled - drained, byte, &data[..transferred])? + drained;
    cv(io::close(reader))?;
    if arm == "poll" {
        let mut polls = [io::PollFd::new(writer, io::poll_events::POLLOUT)];
        cv(io::poll(&mut polls, 0))?;
        check(
            polls[0].revents & io::poll_events::POLLERR != 0
                && polls[0].revents & io::poll_events::POLLOUT == 0,
            "no-reader poll mismatch",
        )?;
    }
    Ok(bytes)
}

fn blocking(reader: Fd, writer: Fd, arm: &str, byte: u8) -> TestResult<usize> {
    let (filled, request, drain, expected, occupancy) = match arm {
        "full_block" | "ignored_signal" => (C, A, A, A as i64, C),
        "atomic4096" => (C - (A - 1), A, 1, A as i64, C - (A - 1)),
        "atomic4095" => (C - (A - 2), A - 1, 1, (A - 1) as i64, C - (A - 2)),
        "large_block" => (C - A, 2 * A, A, (2 * A) as i64, C),
        "last_reader" | "duplicate_reader" => (C, A, 0, -(Errno::EPIPE as i64), C),
        "signal_before" => (C, A, 0, -(Errno::EINTR as i64), C),
        "signal_after" => (C - A, 2 * A, 0, A as i64, C),
        _ => return Err("unknown blocking arm".into()),
    };
    let action = if arm == "ignored_signal" {
        signal::Sigaction::ignore()
    } else {
        signal::Sigaction::new(caught_signal)
    };
    cv(signal::sigaction(signal::SIGUSR1, Some(&action), None))?;
    fill(writer, filled, byte)?;
    mode(writer, false)?;
    let (ack_r, ack_w) = cv(io::pipe())?;
    let (release_r, release_w) = cv(io::pipe())?;
    mode(ack_r, true)?;
    mode(release_r, true)?;
    let data = payload(request);
    let pid = match cv(process::fork())? {
        ForkResult::Child => {
            let run = || -> TestResult<()> {
                cv(io::close(reader))?;
                cv(io::close(ack_r))?;
                cv(io::close(release_w))?;
                send(ack_w, cv(process::gettid())?.raw() as i64)?;
                // Exactly one victim syscall: EAGAIN is recorded as a failed blocking write.
                let result = errno(io::write(writer, &data));
                send(ack_w, result)?;
                receive(release_r)?;
                if arm == "signal_before" {
                    send(ack_w, errno(io::write(writer, &[0x7d])))?;
                    receive(release_r)?;
                }
                check(result == expected, "victim return")
            };
            process::exit(if run().is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };
    cv(io::close(ack_w))?;
    cv(io::close(release_r))?;
    let run = || -> TestResult<usize> {
        let tid = receive(ack_r)? as u64;
        let observed = wait_witness(writer, tid, occupancy)?;
        let closes = arm == "last_reader" || arm == "duplicate_reader";
        if arm == "duplicate_reader" {
            let duplicate = cv(io::dup(reader))?;
            cv(io::close(reader))?;
            stays_waiting(writer, tid, occupancy)?;
            cv(io::close(duplicate))?;
        } else if closes {
            cv(io::close(reader))?;
        } else if arm.starts_with("signal_") || arm == "ignored_signal" {
            cv(signal::kill(pid, signal::SIGUSR1))?;
            if arm == "ignored_signal" {
                ignored_stays_blocked(writer, tid, occupancy, ack_r)?;
            }
        }
        if drain != 0 {
            check(
                read_exact(reader, drain)? == vec![byte; drain],
                "prescribed drain mismatch",
            )?;
        }
        // Stop draining until the writer acknowledges the actual return count.
        let got = receive(ack_r)?;
        check(
            got == expected,
            &format!("return={} expected={}", got, expected),
        )?;
        let finished = query(writer, tid)?;
        check(
            finished.identity == observed.identity
                && finished.queued == 0
                && finished.blocked == 0
                && finished.blocked_in_syscall == 0,
            "residual waiter or blocked flag",
        )?;
        let bytes = if closes {
            0
        } else {
            let transferred = got.max(0) as usize;
            if arm == "signal_before" {
                check(read_exact(reader, 1)? == [byte], "recovery drain")?;
                send(release_w, 1)?;
                check(receive(ack_r)? == 1, "post-EINTR write did not recover")?;
                verify(reader, filled - 1, byte, &[0x7d])? + 1
            } else {
                verify(reader, filled - drain, byte, &data[..transferred])? + drain
            }
        };
        send(release_w, 1)?;
        reap(pid)?;
        if !closes {
            cv(io::close(reader))?;
        }
        Ok(bytes)
    };
    let result = run();
    if result.is_err() {
        let _ = signal::kill(pid, signal::SIGKILL);
        let _ = reap(pid);
    }
    cv(io::close(ack_r))?;
    cv(io::close(release_w))?;
    result
}
#[repr(C)]
struct IoVec {
    base: u64,
    len: u64,
}
fn writev_atomic(reader: Fd, writer: Fd, byte: u8) -> TestResult<usize> {
    let filled = C - A / 2;
    fill(writer, filled, byte)?;
    mode(writer, false)?;
    let mut children = Vec::new();
    for tag in [0x31u8, 0x52u8] {
        let (ack_r, ack_w) = cv(io::pipe())?;
        let (release_r, release_w) = cv(io::pipe())?;
        mode(ack_r, true)?;
        mode(release_r, true)?;
        let pid = match cv(process::fork())? {
            ForkResult::Child => {
                let run = || -> TestResult<()> {
                    cv(io::close(reader))?;
                    let data = vec![tag; A];
                    let vectors = [
                        IoVec {
                            base: data.as_ptr() as u64,
                            len: (A / 2) as u64,
                        },
                        IoVec {
                            base: unsafe { data.as_ptr().add(A / 2) } as u64,
                            len: (A / 2) as u64,
                        },
                    ];
                    send(ack_w, cv(process::gettid())?.raw() as i64)?;
                    let result =
                        unsafe { syscall3(nr::WRITEV, writer.raw(), vectors.as_ptr() as u64, 2) }
                            as i64;
                    send(ack_w, result)?;
                    receive(release_r)?;
                    check(result == A as i64, "writev aggregate result")
                };
                process::exit(if run().is_ok() { 0 } else { 1 });
            }
            ForkResult::Parent(pid) => pid.raw() as i32,
        };
        cv(io::close(ack_w))?;
        cv(io::close(release_r))?;
        let tid = receive(ack_r)? as u64;
        children.push((pid, tid, ack_r, release_w));
    }
    let run = || -> TestResult<usize> {
        // Half a record fits, but neither aggregate may copy any prefix.
        for &(_, tid, _, _) in &children {
            wait_witness(writer, tid, filled)?;
        }
        check(
            read_exact(reader, filled)? == vec![byte; filled],
            "writev filler",
        )?;
        for &(_, tid, ack, _) in &children {
            check(receive(ack)? == A as i64, "writev short/error return")?;
            let state = query(writer, tid)?;
            check(
                state.queued == 0 && state.blocked_in_syscall == 0,
                "writev residual waiter",
            )?;
        }
        let data = read_exact(reader, 2 * A)?;
        let tags: Vec<u8> = data.chunks_exact(A).map(|record| record[0]).collect();
        check(
            tags == [0x31, 0x52] || tags == [0x52, 0x31],
            "writev lost/duplicated record",
        )?;
        check(
            data.chunks_exact(A)
                .all(|record| record.iter().all(|b| *b == record[0])),
            "writev records interleaved",
        )?;
        check(
            matches!(io::read(reader, &mut [0; 1]), Err(Error::Os(Errno::EAGAIN))),
            "writev trailing data",
        )?;
        for &(pid, _, _, release) in &children {
            send(release, 1)?;
            reap(pid)?;
        }
        Ok(filled + 2 * A)
    };
    let result = run();
    for &(pid, _, ack, release) in &children {
        if result.is_err() {
            let _ = signal::kill(pid, signal::SIGKILL);
            let _ = reap(pid);
        }
        cv(io::close(ack))?;
        cv(io::close(release))?;
    }
    cv(io::close(reader))?;
    result
}
fn expected_bytes(arm: &str) -> usize {
    match arm {
        "full_block" | "ignored_signal" | "large_block" => C + A,
        "atomic4096" | "atomic4095" | "signal_before" | "poll" => C + 1,
        "nonblock_full" | "nonblock_large" | "signal_after" => C,
        "nonblock_atomic" => C - (A - 1),
        "writev_atomic" => C + 3 * A / 2,
        "no_writer_eof" => 0,
        "last_reader" | "duplicate_reader" => 0,
        _ => panic!("unknown arm"),
    }
}
fn emit(line: &str) {
    println!("{}", line);
    println!("{}", line);
}
fn main() {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let mut passed = 0;
    let mut failed = 0;
    for kind in ["pipe", "fifo"] {
        for (ordinal, arm) in ARMS.iter().enumerate() {
            let outcome = (|| {
                let (reader, writer) = object(kind, ordinal)?;
                let byte = 0x80 + ordinal as u8;
                let result = if *arm == "no_writer_eof" {
                    // FIFO setup above checked EOF before any writer opened.
                    // Also check the last-writer EOF edge for both kinds.
                    cv(io::close(writer))?;
                    check(cv(io::read(reader, &mut [0; 1]))? == 0, "no-writer EOF")?;
                    cv(io::close(reader))?;
                    Ok(0)
                } else if *arm == "writev_atomic" {
                    writev_atomic(reader, writer, byte)
                } else if arm.starts_with("nonblock_") || *arm == "poll" {
                    controls(reader, writer, arm, byte)
                } else {
                    blocking(reader, writer, arm, byte)
                };
                if *arm != "no_writer_eof" {
                    cv(io::close(writer))?;
                }
                let bytes = result?;
                check(bytes == expected_bytes(arm), "incomplete byte tally")?;
                Ok::<usize, String>(bytes)
            })();
            match outcome {
                Ok(bytes) => {
                    passed += 1;
                    emit(&format!(
                        "[PIPE_WRITE_ORACLE:{}:{}:{}:verdict=PASS:bytes={}:expected={}]",
                        arch,
                        kind,
                        arm,
                        bytes,
                        expected_bytes(arm)
                    ));
                }
                Err(detail) => {
                    failed += 1;
                    emit(&format!(
                        "[PIPE_WRITE_ORACLE:{}:{}:{}:verdict=FAIL:{}]",
                        arch, kind, arm, detail
                    ));
                    // Failed setup can retain descriptors: do not pretend later arms are independent.
                    break;
                }
            }
        }
    }
    emit(&format!(
        "[PIPE_WRITE_SUMMARY:{}:passed={}:failed={}]",
        arch, passed, failed
    ));
    process::exit(if failed == 0 && passed == 2 * ARMS.len() {
        0
    } else {
        1
    });
}
