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
//! Four stages, each a distinct failure mode:
//!
//!   stage 1  `idle`   — block on a connected fd with no readable bytes and a
//!                       finite timeout. Must return 0 having actually slept.
//!                       A wedge hangs here; a vacuous instant return (one in
//!                       which the blocking path was not entered) is also a
//!                       FAIL, because a poll that does not block is not
//!                       evidence about blocking.
//!   stage 2  `armed`  — data is already pending when poll is called. Must
//!                       report POLLIN from the entry scan.
//!   stage 3  `late`   — THE LOST-WAKE STAGE. The poller enters `poll()` first
//!                       and a forked peer writes only afterwards, so readiness
//!                       is published while the poller is already blocked. This
//!                       is the one stage a stale-snapshot or edge-triggered
//!                       wake cannot pass by luck.
//!   stage 4  `forced` — THE CLASSIFICATION STAGE. The peer is made to publish
//!                       AFTER the poll's deadline on purpose (a 150 ms poll
//!                       against a 500 ms peer delay), so the poll returning 0
//!                       is the CORRECT answer and the verdict has to say so.
//!
//! Do not weaken any stage to `timeout=0`.
//!
//! ## What a 0-ready return means, and what #693 made of it
//!
//! Stages 3 and 4 both end in a `poll()` that can hand back `ready=0` with
//! empty `revents`, and the two reasons that happens are opposites:
//!
//!   * readiness was published while the poller was parked and the poll failed
//!     to report it — a lost wake, and a kernel defect;
//!   * the peer had not published by the poll's deadline — in which case 0 is
//!     the correct answer for a socket with an empty receive buffer.
//!
//! Until #693, this file decided that fork from `ready != 1 || revents & POLLIN
//! == 0` alone and named the result `late_lost_wake`: a kernel wake-loss
//! verdict whose inputs did not include the peer's publication instant, so it
//! emitted the same word for both. The #693 RCA read thirteen reproductions
//! and the issue's own preserved specimen and found 14 of 14 of them to be the
//! second kind — the peer's `nanosleep` overran by up to 10.3 s under round
//! robin and its write landed after the poll's whole 5 s budget had expired.
//!
//! A timeout arm now recovers the peer's stamps before deciding: the peer's
//! exit code says whether it published, and its `write_ms` says when.
//! `late_lost_wake` now needs TWO facts, and it is a FAIL only with both: the
//! peer began its send before the poll's own deadline (`entry + timeout_ms`, a
//! lower bound on the kernel's), and the bytes were on the socket the instant
//! the poll gave up — which the probe reads directly. Anything else is a window
//! the publication did not enter, decides neither way, and is retried against a
//! higher poll ceiling; if the peer keeps overrunning, the FAIL that ends it is
//! `late_peer_overrun` carrying the measured overrun, not a wake-loss claim.
//!
//! Both facts are needed because each alone is a stamp that can lie in a
//! direction that manufactures #693's error again:
//!
//!   * `returned`, the parent's post-syscall stamp, is not the poll's deadline.
//!     They differ by however long it took the parent to get the CPU back,
//!     measured at 543 ms for a 150 ms poll on beast KVM. A publication landing
//!     in that gap is one the poll had already correctly declined to report.
//!   * `write_ms` is stamped BEFORE the peer's `send()`, so it is a lower bound
//!     on the publication and not the publication itself. A peer descheduled
//!     between the stamp and the syscall publishes later than it says, measured
//!     at 762 ms on beast KVM. The probe closes that: an empty socket at the
//!     instant the poll gave up is the kernel's state, not a stamp.
//!
//! The authoritative detector is on the kernel side —
//! `[POLL_TCP_READY_LOST]` from `sys_poll`, which compares the connection's own
//! publication instant against the kernel's own deadline. This verdict is the
//! corroborating one, and the two agree on 5 of 5 boots under mutation 693-K.
//!
//! ## The anchor, and the three facts each trial establishes
//!
//! The parent stamps `anchor` before it sends the readiness token, and the
//! token carries that stamp. The peer sleeps to `anchor + delay_ms` on the same
//! kernel clock, stamps `write_ms` immediately before its send, and ships the
//! anchor back with it. The parent then has three comparable instants of its
//! own -- `anchor`, `entry` (taken immediately before `poll()`) and `returned`
//! -- plus the peer's `write_ms`, and it establishes:
//!
//!   echo     == anchor                (this payload belongs to this trial, not
//!                                      to a leftover from an earlier attempt)
//!   write_ms >= anchor + delay_ms     (the peer really waited: it sleeps to
//!                                      that instant, so a short wait is the
//!                                      kernel's doing and is reported as one)
//!   entry    <= write_ms              (the parent was inside `poll()` before
//!                                      the peer published, so the publication
//!                                      landed on a parked poller)
//!
//! `park_ms = write_ms - entry` is then REPORTED, so the evidence shows the
//! separation instead of asserting it. A return before `write_ms` means the
//! poll saw readiness the peer had not published -- stale state, not a wake --
//! and is a named FAIL.
//!
//! The middle fact is what stops the stage from silently degenerating into a
//! second copy of stage 2. An earlier revision established only
//! `entry <= write_ms`; a mutation that had the peer write immediately then
//! PASSED it, reporting `park_ms=0`, because the two stamps landed in the same
//! truncated millisecond. A bound a degenerate run satisfies is not a bound.
//!
//! Two design points keep this from becoming a flake generator, which is
//! exactly what two earlier revisions of this bound were:
//!
//!   * The `fork()` is OUTSIDE the timing window. The parent forks, then hands
//!     the child the readiness token over the *reverse* direction of the same
//!     connection, and only then polls. The child waits on that token before
//!     starting its delay, so process creation -- which on a loaded emulated
//!     x86 boot costs hundreds of milliseconds, far more than the delay itself
//!     -- is spent before the trial's clock starts. The first revision compared
//!     the parent's elapsed against the child's sleep *duration* and reddened
//!     ~8% of healthy aarch64 boots; the second moved to a shared absolute
//!     deadline but still paid the fork inside the window and could not reach
//!     its own poll in time on x86.
//!   * `entry <= write_ms` is a PRECONDITION, not a verdict. If the parent was
//!     descheduled between handing over the token and reaching `poll()`, the
//!     trial decides neither way, so its payload is consumed, the child reaped,
//!     and the delay widened. Only an exhausted budget is a FAIL, and it reports
//!     the measured overshoot so the failure is legible.
//!
//!     #694 is why that precondition is scored against `write_ms` rather than
//!     against the peer's receipt stamp. The old form compared `entry` to the
//!     instant the PEER was scheduled to read the token, which `delay_ms` does
//!     not move at all -- so widening the delay could not influence the race it
//!     was retrying, and five doublings were five flips of one coin (4 of 24
//!     beast KVM boots exhausted the ladder and emitted a false FAIL). With the
//!     peer anchored to the parent's own pre-send stamp, the peer's write lands
//!     at `anchor + delay_ms + overrun`, so each doubling strictly widens the
//!     room the parent has to get from its `send()` to its `poll()`.

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
/// The opposite inconclusive shape, and the one #693 is filed on: the parent
/// reached `poll()` in time, but the PEER did not publish before the poll's
/// deadline, so the window the trial is about did not contain a publication.
/// Widening the delay makes that strictly worse, so this retry raises the
/// poll's own ceiling instead -- 5 s, 10 s, 20 s. Raising the ceiling is free
/// when the trial then succeeds, because the poll returns when the peer
/// publishes and not when the timeout expires. Exhausting the budget means the
/// peer could not get from a `nanosleep` deadline to the next instruction
/// inside twenty seconds across three tries, which is a real pathology and is
/// reported as `late_peer_overrun` with the measured overrun rather than as a
/// lost wake, which is what the pre-#693 code called it.
const LATE_MAX_PEER_LATE_ATTEMPTS: u32 = 3;

/// Stage 4's poll timeout and peer delay. The peer is told to wait more than
/// three times the poll's whole budget, so its publication lands after the
/// return by construction and the arm that classifies a 0-ready return runs on
/// each boot. Both are small because the stage's cost is the peer's delay, and
/// it is paid once per boot on each arch.
const FORCED_TIMEOUT_MS: i32 = 150;
const FORCED_WRITE_DELAY_MS: u64 = 500;
/// The readiness token the parent sends on its own end of the connection to
/// tell the child that the parent is about to poll. It travels the opposite
/// direction from the payload, so it cannot make the polled fd readable.
///
/// #694: the token now CARRIES the parent's own pre-send stamp, and the peer
/// sleeps to `anchor + delay_ms` on that stamp rather than to `token_ms +
/// delay_ms` on its own receipt. The old anchor was the peer's receipt instant,
/// which moves with the peer's scheduling, so widening `delay_ms` did not move
/// the write relative to the parent's poll entry at all and the retry ladder
/// was five flips of one coin. Anchored to the parent's clock, the peer's write
/// instant is `anchor + delay_ms + overrun`, so a wider delay buys the parent
/// strictly more room to reach `poll()` -- the retry can now influence the race
/// it retries.
const READY_TOKEN_TAG: u8 = b'R';
const READY_TOKEN_LEN: usize = 1 + 8;

const PAYLOAD_ARMED: &[u8] = b"armed568";
/// Stage 3's payload is the tag, the anchor the child was handed echoed back,
/// and the child's own `monotonic_ms()` stamp taken immediately before this
/// send. The echo is what ties the write to THIS trial's anchor; the parent
/// refuses a payload whose echo does not match what it sent.
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
const PEER_EXIT_TOKEN_MALFORMED: i32 = 15;

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
    let mut token = [0u8; READY_TOKEN_LEN];
    if let Err(f) = recv_exact(server, &mut token, "child_token") {
        peer_exit(
            "token_recv_failed",
            &format!("delay_ms={} detail={}", delay_ms, f.detail),
            PEER_EXIT_TOKEN_RECV_FAILED,
        );
    }
    if token[0] != READY_TOKEN_TAG {
        peer_exit(
            "token_malformed",
            &format!("tag={:#04x} delay_ms={}", token[0], delay_ms),
            PEER_EXIT_TOKEN_MALFORMED,
        );
    }
    let mut anchor_bytes = [0u8; 8];
    anchor_bytes.copy_from_slice(&token[1..]);
    let anchor = u64::from_le_bytes(anchor_bytes);
    // Reported for the same reason the receipt stamp used to be: it is the
    // peer's own reading of when it was scheduled, and the gap between it and
    // the anchor is the dispatch latency that #693 turned out to be about.
    let token_ms = match monotonic_ms("child_token_stamp") {
        Ok(ms) => ms,
        Err(f) => peer_exit(
            "token_clock_failed",
            &format!("delay_ms={} detail={}", delay_ms, f.detail),
            PEER_EXIT_TOKEN_CLOCK_FAILED,
        ),
    };
    sleep_until(anchor + delay_ms);
    let write_ms = match monotonic_ms("child_write") {
        Ok(ms) => ms,
        Err(f) => peer_exit(
            "write_clock_failed",
            &format!("anchor={} delay_ms={} detail={}", anchor, delay_ms, f.detail),
            PEER_EXIT_WRITE_CLOCK_FAILED,
        ),
    };
    let mut msg = [0u8; LATE_MSG_LEN];
    msg[..7].copy_from_slice(PAYLOAD_LATE);
    msg[7..15].copy_from_slice(&anchor.to_le_bytes());
    msg[15..].copy_from_slice(&write_ms.to_le_bytes());
    match socket::send(server, &msg) {
        Ok(n) => peer_exit(
            "sent",
            &format!(
                "n={} want={} anchor={} token_ms={} write_ms={} delay_ms={}",
                n, LATE_MSG_LEN, anchor, token_ms, write_ms, delay_ms
            ),
            PEER_EXIT_SENT,
        ),
        Err(e) => peer_exit(
            "send_failed",
            &format!(
                "err={} anchor={} token_ms={} write_ms={} delay_ms={}",
                e, anchor, token_ms, write_ms, delay_ms
            ),
            PEER_EXIT_SEND_FAILED,
        ),
    }
}

/// What the probe of a timed-out poll found, and anything it took off the
/// socket while looking.
///
/// The non-blocking read is destructive: on a genuine lost wake it is exactly
/// the read that succeeds, and the bytes it takes are the peer's payload --
/// the payload whose stamps decide the verdict. An earlier revision threw them
/// away, which would have made the one case this stage exists to catch the one
/// case it could not score.
struct LostWakeProbe {
    marker: String,
    consumed: [u8; LATE_MSG_LEN],
    consumed_len: usize,
    /// True when either probe found bytes on the socket: the `timeout=0`
    /// rescan reported a ready fd, or the non-blocking read returned bytes.
    /// This is the fact that separates a lost wake from a late publication, and
    /// the #693 RCA named it in advance -- "a real lost wake would show as
    /// `rescan_ready=1` or a positive `nbread_n`".
    data_present: bool,
}

/// Re-read the fd the blocking poll just gave up on, two ways, and render both
/// results as one marker line. See the call site for what each probe decides.
fn late_lost_wake_probe(client: Fd) -> LostWakeProbe {
    // Stamp the probe on the same clock the peer stamps its write with. Marker
    // ORDER on the shared console decided three of the first four specimens and
    // left the fourth open, because the peer prints after its `send` returns and
    // the parent prints after its probe runs -- two prints whose interleaving a
    // preemption can invert. Two comparable instants close that gap: `probe_ms`
    // against the peer's `write_ms` says which came first without depending on
    // when either side got to print.
    let probe_ms = monotonic_ms("late_probe").unwrap_or(0);
    let mut fds = [PollFd::new(client, POLLIN)];
    let mut data_present = false;
    let rescan = match io::poll(&mut fds, 0) {
        Ok(n) => {
            if n > 0 {
                data_present = true;
            }
            format!("rescan_ready={} rescan_revents={:#06x}", n, fds[0].revents)
        }
        Err(e) => format!("rescan_err={}", e),
    };

    let mut consumed = [0u8; LATE_MSG_LEN];
    let mut consumed_len = 0usize;
    let nbread = match io::fcntl_getfl(client) {
        Ok(original) => {
            match io::fcntl_setfl(client, original as i32 | io::status_flags::O_NONBLOCK) {
                Ok(_) => {
                    let outcome = match io::read(client, &mut consumed) {
                        Ok(n) => {
                            consumed_len = n;
                            if n > 0 {
                                data_present = true;
                            }
                            format!("nbread_n={}", n)
                        }
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

    LostWakeProbe {
        marker: format!(
            "[POLL_TCP_ORACLE:LOSTWAKE_PROBE:probe_ms={} {} {}]",
            probe_ms, rescan, nbread
        ),
        consumed,
        consumed_len,
        data_present,
    }
}

/// What the peer ships inside its payload: the anchor it was handed, echoed
/// back, and the instant immediately before its `send`.
struct PeerStamps {
    anchor_echo: u64,
    write_ms: u64,
}

fn decode_late_payload(buf: &[u8; LATE_MSG_LEN]) -> Result<PeerStamps, Failure> {
    if &buf[..7] != PAYLOAD_LATE {
        return Err(fail("late_payload", format!("tag={:?}", &buf[..7])));
    }
    let mut stamp = [0u8; 8];
    stamp.copy_from_slice(&buf[7..15]);
    let anchor_echo = u64::from_le_bytes(stamp);
    stamp.copy_from_slice(&buf[15..]);
    let write_ms = u64::from_le_bytes(stamp);
    Ok(PeerStamps {
        anchor_echo,
        write_ms,
    })
}

/// How long the parent will wait for a payload whose sender has already been
/// reaped. The peer's `send` returned before it exited, so the bytes exist;
/// this bounds the wait for them to become readable rather than betting the
/// whole oracle on a blocking `recv` that a torn-down connection would not
/// satisfy.
const LATE_DRAIN_TIMEOUT_MS: i32 = 2_000;

/// Recover the peer's stamps after a timeout return, starting from whatever the
/// probe's destructive read already took off the socket.
///
/// This is the fact #693 lacked. The issue's own text names its undecided fork
/// as "whether the peer's `send()` was actually issued and its readiness lost,
/// or whether the peer's own `recv` of the token never completed so it never
/// wrote at all", and the arm that had to answer it reaped the peer and
/// reported without ever reading the socket. The peer's exit code names the
/// branch it took; these two stamps say WHEN it published, which is what
/// separates a lost publication from one that had not happened yet.
/// <!-- claim-lint:ok: the quoted sentence is #693's own text, verbatim; the
///      RCA decided it 13 of 13 reproductions, docs/planning/green-program/
///      sockets/693-RCA-2026-09-02.md §5. -->
fn collect_late_payload(client: Fd, probe: &LostWakeProbe) -> Result<PeerStamps, Failure> {
    let mut buf = [0u8; LATE_MSG_LEN];
    let mut filled = probe.consumed_len.min(LATE_MSG_LEN);
    buf[..filled].copy_from_slice(&probe.consumed[..filled]);

    while filled < LATE_MSG_LEN {
        let mut fds = [PollFd::new(client, POLLIN)];
        match io::poll(&mut fds, LATE_DRAIN_TIMEOUT_MS) {
            Ok(0) => {
                return Err(fail(
                    "late_payload_undelivered",
                    format!(
                        "filled={} want={} waited_ms={}",
                        filled, LATE_MSG_LEN, LATE_DRAIN_TIMEOUT_MS
                    ),
                ))
            }
            Ok(_) => {}
            Err(e) => return Err(fail("late_payload_poll", format!("{}", e))),
        }
        match socket::recv(client, &mut buf[filled..]) {
            Ok(0) => {
                return Err(fail(
                    "late_payload_eof",
                    format!("filled={} want={}", filled, LATE_MSG_LEN),
                ))
            }
            Ok(n) => filled += n,
            Err(e) => return Err(fail("late_payload_recv", format!("{}", e))),
        }
    }

    decode_late_payload(&buf)
}

/// The two things a stage-3-shaped trial can be asking of the kernel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LateMode {
    /// The peer's publication is meant to land INSIDE the poll's window, so a
    /// poll that returns 0 ready fds either lost it or was not given one. This
    /// is the lost-wake test proper, and it is the one whose window the peer's
    /// own dispatch latency can overrun -- see `Forced` for why that matters.
    Race,
    /// The peer's publication is meant to land AFTER the poll's deadline, by
    /// construction: a short poll timeout against a long peer delay. A poll that
    /// returns 0 ready fds here is CORRECT, and the whole point of the arm is
    /// that the verdict must say so.
    ///
    /// #693 is a boot in which the `Race` trial's peer published late by
    /// accident, and the verdict called it a lost kernel wake. This arm makes
    /// that same shape happen on purpose on each boot, so a verdict that cannot
    /// tell "published after my deadline" from "you lost my publication" fails
    /// here deterministically instead of once in twenty-four boots: the
    /// pre-#693 predicate, restored as mutation 693-U, reddens this stage on
    /// 5 of 5 aarch64 boots.
    Forced,
}

/// One trial's configuration.
struct LateTrial {
    mode: LateMode,
    stage: &'static str,
    timeout_ms: i32,
    delay_ms: u64,
}

/// How the timeout arm of a trial was decided, once the peer's publication
/// instant is in hand.
enum TimeoutVerdict {
    /// The peer began its send before the poll's budget expired AND the bytes
    /// were on the socket the instant the poll gave up. This is a lost wake.
    Lost,
    /// The peer had not begun its send when the poll's budget expired. The
    /// poll's 0 is the right answer for a socket that had not been written to.
    PublishedAfterDeadline,
    /// The peer had begun its send before the deadline, but the socket was
    /// still empty when the poll gave up -- so the bytes had not reached it
    /// yet, and the poll's 0 was the right answer for the state it read.
    ///
    /// This case is why the verdict needs the probe. `write_ms` is stamped
    /// BEFORE the peer's `send()`, so it is a lower bound on the publication
    /// and not the publication itself: on beast KVM boot 14 of the second
    /// 25-boot battery, a peer stamped 38488, was descheduled, and its send
    /// landed after the poll's 39250 deadline. The kernel's own reading at that
    /// deadline was `rx_len=0 publish=none_in_window`, and the parent's probe
    /// agreed with it (`rescan_ready=0 … nbread_err=EAGAIN`). Calling that a
    /// lost wake would have been #693's error committed a third time.
    NotReadableAtReturn,
}

/// The outcome of one completed trial.
struct LateResult {
    elapsed_ms: u64,
    park_ms: u64,
    late_by_ms: u64,
}

/// Run one stage-3-shaped trial to a verdict, retrying the two INCONCLUSIVE
/// shapes and reserving a FAIL for the shapes that decide something.
///
/// The two inconclusive shapes are opposites and their retries pull in opposite
/// directions, which is why they have separate budgets:
///
///   * the parent did not reach `poll()` before the peer's clock started
///     (`entry > token_ms`) -- widen the peer's delay so the parent gets in;
///   * the peer did not publish before the poll's deadline -- widening the
///     delay makes this strictly worse, so raise the poll's own ceiling
///     instead. Raising it is free when the trial then succeeds: the poll
///     returns when the peer publishes, not when the timeout expires, so a
///     larger ceiling buys only permission to wait.
fn run_late_trial(client: Fd, server: Fd, trial: LateTrial) -> Result<LateResult, Failure> {
    let mut window_attempt: u32 = 1;
    let mut peer_late_attempt: u32 = 1;
    let mut delay_ms = trial.delay_ms;
    let mut timeout_ms = trial.timeout_ms;
    let mut worst_late_by_ms: u64 = 0;

    loop {
        let child = match process::fork() {
            Ok(ForkResult::Child) => late_peer(server, delay_ms),
            Ok(ForkResult::Parent(pid)) => pid,
            Err(e) => return Err(fail("late_fork", format!("{}", e))),
        };

        // Everything expensive -- process creation, the child's first schedule --
        // happens before the token goes out, so the trial's clock has not
        // started yet. The token travels parent -> child on the reverse
        // direction of this connection, so it cannot make `client` readable,
        // and it carries `anchor`: the peer sleeps to `anchor + delay_ms` on
        // the parent's stamp, which is what makes widening the delay actually
        // buy the parent room to reach `poll()` (#694).
        let anchor = monotonic_ms("late_anchor")?;
        let mut token = [0u8; READY_TOKEN_LEN];
        token[0] = READY_TOKEN_TAG;
        token[1..].copy_from_slice(&anchor.to_le_bytes());
        if let Err(e) = socket::send(client, &token) {
            return Err(fail("late_ready_send", format!("{}", e)));
        }

        let mut fds = [PollFd::new(client, POLLIN)];
        let entry = monotonic_ms("late_entry")?;
        let ready = match io::poll(&mut fds, timeout_ms) {
            Ok(n) => n,
            Err(e) => return Err(fail("late_poll", format!("{}", e))),
        };
        let returned = monotonic_ms("late_return")?;
        let elapsed = returned.saturating_sub(entry);

        let timed_out = ready != 1 || fds[0].revents & POLLIN == 0;
        // Whether the socket held bytes at the instant the poll gave up. Read
        // by the probe below on the timeout arm, and it is one of the two facts
        // the `late_lost_wake` verdict needs.
        let mut readable_at_return = false;
        let stamps = if timed_out {
            // Emit the discriminating facts BEFORE the reap, because the reap is
            // the one step in this arm that can itself block without a bound --
            // a peer still parked in its own token receive has not reached its
            // exit. Two probes and the peer's own branch line together separate
            // "the kernel lost a readiness publication" from "the peer had not
            // published one yet":
            //
            //   rescan  -- a timeout=0 poll on the same fd, immediately after
            //              the blocking poll gave up. Ready here means the
            //              readiness existed and only the blocking loop missed
            //              it; not ready means the entry scan agrees with the
            //              blocking scans.
            //   nbread  -- a non-blocking read of the same fd. Bytes here with
            //              rescan not ready means the data is in the socket and
            //              the readiness predicate does not see it, which is a
            //              different defect from either candidate. Whatever it
            //              takes off the socket is carried into the payload
            //              collection below rather than discarded.
            let probe = late_lost_wake_probe(client);
            readable_at_return = probe.data_present;
            emit(&probe.marker);

            let mut status = 0i32;
            let reaped = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);
            let peer_code = (status >> 8) & 0xFF;
            if !reaped.is_ok() || peer_code != PEER_EXIT_SENT {
                // #693's second candidate, and now it is nameable: the peer did
                // not reach its send at all. Its own branch line says where it
                // stopped; this verdict says that it did.
                return Err(fail(
                    "late_peer_no_publish",
                    format!(
                        "stage={} ready={} revents={:#06x} anchor={} entry={} returned={} elapsed_ms={} timeout={} delay_ms={} peer_reaped={} peer_status={:#010x} peer_code={}",
                        trial.stage,
                        ready,
                        fds[0].revents,
                        anchor,
                        entry,
                        returned,
                        elapsed,
                        timeout_ms,
                        delay_ms,
                        reaped.is_ok(),
                        status,
                        peer_code
                    ),
                ));
            }
            collect_late_payload(client, &probe)?
        } else {
            reject_broken("late_broken", fds[0].revents)?;
            let mut buf = [0u8; LATE_MSG_LEN];
            recv_exact(client, &mut buf, "late_recv")?;
            let mut status = 0i32;
            let _ = process::waitpid(child.raw() as i32, &mut status as *mut i32, 0);
            decode_late_payload(&buf)?
        };

        let write_ms = stamps.write_ms;

        // The echo ties this payload to THIS trial's anchor. Without it a
        // payload left over from an earlier attempt could be scored against the
        // current one's stamps, which is a way to pass by accident.
        if stamps.anchor_echo != anchor {
            return Err(fail(
                "late_token_mismatch",
                format!(
                    "stage={} anchor={} echo={} write_ms={} delay_ms={}",
                    trial.stage, anchor, stamps.anchor_echo, write_ms, delay_ms
                ),
            ));
        }

        // The peer waits to an absolute instant on the same kernel clock, so a
        // short wait is not a scheduling artifact -- it is `nanosleep` or the
        // clock misbehaving, and it belongs in the verdict rather than in a
        // retry that would quietly paper over it. This is also the bound that
        // keeps a `Race` trial from degenerating into stage 2: without it, a
        // peer that publishes readiness immediately satisfies the other checks
        // with a measured parked span of 0.
        if write_ms < anchor + delay_ms {
            return Err(fail(
                "late_peer_short_wait",
                format!(
                    "stage={} anchor={} write_ms={} waited_ms={} delay_ms={}",
                    trial.stage,
                    anchor,
                    write_ms,
                    write_ms.saturating_sub(anchor),
                    delay_ms
                ),
            ));
        }

        if timed_out {
            // Score the publication against the poll's own BUDGET, not against
            // the instant the parent got the CPU back and stamped `returned`.
            // The two are not the same number and the gap between them is
            // scheduling latency: on beast KVM boot 20 of the first 25-boot
            // battery, a 150 ms poll returned 693 ms after entry, and the peer
            // published 247 ms after the deadline but 296 ms before that
            // return. Scored against `returned` that reads as a lost wake, and
            // it is not one -- the poll had already decided, correctly, on an
            // empty buffer. Scoring against `returned` would have been the
            // #693 error committed a second time, one instant to the left.
            //
            // `entry` is stamped immediately before the syscall and the kernel
            // starts its countdown at or after that, so `entry + timeout_ms` is
            // a LOWER bound on the kernel's deadline -- which is the direction
            // that keeps this sound. A publication strictly before it is
            // strictly inside the kernel's window too, so a poll that did not
            // report it lost it. The interval this gives up on is the kernel's
            // own syscall-entry latency, microseconds wide.
            let deadline = entry.saturating_add(timeout_ms.max(0) as u64);
            let verdict = if write_ms >= deadline {
                TimeoutVerdict::PublishedAfterDeadline
            } else if readable_at_return {
                TimeoutVerdict::Lost
            } else {
                TimeoutVerdict::NotReadableAtReturn
            };
            let late_by = write_ms.saturating_sub(deadline);
            // Report the classification and the numbers it was made from on
            // each timeout arm, passing or failing. The overrun this prints is
            // the quantity the #693 investigation had to reconstruct from
            // kernel syscall ordering across thirteen preserved boots.
            emit(&format!(
                "[POLL_TCP_ORACLE:LATE_PUBLISH:stage={} decided={} anchor={} entry={} deadline={} returned={} write_ms={} late_by_ms={} delay_ms={} timeout={}]",
                trial.stage,
                match verdict {
                    TimeoutVerdict::Lost => "lost",
                    TimeoutVerdict::PublishedAfterDeadline => "published_after_deadline",
                    TimeoutVerdict::NotReadableAtReturn => "not_readable_at_return",
                },
                anchor,
                entry,
                deadline,
                returned,
                write_ms,
                late_by,
                delay_ms,
                timeout_ms
            ));

            match verdict {
                TimeoutVerdict::Lost => {
                    // Readiness was on the socket while this poll's budget was
                    // still running, and the poll handed back 0 ready fds. THIS
                    // is a lost wake, and it is what this verdict now names.
                    return Err(fail(
                        "late_lost_wake",
                        format!(
                            "stage={} ready={} revents={:#06x} anchor={} entry={} deadline={} returned={} write_ms={} began_before_deadline_ms={} elapsed_ms={} timeout={} delay_ms={} readable_at_return=1",
                            trial.stage,
                            ready,
                            fds[0].revents,
                            anchor,
                            entry,
                            deadline,
                            returned,
                            write_ms,
                            deadline.saturating_sub(write_ms),
                            elapsed,
                            timeout_ms,
                            delay_ms
                        ),
                    ));
                }
                TimeoutVerdict::PublishedAfterDeadline | TimeoutVerdict::NotReadableAtReturn => {
                    if trial.mode == LateMode::Forced {
                        // This is what the Forced arm exists to observe.
                        return Ok(LateResult {
                            elapsed_ms: elapsed,
                            park_ms: write_ms.saturating_sub(entry),
                            late_by_ms: late_by,
                        });
                    }
                    // Race mode: no conclusion about wakes follows from a
                    // window the publication did not enter. Raise the poll's
                    // ceiling and try again.
                    if late_by > worst_late_by_ms {
                        worst_late_by_ms = late_by;
                    }
                    peer_late_attempt += 1;
                    if peer_late_attempt > LATE_MAX_PEER_LATE_ATTEMPTS {
                        return Err(fail(
                            "late_peer_overrun",
                            format!(
                                "attempts={} worst_late_by_ms={} last_timeout={} delay_ms={}",
                                LATE_MAX_PEER_LATE_ATTEMPTS, worst_late_by_ms, timeout_ms, delay_ms
                            ),
                        ));
                    }
                    timeout_ms = timeout_ms.saturating_mul(2);
                    continue;
                }
            }
        }

        // From here the poll reported POLLIN.
        if trial.mode == LateMode::Forced {
            // The peer published inside a window it was told to stay out of.
            // The trial decides neither way, so widen its delay and retry.
            window_attempt += 1;
            delay_ms *= 2;
            if window_attempt > LATE_MAX_ATTEMPTS {
                return Err(fail(
                    "forced_not_late",
                    format!(
                        "attempts={} anchor={} entry={} returned={} write_ms={} timeout={} delay_ms={}",
                        LATE_MAX_ATTEMPTS,
                        anchor,
                        entry,
                        returned,
                        write_ms,
                        timeout_ms,
                        delay_ms / 2
                    ),
                ));
            }
            continue;
        }

        if entry > write_ms {
            // The parent did not reach its own poll entry before the peer wrote,
            // so the publication was already on the socket when the entry scan
            // ran and this trial decides neither way. It is not a
            // verdict: widen the delay and try again. Reporting it as a
            // failure is the bug two earlier revisions of this bound had -- it
            // is ordinary parent-side scheduling latency, not a kernel defect.
            //
            // #694: the comparison is against the peer's WRITE instant, which
            // the anchor pins to `anchor + delay_ms + overrun`. The pre-#694
            // comparison was against the peer's RECEIPT instant, which the
            // delay does not move at all -- so the ladder below could not
            // influence what it was retrying, and five doublings were five
            // flips of one coin (17% exhaustion on beast KVM, #694). Anchored,
            // each doubling strictly widens the room the parent has to get
            // from its own `send()` to its own `poll()`.
            window_attempt += 1;
            delay_ms *= 2;
            if window_attempt > LATE_MAX_ATTEMPTS {
                return Err(fail(
                    "late_window_missed",
                    format!(
                        "attempts={} overshoot_ms={} send_to_poll_ms={} delay_ms={}",
                        LATE_MAX_ATTEMPTS,
                        entry - write_ms,
                        entry.saturating_sub(anchor),
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
        if elapsed >= timeout_ms as u64 {
            return Err(fail(
                "late_woken_by_clock",
                format!(
                    "entry={} returned={} write_ms={} elapsed_ms={} timeout={}",
                    entry, returned, write_ms, elapsed, timeout_ms
                ),
            ));
        }

        return Ok(LateResult {
            elapsed_ms: elapsed,
            park_ms: write_ms.saturating_sub(entry),
            late_by_ms: 0,
        });
    }
}

/// Stage 3: the lost-wake stage. Readiness is published while the poller is
/// already parked inside `poll()`.
fn stage_late(client: Fd, server: Fd) -> Result<LateResult, Failure> {
    run_late_trial(
        client,
        server,
        LateTrial {
            mode: LateMode::Race,
            stage: "late",
            timeout_ms: LATE_TIMEOUT_MS,
            delay_ms: LATE_WRITE_DELAY_MS,
        },
    )
}

/// Stage 4: the classification stage. The peer is made to publish AFTER the
/// poll's deadline on purpose, so the arm that decides what a 0-ready return
/// means runs on each boot instead of once in however many boots the peer
/// happens to overrun by more than five seconds.
fn stage_forced_late(client: Fd, server: Fd) -> Result<LateResult, Failure> {
    run_late_trial(
        client,
        server,
        LateTrial {
            mode: LateMode::Forced,
            stage: "forced",
            timeout_ms: FORCED_TIMEOUT_MS,
            delay_ms: FORCED_WRITE_DELAY_MS,
        },
    )
}

struct OracleResult {
    idle_ms: u64,
    late: LateResult,
    forced: LateResult,
}

fn run() -> Result<OracleResult, Failure> {
    let (listener, client, server) = connect_loopback()?;

    let idle_ms = stage_idle(client)?;
    stage_armed(client, server)?;
    let late = stage_late(client, server)?;
    let forced = stage_forced_late(client, server)?;

    let _ = io::close(server);
    let _ = io::close(client);
    let _ = io::close(listener);

    Ok(OracleResult {
        idle_ms,
        late,
        forced,
    })
}

fn main() {
    match run() {
        Ok(r) => {
            emit(&format!(
                "[POLL_TCP_ORACLE:PASS:stages=4:idle_ms={}:late_ms={}:park_ms={}:forced_ms={}:forced_late_by_ms={}]",
                r.idle_ms, r.late.elapsed_ms, r.late.park_ms, r.forced.elapsed_ms, r.forced.late_by_ms
            ));
            process::exit(0);
        }
        Err(f) => {
            emit(&format!("[POLL_TCP_ORACLE:FAIL:{}:{}]", f.stage, f.detail));
            process::exit(1);
        }
    }
}
