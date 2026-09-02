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
//! Stage 3's bounds are derived from instants the PEER measures and reports,
//! never from either side's guess about the other's timing. The child stamps
//! `monotonic_ms()` when it receives the readiness token and again immediately
//! before its send, and ships both stamps with the payload. The parent decodes
//! them and proves three facts:
//!
//!   entry    <= token_ms              (the parent had already committed to
//!                                      this poll before the peer's clock even
//!                                      started)
//!   write_ms >= token_ms + delay_ms   (the peer really waited: it sleeps until
//!                                      that instant on its own clock, so a
//!                                      short wait is the kernel's fault and is
//!                                      reported as one)
//!   return   >= write_ms              (the poll did not return before the peer
//!                                      wrote)
//!
//! Together they put the whole of the peer's delay strictly inside the poll:
//! `park_ms = write_ms - entry` is at least `delay_ms`, and it is REPORTED, so
//! the evidence shows the separation instead of asserting it. A return before
//! `write_ms` means the poll saw readiness the peer had not published -- stale
//! state, not a wake -- and is a named FAIL.
//!
//! The middle fact is what stops the stage from silently degenerating into a
//! second copy of stage 2. An earlier revision proved only `entry <= write_ms`;
//! a mutation that had the peer write immediately then PASSED it, reporting
//! `park_ms=0`, because the two stamps landed in the same truncated
//! millisecond. A bound a degenerate run satisfies is not a bound.
//!
//! Two design points keep this from becoming a flake generator, which is
//! exactly what two earlier revisions of this bound were:
//!
//!   * The `fork()` is OUTSIDE the timing window. The parent forks, then hands
//!     the child a one-byte readiness token over the *reverse* direction of the
//!     same connection, and only then stamps `entry` and polls. The child waits
//!     on that token before starting its delay, so process creation -- which on
//!     a loaded emulated x86 boot costs hundreds of milliseconds, far more than
//!     the delay itself -- is spent before either clock starts. The first
//!     revision compared the parent's elapsed against the child's sleep
//!     *duration* and reddened ~8% of healthy aarch64 boots; the second moved
//!     to a shared absolute deadline but still paid the fork inside the window
//!     and could not reach its own poll in time on x86 at all.
//!   * `entry <= token_ms` is a PRECONDITION, not a verdict. If the parent was
//!     descheduled between handing over the token and reaching `poll()`, the
//!     trial proves nothing either way, so its payload is consumed, the child
//!     reaped, and the delay widened. Only an exhausted budget is a FAIL, and it
//!     reports the measured overshoot so the failure is legible.

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

/// Stage 3 timeout, and how long the child waits after the readiness token
/// before it writes. The delay is a duration on the child's side because the
/// child measures and reports the instant it actually wrote -- nothing is
/// predicted, so nothing has to be guessed accurately.
const LATE_TIMEOUT_MS: i32 = 5_000;
const LATE_WRITE_DELAY_MS: u64 = 80;
/// A trial in which the parent did not reach `poll()` before the peer wrote
/// proves nothing about wakes, so it is retried rather than reported. The delay
/// doubles each time -- 80, 160, 320, 640, 1280 ms, all far inside the 5 s poll
/// timeout -- so exhausting this budget means the parent could not get from one
/// `send()` to the next syscall inside 1.28 s. That is a real pathology, and it
/// is the only thing that turns a missed window into a FAIL.
const LATE_MAX_ATTEMPTS: u32 = 5;
/// One byte, sent by the parent on its own end of the connection, that tells the
/// child the parent is about to poll. It travels the opposite direction from the
/// payload, so it cannot make the polled fd readable.
const READY_TOKEN: &[u8] = b"R";

const PAYLOAD_ARMED: &[u8] = b"armed568";
/// Stage 3's payload is the tag followed by two little-endian `monotonic_ms()`
/// stamps taken by the child: when it received the readiness token, and
/// immediately before this send. The parent scores its poll against both.
const PAYLOAD_LATE: &[u8] = b"late568";
const LATE_MSG_LEN: usize = 7 + 8 + 8;

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

/// Read exactly `buf.len()` bytes, or fail naming the stage. Loopback delivers
/// these short messages in one piece in practice, but a short read must not be
/// silently reinterpreted as a different message.
fn recv_exact(fd: Fd, buf: &mut [u8], stage: &'static str) -> Result<(), Failure> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match socket::recv(fd, &mut buf[filled..]) {
            Ok(0) => {
                return Err(fail(
                    stage,
                    format!("peer closed after {} of {} bytes", filled, buf.len()),
                ))
            }
            Ok(n) => filled += n,
            Err(e) => return Err(fail(stage, format!("{}", e))),
        }
    }
    Ok(())
}

/// Sleep until an absolute instant on the child's own monotonic clock. Sleeping
/// to an instant rather than for a duration means an interrupted `nanosleep`
/// cannot shorten the wait without the parent being able to see it.
fn sleep_until(due_ms: u64) {
    while let Ok(now) = monotonic_ms("child_sleep") {
        if now >= due_ms {
            return;
        }
        let _ = time::sleep_ms(due_ms - now);
    }
}

/// Exit codes the stage-3 peer uses to name the branch it took. `0` is the
/// one that means the payload reached the socket; the other four each name a
/// place the peer stopped short, and the parent prints whichever it read out of
/// `waitpid`.
const PEER_EXIT_SENT: i32 = 0;
const PEER_EXIT_TOKEN_RECV_FAILED: i32 = 11;
const PEER_EXIT_TOKEN_CLOCK_FAILED: i32 = 12;
const PEER_EXIT_WRITE_CLOCK_FAILED: i32 = 13;
const PEER_EXIT_SEND_FAILED: i32 = 14;

/// Report the branch the stage-3 peer took, then leave with the matching code.
///
/// #693 recorded a `late_lost_wake` whose two candidate explanations -- the
/// kernel lost a readiness publication, or the peer had not published one --
/// could not be told apart from anything the boot emitted: `late_peer` swallowed
/// its `send` result and exited `0` on both, so neither the verdict line nor the
/// exit status discriminated. This marker is that missing fact. Each of the five
/// paths out of the peer emits it, the successful one included, so a PASS boot
/// carries the same evidence shape as a FAIL, and a missing branch line means
/// the peer stopped before reaching any of the five.
fn peer_exit(branch: &str, detail: &str, code: i32) -> ! {
    emit(&format!(
        "[POLL_TCP_ORACLE:PEER:branch={}:{}]",
        branch, detail
    ));
    process::exit(code);
}

/// The child half of stage 3. Waits for the parent's readiness token, stamps
/// that moment, waits out the delay, stamps again, and ships both stamps with
/// the payload. Never returns.
fn late_peer(server: Fd, delay_ms: u64) -> ! {
    let mut token = [0u8; 1];
    if let Err(f) = recv_exact(server, &mut token, "child_token") {
        peer_exit(
            "token_recv_failed",
            &format!("delay_ms={} detail={}", delay_ms, f.detail),
            PEER_EXIT_TOKEN_RECV_FAILED,
        );
    }
    let token_ms = match monotonic_ms("child_token_stamp") {
        Ok(ms) => ms,
        Err(f) => peer_exit(
            "token_clock_failed",
            &format!("delay_ms={} detail={}", delay_ms, f.detail),
            PEER_EXIT_TOKEN_CLOCK_FAILED,
        ),
    };
    sleep_until(token_ms + delay_ms);
    let write_ms = match monotonic_ms("child_write") {
        Ok(ms) => ms,
        Err(f) => peer_exit(
            "write_clock_failed",
            &format!("token_ms={} delay_ms={} detail={}", token_ms, delay_ms, f.detail),
            PEER_EXIT_WRITE_CLOCK_FAILED,
        ),
    };
    let mut msg = [0u8; LATE_MSG_LEN];
    msg[..7].copy_from_slice(PAYLOAD_LATE);
    msg[7..15].copy_from_slice(&token_ms.to_le_bytes());
    msg[15..].copy_from_slice(&write_ms.to_le_bytes());
    match socket::send(server, &msg) {
        Ok(n) => peer_exit(
            "sent",
            &format!(
                "n={} want={} token_ms={} write_ms={} delay_ms={}",
                n, LATE_MSG_LEN, token_ms, write_ms, delay_ms
            ),
            PEER_EXIT_SENT,
        ),
        Err(e) => peer_exit(
            "send_failed",
            &format!(
                "err={} token_ms={} write_ms={} delay_ms={}",
                e, token_ms, write_ms, delay_ms
            ),
            PEER_EXIT_SEND_FAILED,
        ),
    }
}

/// Re-read the fd the blocking poll just gave up on, two ways, and render both
/// results as one marker line. See the call site for what each probe decides.
fn late_lost_wake_probe(client: Fd) -> String {
    let mut fds = [PollFd::new(client, POLLIN)];
    let rescan = match io::poll(&mut fds, 0) {
        Ok(n) => format!("rescan_ready={} rescan_revents={:#06x}", n, fds[0].revents),
        Err(e) => format!("rescan_err={}", e),
    };

    let nbread = match io::fcntl_getfl(client) {
        Ok(original) => {
            match io::fcntl_setfl(client, original as i32 | io::status_flags::O_NONBLOCK) {
                Ok(_) => {
                    let mut buf = [0u8; LATE_MSG_LEN];
                    let outcome = match io::read(client, &mut buf) {
                        Ok(n) => format!("nbread_n={}", n),
                        Err(e) => format!("nbread_err={}", e),
                    };
                    let _ = io::fcntl_setfl(client, original as i32);
                    outcome
                }
                Err(e) => format!("nbread_setfl_err={}", e),
            }
        }
        Err(e) => format!("nbread_getfl_err={}", e),
    };

    format!("[POLL_TCP_ORACLE:LOSTWAKE_PROBE:{} {}]", rescan, nbread)
}

/// The lost-wake stage: readiness is published while the poller is already
/// parked inside `poll()`.
///
/// Returns `(elapsed_ms, park_ms, attempts)`, where `park_ms` is the measured
/// span between the parent's poll entry and the peer's actual write.
fn stage_late(client: Fd, server: Fd) -> Result<(u64, u64, u64), Failure> {
    let mut attempt: u32 = 1;
    let mut delay_ms = LATE_WRITE_DELAY_MS;
    loop {
        let child = match process::fork() {
            Ok(ForkResult::Child) => late_peer(server, delay_ms),
            Ok(ForkResult::Parent(pid)) => pid,
            Err(e) => return Err(fail("late_fork", format!("{}", e))),
        };

        // Everything expensive -- process creation, the child's first schedule --
        // happens before the token goes out, so neither clock has started yet.
        // The token travels parent -> child on the reverse direction of this
        // connection, so it cannot make `client` readable.
        if let Err(e) = socket::send(client, READY_TOKEN) {
            return Err(fail("late_ready_send", format!("{}", e)));
        }

        let mut fds = [PollFd::new(client, POLLIN)];
        let entry = monotonic_ms("late_entry")?;
        let ready = match io::poll(&mut fds, LATE_TIMEOUT_MS) {
            Ok(n) => n,
            Err(e) => return Err(fail("late_poll", format!("{}", e))),
        };
        let returned = monotonic_ms("late_return")?;
        let elapsed = returned.saturating_sub(entry);

        if ready != 1 || fds[0].revents & POLLIN == 0 {
            // #693: emit the discriminating facts BEFORE the reap, because the
            // reap is the one step in this arm that can itself block without a
            // bound -- a peer still parked in its own token receive has not
            // reached its exit. Two probes and the peer's own branch line
            // together separate "the kernel lost a readiness publication" from
            // "the peer had not published one yet":
            //
            //   rescan  -- a timeout=0 poll on the same fd, immediately after
            //              the blocking poll gave up. Ready here means the
            //              readiness existed and only the blocking loop missed
            //              it; not ready means the entry scan agrees with the
            //              blocking scans.
            //   nbread  -- a non-blocking read of the same fd. Bytes here with
            //              rescan not ready means the data is in the socket and
            //              the readiness predicate does not see it, which is a
            //              different defect from either candidate.
            //
            // Both probes run only on this already-failed arm, so neither can
            // consume anything a passing trial needs.
            emit(&late_lost_wake_probe(client));
            let mut status = 0i32;
            let reaped = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);
            return Err(fail(
                "late_lost_wake",
                format!(
                    "ready={} revents={:#06x} elapsed_ms={} delay_ms={} peer_reaped={} peer_status={:#010x} peer_code={}",
                    ready,
                    fds[0].revents,
                    elapsed,
                    delay_ms,
                    reaped.is_ok(),
                    status,
                    (status >> 8) & 0xFF
                ),
            ));
        }
        reject_broken("late_broken", fds[0].revents)?;

        let mut buf = [0u8; LATE_MSG_LEN];
        recv_exact(client, &mut buf, "late_recv")?;
        let mut status = 0i32;
        let _ = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);
        if &buf[..7] != PAYLOAD_LATE {
            return Err(fail("late_payload", format!("tag={:?}", &buf[..7])));
        }
        let mut stamp = [0u8; 8];
        stamp.copy_from_slice(&buf[7..15]);
        let token_ms = u64::from_le_bytes(stamp);
        stamp.copy_from_slice(&buf[15..]);
        let write_ms = u64::from_le_bytes(stamp);

        // The peer waits to an absolute instant on the same kernel clock, so a
        // short wait is not a scheduling artifact -- it is `nanosleep` or the
        // clock misbehaving, and it belongs in the verdict rather than in a
        // retry that would quietly paper over it. This is also the bound that
        // keeps stage 3 from degenerating into stage 2: without it, a peer that
        // publishes readiness immediately satisfies every other check with a
        // proven parked span of zero.
        if write_ms < token_ms + delay_ms {
            return Err(fail(
                "late_peer_short_wait",
                format!(
                    "token_ms={} write_ms={} waited_ms={} delay_ms={}",
                    token_ms,
                    write_ms,
                    write_ms.saturating_sub(token_ms),
                    delay_ms
                ),
            ));
        }

        if entry > token_ms {
            // The parent did not reach its own poll entry before the peer's
            // clock started. Nothing about wakes can be concluded from this
            // trial either way, so it is not a verdict: widen the delay and try
            // again. Reporting this as a failure is the bug two earlier
            // revisions of this bound had -- it is ordinary parent-side
            // scheduling latency, not a kernel defect.
            attempt += 1;
            delay_ms *= 2;
            if attempt > LATE_MAX_ATTEMPTS {
                return Err(fail(
                    "late_window_missed",
                    format!(
                        "attempts={} overshoot_ms={} delay_ms={}",
                        LATE_MAX_ATTEMPTS,
                        entry - token_ms,
                        delay_ms / 2
                    ),
                ));
            }
            continue;
        }

        // Anti-vacuity, scored against the peer's measured write instant rather
        // than either side's stopwatch: the only thing that can make this poll
        // report POLLIN is that send, so a correct kernel cannot return before
        // it. A return before it is readiness the peer had not published --
        // stale state, not a wake. No timing slack, nothing to retune.
        if returned < write_ms {
            return Err(fail(
                "late_early_ready",
                format!(
                    "returned={} write_ms={} entry={} elapsed_ms={}",
                    returned, write_ms, entry, elapsed
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

        return Ok((elapsed, write_ms - entry, attempt as u64));
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
