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
//! Do not weaken any stage to `timeout=0`.
//!
//! Stage 3's bounds are derived from an *absolute instant both processes name*,
//! never from either side's own stopwatch. The parent computes `write_deadline`
//! on the kernel monotonic clock BEFORE forking; the child inherits it and
//! sleeps *until* it rather than sleeping a duration from whenever it happens to
//! be scheduled. The parent then proves two facts from its own two timestamps:
//!
//!   entry  + ENTRY_GUARD_MS <= write_deadline   (it was inside poll() before
//!                                                the peer's write was due)
//!   return                   >= write_deadline   (the poll did not return
//!                                                before that write was due)
//!
//! Both hold with zero timing slack on a correct kernel, because the only thing
//! that can make this poll return POLLIN is the child's send, and that send is
//! at or after `write_deadline` by construction. The first is a *precondition*,
//! not a verdict: if the parent was descheduled long enough to miss its own
//! window the trial proves nothing, so it is drained and retried with a wider
//! margin, and only an exhausted retry budget is a FAIL. An earlier revision
//! compared the parent's elapsed against the child's sleep *duration*; because
//! the child's clock starts at the fork and the parent's at its poll entry, that
//! bound sat on top of its own distribution and reddened ~8% of healthy boots.

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
/// Stage 1 must sleep at least this long. Derived, not hand-picked: a poll that
/// returns 0 returned because its timeout expired, so it must have consumed the
/// whole timeout. The single millisecond of give is the truncation of the two
/// `monotonic_ms()` stamps, nothing else. This is strictly stronger than the
/// hand-picked 80 it replaces, and it cannot be retuned without changing the
/// timeout the stage actually asks for.
const IDLE_MIN_ELAPSED_MS: u64 = IDLE_TIMEOUT_MS as u64 - 1;

/// Stage 3 timeout, and how far ahead of the fork the shared write deadline is
/// placed. The deadline is absolute; the child sleeps until it, so the write
/// instant does not drift with the child's scheduling.
const LATE_TIMEOUT_MS: i32 = 5_000;
const LATE_WRITE_DELAY_MS: u64 = 80;
/// Margin the parent must have left when it stamps its poll entry, in the same
/// truncated milliseconds as the two stamps it separates. Two milliseconds
/// covers the truncation of both, so `entry + guard <= deadline` is a real
/// ordering fact and not an artifact of the clock's resolution.
const ENTRY_GUARD_MS: u64 = 2;
/// A trial in which the parent missed its own window proves nothing about
/// wakes, so it is retried rather than reported. Each retry widens the deadline,
/// so exhausting this budget means the parent could not get from `fork()` to
/// `poll()` inside 80, 160, 240 and 320 ms in turn -- a real pathology, and the
/// only thing that turns a missed window into a FAIL.
const LATE_MAX_ATTEMPTS: u64 = 4;

const PAYLOAD_ARMED: &[u8] = b"armed568";
const PAYLOAD_LATE: &[u8] = b"late568";

struct Failure {
    stage: &'static str,
    detail: String,
}

fn fail(stage: &'static str, detail: String) -> Failure {
    Failure { stage, detail }
}

/// The monotonic clock in milliseconds, or a `Failure` naming the stage that
/// asked. A clock error used to be folded onto `0`, which is a legal reading --
/// a kernel fault then arrived disguised as a test-timing artifact.
fn monotonic_ms(stage: &'static str) -> Result<u64, Failure> {
    match time::now_monotonic() {
        Ok(ts) => Ok((ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000),
        Err(e) => Err(fail("clock_error", format!("at={} err={}", stage, e))),
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
    let started = monotonic_ms("idle_entry")?;
    let ready = match io::poll(&mut fds, IDLE_TIMEOUT_MS) {
        Ok(n) => n,
        Err(e) => return Err(fail("idle_poll", format!("{}", e))),
    };
    let elapsed = monotonic_ms("idle_return")?.saturating_sub(started);

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

/// Sleep until an absolute instant on the kernel monotonic clock. The child
/// uses this rather than sleeping a duration, so the write lands at the instant
/// the parent named before the fork no matter when the child is first scheduled.
fn sleep_until(deadline_ms: u64) -> Result<(), Failure> {
    loop {
        let now = monotonic_ms("child_sleep")?;
        if now >= deadline_ms {
            return Ok(());
        }
        let _ = time::sleep_ms(deadline_ms - now);
    }
}

/// Consume a trial whose precondition failed: the peer's write is still coming,
/// so wait for it and read it, leaving the connection as empty as it was.
fn drain_late_payload(client: Fd) -> Result<(), Failure> {
    let mut fds = [PollFd::new(client, POLLIN)];
    match io::poll(&mut fds, LATE_TIMEOUT_MS) {
        Ok(1) if fds[0].revents & POLLIN != 0 => {}
        Ok(n) => {
            return Err(fail(
                "late_drain_stuck",
                format!("ready={} revents={:#06x}", n, fds[0].revents),
            ))
        }
        Err(e) => return Err(fail("late_drain_poll", format!("{}", e))),
    }
    let mut buf = [0u8; 64];
    match socket::recv(client, &mut buf) {
        Ok(_) => Ok(()),
        Err(e) => Err(fail("late_drain_recv", format!("{}", e))),
    }
}

/// The lost-wake stage: readiness is published while the poller is already
/// parked inside `poll()`.
///
/// Returns `(elapsed_ms, park_ms, attempts)`, where `park_ms` is the span the
/// poll is *proven* to have spent blocked before the peer's write was due.
fn stage_late(client: Fd, server: Fd) -> Result<(u64, u64, u64), Failure> {
    let mut attempt: u64 = 1;
    loop {
        // The deadline is absolute and is computed BEFORE the fork, so both
        // processes name the same instant on the same clock. Widening it per
        // attempt is what makes a missed window self-correcting.
        let write_deadline =
            monotonic_ms("late_setup")? + LATE_WRITE_DELAY_MS * attempt;

        let child = match process::fork() {
            Ok(ForkResult::Child) => {
                if sleep_until(write_deadline).is_ok() {
                    let _ = socket::send(server, PAYLOAD_LATE);
                }
                process::exit(0);
            }
            Ok(ForkResult::Parent(pid)) => pid,
            Err(e) => return Err(fail("late_fork", format!("{}", e))),
        };

        let entry = monotonic_ms("late_entry")?;
        if entry + ENTRY_GUARD_MS > write_deadline {
            // The parent did not reach its own poll entry before the peer's
            // write was due. Nothing about wakes can be concluded from this
            // trial either way, so it is not a verdict: drain the write the
            // child is still going to make, reap it, and try again with a wider
            // margin. Reporting this as a failure is the bug the previous
            // revision had -- it is ordinary parent-side scheduling latency.
            drain_late_payload(client)?;
            let mut status = 0i32;
            let _ = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);
            attempt += 1;
            if attempt > LATE_MAX_ATTEMPTS {
                return Err(fail(
                    "late_parent_never_parked",
                    format!(
                        "attempts={} guard_ms={} last_slack_ms={}",
                        LATE_MAX_ATTEMPTS,
                        ENTRY_GUARD_MS,
                        write_deadline.saturating_sub(entry)
                    ),
                ));
            }
            continue;
        }

        let mut fds = [PollFd::new(client, POLLIN)];
        let ready = match io::poll(&mut fds, LATE_TIMEOUT_MS) {
            Ok(n) => n,
            Err(e) => return Err(fail("late_poll", format!("{}", e))),
        };
        let returned = monotonic_ms("late_return")?;
        let elapsed = returned.saturating_sub(entry);
        let park_ms = write_deadline - entry;

        let mut status = 0i32;
        let _ = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);

        if ready != 1 || fds[0].revents & POLLIN == 0 {
            return Err(fail(
                "late_lost_wake",
                format!(
                    "ready={} revents={:#06x} elapsed_ms={} park_ms={}",
                    ready, fds[0].revents, elapsed, park_ms
                ),
            ));
        }
        reject_broken("late_broken", fds[0].revents)?;

        // Anti-vacuity, derived from the shared deadline rather than from either
        // side's stopwatch: the only thing that can make this poll report POLLIN
        // is the child's send, and that send is at or after `write_deadline` by
        // construction, so a correct kernel cannot return earlier than it. A
        // return before the deadline means the poll saw readiness that the peer
        // had not published yet -- stale state, not a wake. There is no timing
        // slack in this bound and nothing in it to retune.
        if returned < write_deadline {
            return Err(fail(
                "late_early_ready",
                format!(
                    "returned={} deadline={} entry={} elapsed_ms={}",
                    returned, write_deadline, entry, elapsed
                ),
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
        return match socket::recv(client, &mut buf) {
            Ok(n) if &buf[..n] == PAYLOAD_LATE => Ok((elapsed, park_ms, attempt)),
            Ok(n) => Err(fail("late_payload", format!("n={}", n))),
            Err(e) => Err(fail("late_recv", format!("{}", e))),
        };
    }
}

fn run() -> Result<(u64, u64, u64, u64), Failure> {
    let (listener, client, server) = connect_loopback()?;

    let idle_ms = stage_idle(client)?;
    stage_armed(client, server)?;
    let (late_ms, park_ms, attempts) = stage_late(client, server)?;

    let _ = io::close(server);
    let _ = io::close(client);
    let _ = io::close(listener);

    Ok((idle_ms, late_ms, park_ms, attempts))
}

fn main() {
    match run() {
        Ok((idle_ms, late_ms, park_ms, attempts)) => {
            emit(&format!(
                "[POLL_TCP_ORACLE:PASS:stages=3:idle_ms={}:late_ms={}:park_ms={}:attempts={}]",
                idle_ms, late_ms, park_ms, attempts
            ));
            process::exit(0);
        }
        Err(f) => {
            emit(&format!("[POLL_TCP_ORACLE:FAIL:{}:{}]", f.stage, f.detail));
            process::exit(1);
        }
    }
}
