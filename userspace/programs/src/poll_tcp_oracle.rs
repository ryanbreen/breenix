//! #568 oracle: a blocking `poll()` on a connected TCP socket must block, wake,
//! and return — and must not wedge the guest.
//!
//! #568 was filed against `sys_poll` blocking on an `FdKind::TcpConnection` and
//! was worked around at both ends rather than fixed: `libbreenix`'s receive
//! deadline switched to `O_NONBLOCK` plus a sleeping retry (see the #568 comment
//! in `socket.rs`), and `poll_test` only ever polls a TCP *listener*, and only
//! with `timeout=0`. The result was that no program in any boot profile drove a
//! blocking poll on a connected TCP fd, so the path the issue is about was never
//! executed again. This oracle is that missing caller, and it runs on every boot.
//!
//! Three stages, each a distinct failure mode:
//!
//!   stage 1  `idle`   — block on a connected fd with nothing to read and a
//!                       finite timeout. Must return 0 having actually slept.
//!                       A wedge hangs here; a vacuous instant return (the
//!                       blocking path never entered) is also a FAIL, because a
//!                       poll that does not block cannot prove anything about
//!                       blocking.
//!   stage 2  `armed`  — data is already pending when poll is called. Must
//!                       report POLLIN from the entry scan.
//!   stage 3  `late`   — THE LOST-WAKE STAGE. The poller enters `poll()` first
//!                       and a forked peer writes only afterwards, so readiness
//!                       is published while the poller is already blocked. This
//!                       is the one stage a stale-snapshot or edge-triggered
//!                       wake cannot pass by luck: a poll that misses the
//!                       publication either returns 0 at the timeout (lost wake)
//!                       or returns POLLIN only after the full timeout elapsed
//!                       (woken by the clock, not by the data). Both are FAIL.
//!
//! Do not weaken any stage to `timeout=0`. Stage 3's two elapsed bounds are both
//! derived from this test's own parameters -- it must have blocked at least
//! until the peer wrote, and it must not have consumed its whole timeout -- so
//! neither is a constant that can be retuned to make a failure pass.

use std::string::String;

use libbreenix::io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLNVAL};
use libbreenix::io::{self, PollFd};
use libbreenix::process::{self, ForkResult};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time;
use libbreenix::types::Fd;

/// Loopback port for the oracle's own connection. Distinct from poll_test's
/// 9091 and from every service port init starts.
const ORACLE_PORT: u16 = 9568;

/// Stage 1 timeout. Long enough that a real block is unambiguous -- the poll
/// fast path returns in microseconds, so this separates the two by four orders
/// of magnitude. Kept small because init waits on this oracle and the aarch64
/// boot gate's marker window is a fixed host-side deadline.
const IDLE_TIMEOUT_MS: i32 = 120;
/// Stage 1 must sleep at least this long. Guards against a vacuous pass in
/// which poll returns immediately and never enters the blocking path at all.
const IDLE_MIN_ELAPSED_MS: u64 = 80;

/// Stage 3 timeout, and the delay before the peer writes.
const LATE_TIMEOUT_MS: i32 = 5_000;
const LATE_WRITE_DELAY_MS: u64 = 80;

const PAYLOAD_ARMED: &[u8] = b"armed568";
const PAYLOAD_LATE: &[u8] = b"late568";

struct Failure {
    stage: &'static str,
    detail: String,
}

fn fail(stage: &'static str, detail: String) -> Failure {
    Failure { stage, detail }
}

fn monotonic_ms() -> u64 {
    match time::now_monotonic() {
        Ok(ts) => (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000,
        Err(_) => 0,
    }
}

fn emit(line: &str) {
    // Emitted twice: console output interleaves at byte granularity, so a
    // single shredded copy must not be able to hide the verdict.
    print!("{}\n", line);
    print!("{}\n", line);
}

/// Establish a loopback TCP connection and return `(listener, client, server)`.
fn connect_loopback() -> Result<(Fd, Fd, Fd), Failure> {
    let listener = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => return Err(fail("listen_socket", format!("{}", e))),
    };
    let bind_addr = SockAddrIn::new([0, 0, 0, 0], ORACLE_PORT);
    if let Err(e) = socket::bind_inet(listener, &bind_addr) {
        return Err(fail("bind", format!("{}", e)));
    }
    if let Err(e) = socket::listen(listener, 4) {
        return Err(fail("listen", format!("{}", e)));
    }

    let client = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => return Err(fail("client_socket", format!("{}", e))),
    };
    let peer_addr = SockAddrIn::new([127, 0, 0, 1], ORACLE_PORT);
    if let Err(e) = socket::connect_inet(client, &peer_addr) {
        return Err(fail("connect", format!("{}", e)));
    }

    let server = match socket::accept(listener, None) {
        Ok(fd) => fd,
        Err(e) => return Err(fail("accept", format!("{}", e))),
    };

    Ok((listener, client, server))
}

/// Reject the revents bits that mean the connection died rather than became
/// readable. Without this a torn-down connection would satisfy "poll returned".
fn reject_broken(stage: &'static str, revents: i16) -> Result<(), Failure> {
    if revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
        return Err(fail(stage, format!("revents={:#06x}", revents)));
    }
    Ok(())
}

fn stage_idle(client: Fd) -> Result<u64, Failure> {
    let mut fds = [PollFd::new(client, POLLIN)];
    let started = monotonic_ms();
    let ready = match io::poll(&mut fds, IDLE_TIMEOUT_MS) {
        Ok(n) => n,
        Err(e) => return Err(fail("idle_poll", format!("{}", e))),
    };
    let elapsed = monotonic_ms().saturating_sub(started);

    if ready != 0 {
        return Err(fail(
            "idle_ready",
            format!("ready={} revents={:#06x}", ready, fds[0].revents),
        ));
    }
    reject_broken("idle_broken", fds[0].revents)?;
    if elapsed < IDLE_MIN_ELAPSED_MS {
        return Err(fail(
            "idle_vacuous",
            format!("elapsed_ms={} min={}", elapsed, IDLE_MIN_ELAPSED_MS),
        ));
    }
    Ok(elapsed)
}

fn stage_armed(client: Fd, server: Fd) -> Result<(), Failure> {
    if let Err(e) = socket::send(server, PAYLOAD_ARMED) {
        return Err(fail("armed_send", format!("{}", e)));
    }

    let mut fds = [PollFd::new(client, POLLIN)];
    let ready = match io::poll(&mut fds, LATE_TIMEOUT_MS) {
        Ok(n) => n,
        Err(e) => return Err(fail("armed_poll", format!("{}", e))),
    };
    if ready != 1 || fds[0].revents & POLLIN == 0 {
        return Err(fail(
            "armed_not_ready",
            format!("ready={} revents={:#06x}", ready, fds[0].revents),
        ));
    }

    let mut buf = [0u8; 64];
    match socket::recv(client, &mut buf) {
        Ok(n) if &buf[..n] == PAYLOAD_ARMED => Ok(()),
        Ok(n) => Err(fail("armed_payload", format!("n={}", n))),
        Err(e) => Err(fail("armed_recv", format!("{}", e))),
    }
}

/// The lost-wake stage: readiness is published while the poller is already
/// parked inside `poll()`.
fn stage_late(client: Fd, server: Fd) -> Result<u64, Failure> {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Give the parent time to reach the blocking path before the write.
            let _ = time::sleep_ms(LATE_WRITE_DELAY_MS);
            let _ = socket::send(server, PAYLOAD_LATE);
            process::exit(0);
        }
        Ok(ForkResult::Parent(_)) => {}
        Err(e) => return Err(fail("late_fork", format!("{}", e))),
    }

    let mut fds = [PollFd::new(client, POLLIN)];
    let started = monotonic_ms();
    let ready = match io::poll(&mut fds, LATE_TIMEOUT_MS) {
        Ok(n) => n,
        Err(e) => return Err(fail("late_poll", format!("{}", e))),
    };
    let elapsed = monotonic_ms().saturating_sub(started);

    if ready != 1 || fds[0].revents & POLLIN == 0 {
        return Err(fail(
            "late_lost_wake",
            format!(
                "ready={} revents={:#06x} elapsed_ms={}",
                ready, fds[0].revents, elapsed
            ),
        ));
    }
    reject_broken("late_broken", fds[0].revents)?;

    // Both bounds come from this test's own parameters, never from a constant
    // tuned to one host: emulated x86 runs this stage 30x slower than native
    // AArch64, so any hand-picked millisecond ceiling is either vacuous on one
    // platform or a false red on the other.
    //
    // Lower bound (anti-vacuity): the poller must still have been blocked when
    // the peer wrote. If the child's write beat the parent into poll(), the
    // entry scan satisfies the poll and stage 3 silently degrades into a second
    // copy of stage 2 -- it would pass while testing nothing about wakes.
    if elapsed < LATE_WRITE_DELAY_MS {
        return Err(fail(
            "late_vacuous",
            format!("elapsed_ms={} write_delay={}", elapsed, LATE_WRITE_DELAY_MS),
        ));
    }
    // Upper bound: a poll that consumed its whole timeout was ended by the
    // clock, whatever its final scan then happened to find. That is the lost
    // wake wearing a passing revents.
    if elapsed >= LATE_TIMEOUT_MS as u64 {
        return Err(fail(
            "late_woken_by_clock",
            format!("elapsed_ms={} timeout={}", elapsed, LATE_TIMEOUT_MS),
        ));
    }

    let mut buf = [0u8; 64];
    match socket::recv(client, &mut buf) {
        Ok(n) if &buf[..n] == PAYLOAD_LATE => Ok(elapsed),
        Ok(n) => Err(fail("late_payload", format!("n={}", n))),
        Err(e) => Err(fail("late_recv", format!("{}", e))),
    }
}

fn run() -> Result<(u64, u64), Failure> {
    let (listener, client, server) = connect_loopback()?;

    let idle_ms = stage_idle(client)?;
    stage_armed(client, server)?;
    let late_ms = stage_late(client, server)?;

    let _ = io::close(server);
    let _ = io::close(client);
    let _ = io::close(listener);

    Ok((idle_ms, late_ms))
}

fn main() {
    match run() {
        Ok((idle_ms, late_ms)) => {
            emit(&format!(
                "[POLL_TCP_ORACLE:PASS:stages=3:idle_ms={}:late_ms={}]",
                idle_ms, late_ms
            ));
            process::exit(0);
        }
        Err(f) => {
            emit(&format!("[POLL_TCP_ORACLE:FAIL:{}:{}]", f.stage, f.detail));
            process::exit(1);
        }
    }
}
