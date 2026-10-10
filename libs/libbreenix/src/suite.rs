//! Effort-suite runner.
//!
//! A suite binary (`/sbin/suite-<id>`) declares its categories and cases as a static
//! table and calls [`Suite::run`]. The runner prints the suite's serial lines on
//! stdout, one per line, exactly:
//!
//! ```text
//! SUITE <id> START cases=<total>
//! SUITE <id> CASE <category>/<case> PASS ms=<n>
//! SUITE <id> CASE <category>/<case> FAIL ms=<n> msg=<short plain reason>
//! SUITE <id> CASE <category>/<case> SKIP msg=<why>
//! SUITE <id> DONE passed=<p> failed=<f> skipped=<s> total=<n>
//! ```
//!
//! and draws a benchmark-style panel on the framebuffer (libgfx's diagnostics panel,
//! scored) as the cases run. When every case has run it leaves the final panel up and
//! idles: a suite runs as PID 1 and never exits.
//!
//! Each record is written with a single write that starts with a newline, so output
//! another writer left without one (the x86-64 scheduler's raw COM1 breadcrumbs, say)
//! never prefixes it.
//!
//! Each case runs in a forked child whose stdout and stderr go to `/dev/null`, so a
//! case cannot print a line that looks like one of the suite's own. A case that
//! crashes or runs past the suite's time limit is reported as a FAIL saying so, and
//! the suite goes on to the next case; so is a case that cannot be started in a child
//! at all, since running it in the suite's own process would lose that isolation.
//! A case can read how much of its limit is left with [`case_ms_left`], to bound its
//! own waits and fail with its own reason before it is killed.
//!
//! While it runs, a case may also report what it measures, for a reader to show live:
//! [`value`] prints `SUITE <id> VALUE <category>/<case> <name>=<number><unit>` with an
//! optional ` expect=<low>..<high><unit>`, and [`wait_for`] prints
//! `SUITE <id> WAIT <category>/<case> until=<ms> for=<ms>` as it starts a wait, `until`
//! in CLOCK_MONOTONIC milliseconds (the time since boot). They are written, like the
//! runner's own lines, with one write that starts with a newline, through a copy of the
//! suite's stdout the case keeps (close-on-exec) when its own output goes to /dev/null.
//! A case process prints at most [`RECORDS_PER_CASE`] of them; the rest are dropped.
//!
//! The panel shows those records live: under the groups, a strip with the clocks
//! (CLOCK_MONOTONIC to the millisecond and CLOCK_REALTIME as wall time), a countdown
//! for the case's latest WAIT that snaps to the value it reports when the wait is over,
//! and its latest VALUEs on gauges against the ranges they accept (libgfx's
//! `diagnostics::draw_live`). The case sends each record to the runner as well, down a
//! pipe; the runner, not the case, redraws the strip about 15 times a second while it
//! waits for the case, so drawing never runs in the case's process or inside anything
//! it times. `/etc/breenix/suite-live` reading `off` turns the strip, and the pipe, off.
//!
//! A suite runs as PID 1, so any process a case leaves behind is reparented to the
//! runner once the case ends. Before the next case starts, the runner kills every such
//! process with kill(-1, SIGKILL) and reaps it, whatever process group or session it
//! moved to, so no case sees another's processes. A case whose processes outlive
//! SIGKILL fails saying so.
//!
//! The manifest `docs/suites/<id>.json` lists the same categories and cases in the same
//! order, with the same titles; a host test (`tests/suite_manifests.rs`) checks it
//! against the `SUITE` table in the suite's source.
//!
//! ```rust,ignore
//! use libbreenix::suite::{case, category, check, skip, suite, CaseResult, Suite};
//!
//! static SUITE: Suite = suite("smoke", "Smoke", &[
//!     category("process", "Process", &[
//!         case("getpid", "getpid returns a positive PID", getpid),
//!     ]),
//! ]);
//!
//! fn getpid() -> CaseResult {
//!     let pid = libbreenix::process::getpid()?;
//!     check(pid.raw() > 0, "getpid returned 0")
//! }
//!
//! fn main() { SUITE.run() }
//! ```

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::fmt::Write as _;
use std::string::String;
use std::vec::Vec;

use libgfx::diagnostics::{self, Check, CheckState, Group, Live, LiveArea, Panel, Reading, Verdict};
use libgfx::framebuf::FrameBuf;

use crate::error::Error;
use crate::fs;
use crate::io::{self, poll_events, status_flags, PollFd};
use crate::process::{self, ForkResult, WNOHANG};
use crate::signal::{self, SIGKILL};
use crate::types::Fd;
use crate::{graphics, time};

/// Why a case did not pass.
#[derive(Debug)]
pub enum CaseError {
    /// The case ran and something was wrong.
    Fail(String),
    /// The case was not run, and why.
    Skip(String),
}

impl From<String> for CaseError {
    fn from(msg: String) -> Self { CaseError::Fail(msg) }
}

impl From<&str> for CaseError {
    fn from(msg: &str) -> Self { CaseError::Fail(String::from(msg)) }
}

impl From<Error> for CaseError {
    fn from(error: Error) -> Self { CaseError::Fail(std::format!("{error}")) }
}

/// A case passes by returning `Ok(())`. `?` turns a libbreenix error or a string into
/// a FAIL with that message.
pub type CaseResult = Result<(), CaseError>;

/// Fail the case with `msg`.
pub fn fail(msg: impl Into<String>) -> CaseResult { Err(CaseError::Fail(msg.into())) }

/// Skip the case, saying why.
pub fn skip(why: impl Into<String>) -> CaseResult { Err(CaseError::Skip(why.into())) }

/// Pass if `condition` holds, otherwise fail with `msg`.
pub fn check(condition: bool, msg: &str) -> CaseResult {
    if condition { Ok(()) } else { fail(msg) }
}

/// One case: its id (lowercase words joined by '-'), the title the manifest gives it,
/// and the function that runs it.
pub struct Case {
    pub id: &'static str,
    pub title: &'static str,
    pub run: fn() -> CaseResult,
}

pub const fn case(id: &'static str, title: &'static str, run: fn() -> CaseResult) -> Case {
    Case { id, title, run }
}

/// A category of cases, shown as one group with its own progress bar.
pub struct Category {
    pub id: &'static str,
    pub title: &'static str,
    pub cases: &'static [Case],
}

pub const fn category(id: &'static str, title: &'static str, cases: &'static [Case]) -> Category {
    Category { id, title, cases }
}

/// How long a case may run before it is killed and fails, unless the suite sets its own.
pub const DEFAULT_CASE_LIMIT_MS: u64 = 10_000;

/// In a case's process, when its limit passes, in monotonic nanoseconds; 0 elsewhere.
static CASE_DEADLINE_NS: AtomicU64 = AtomicU64::new(0);

/// The milliseconds left before the running case is killed, or `u64::MAX` when called
/// outside a case.
pub fn case_ms_left() -> u64 {
    let deadline = CASE_DEADLINE_NS.load(Ordering::Relaxed);
    if deadline == 0 {
        return u64::MAX;
    }
    match monotonic_ns() {
        Some(now) => ((deadline as i128 - now).max(0) / 1_000_000) as u64,
        None => 0,
    }
}

/// In a case's process, a close-on-exec copy of the suite's stdout for its VALUE and
/// WAIT records; -1 elsewhere.
static RECORD_FD: AtomicI32 = AtomicI32::new(-1);
/// In a case's process, the write end of the runner's pipe for the live panel's copy
/// of its records (close-on-exec, non-blocking); -1 elsewhere or with the strip off.
static LIVE_FD: AtomicI32 = AtomicI32::new(-1);
/// In a case's process, `SUITE <id> ` and the case's `<category>/<case>`.
static RECORD_NAMES: OnceLock<(String, String)> = OnceLock::new();
/// Records this process has printed.
static RECORDS: AtomicU32 = AtomicU32::new(0);

/// The most VALUE and WAIT records one case process prints.
pub const RECORDS_PER_CASE: u32 = 12;

/// Print one record for the running case; nothing outside a case or past the cap.
fn record(kind: &str, rest: &str) {
    let fd = RECORD_FD.load(Ordering::Relaxed);
    let Some((prefix, name)) = RECORD_NAMES.get() else { return };
    if fd < 0 || RECORDS.fetch_add(1, Ordering::Relaxed) >= RECORDS_PER_CASE {
        return;
    }
    let line = std::format!("{prefix}{kind} {name} {rest}");
    emit_to(Fd::from_raw(fd as u64), &line);
    let live = LIVE_FD.load(Ordering::Relaxed);
    if live >= 0 {
        // One write of a short line: whole or not at all, and never blocking the case.
        let _ = io::write(Fd::from_raw(live as u64), std::format!("{line}\n").as_bytes());
    }
}

/// Whether `text` is a record word: lowercase words of a-z and 0-9 joined by '-'.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// Report a measured quantity of the running case: `SUITE <id> VALUE <category>/<case>
/// <name>=<number><unit>`, with ` expect=<low>..<high><unit>` when `expect` gives the
/// range the case accepts. `name` is lowercase words joined by '-' and `unit` lowercase
/// letters (or empty); a record with any other name or unit is not printed. Report the
/// quantities a case asserts on, a few per case, never one per loop iteration.
pub fn value(name: &str, number: i64, unit: &str, expect: Option<(i64, i64)>) {
    if !is_word(name) || !unit.bytes().all(|b| b.is_ascii_lowercase()) {
        return;
    }
    let mut rest = std::format!("{name}={number}{unit}");
    if let Some((low, high)) = expect {
        let _ = write!(rest, " expect={low}..{high}{unit}");
    }
    record("VALUE", &rest);
}

/// Report that the running case starts waiting `ms` milliseconds on a sleep or timer:
/// `SUITE <id> WAIT <category>/<case> until=<ms> for=<ms>`, `until` being when the wait
/// should end in CLOCK_MONOTONIC milliseconds.
pub fn wait_for(ms: u64) {
    let now = monotonic_ns().map_or(0, |ns| (ns / 1_000_000) as u64);
    record("WAIT", &std::format!("until={} for={}", now + ms, ms));
}

/// A suite: its id, title and categories, run in order.
pub struct Suite {
    pub id: &'static str,
    pub title: &'static str,
    pub categories: &'static [Category],
    pub case_limit_ms: u64,
}

pub const fn suite(id: &'static str, title: &'static str, categories: &'static [Category]) -> Suite {
    Suite { id, title, categories, case_limit_ms: DEFAULT_CASE_LIMIT_MS }
}

impl Suite {
    /// The same suite with a different per-case time limit.
    pub const fn case_limit_ms(self, ms: u64) -> Suite {
        Suite { case_limit_ms: ms, ..self }
    }

    fn total(&self) -> usize { self.categories.iter().map(|category| category.cases.len()).sum() }

    /// Run every case in order, printing the serial lines and drawing the panel, then
    /// leave the final panel up and idle. Never returns.
    pub fn run(&self) -> ! {
        let total = self.total();
        let mut states: Vec<Vec<State>> = self.categories.iter()
            .map(|category| category.cases.iter().map(|_| State::Pending).collect())
            .collect();
        let mut screen = Screen::open();
        let title = std::format!("BREENIX / {} SUITE", self.title.to_uppercase());
        let subtitle = std::format!("suite {}: {} cases in {} categories", self.id, total, self.categories.len());
        let mut counts = Counts::default();
        let mut feed = Feed::default();
        let suite_start = monotonic_ns();

        emit(&std::format!("SUITE {} START cases={}", self.id, total));
        let mut done = 0;
        for (c, category) in self.categories.iter().enumerate() {
            for (k, case) in category.cases.iter().enumerate() {
                states[c][k] = State::Running;
                let progress = std::format!("Running {}/{} ({} of {})", category.id, case.id, done + 1, total);
                let name = std::format!("{}/{}", category.id, case.id);
                feed.start(&name);
                screen.draw(self, &title, &subtitle, &states, Verdict::Running(&progress), &feed);

                let outcome = run_case(case, self.case_limit_ms, self.id, &name, &mut screen, &mut feed);
                match &outcome {
                    Outcome::Pass { ms } => {
                        counts.passed += 1;
                        emit(&std::format!("SUITE {} CASE {} PASS ms={}", self.id, name, ms));
                    }
                    Outcome::Fail { ms, msg } => {
                        counts.failed += 1;
                        emit(&std::format!("SUITE {} CASE {} FAIL ms={} msg={}", self.id, name, ms, msg));
                    }
                    Outcome::Skip { msg } => {
                        counts.skipped += 1;
                        emit(&std::format!("SUITE {} CASE {} SKIP msg={}", self.id, name, msg));
                    }
                }
                states[c][k] = State::Done(outcome);
                done += 1;
            }
        }
        emit(&std::format!("SUITE {} DONE passed={} failed={} skipped={} total={}",
            self.id, counts.passed, counts.failed, counts.skipped, total));

        let summary = std::format!("{} passed, {} failed, {} skipped of {}",
            counts.passed, counts.failed, counts.skipped, total);
        let verdict = if counts.failed == 0 { Verdict::Passed(&summary) } else { Verdict::Failed(&summary) };
        let ms = ms_since(suite_start);
        feed.finish(std::format!("DONE {}.{} s", ms / 1000, ms % 1000 / 100));
        screen.draw(self, &title, &subtitle, &states, verdict, &feed);
        emit(&std::format!("SUITE_SEQUENCE READY {}", self.id));
        // A sequence replaces PID 1 with the next suite after this suite has
        // emitted its own DONE. Exec replaces the address space and closes only
        // FD_CLOEXEC descriptors; /tmp, PID allocation and page cache persist.
        // Single-suite disks have no sequence file and retain their final panel.
        if let Ok(sequence) = std::fs::read_to_string("/etc/breenix/suite-sequence") {
            let ids: Vec<&str> = sequence.trim_end().split(',').collect();
            // The x86 sequence gate freezes and captures this boundary before
            // acknowledging through the keyboard. Single-suite and ARM boots
            // have no sequence file and never wait for host input.
            let mut byte = [0u8; 1];
            loop {
                match io::read(Fd::STDIN, &mut byte) {
                    Ok(1) if byte[0] == b'\n' => break,
                    Ok(_) => { let _ = process::yield_now(); }
                    Err(error) => {
                        emit(&std::format!("SUITE_SEQUENCE FAIL acknowledgement: {:?}", error));
                        idle(&mut screen, &feed);
                    }
                }
            }
            if let Some(index) = ids.iter().position(|id| *id == self.id) {
                if let Some(next) = ids.get(index + 1) {
                    let path = std::format!("/sbin/suite-{}\0", next);
                    let error = process::exec(path.as_bytes()).unwrap_err();
                    emit(&std::format!("SUITE_SEQUENCE FAIL exec {}: {:?}", next, error));
                }
            }
        }
        idle(&mut screen, &feed)
    }
}

#[derive(Default)]
struct Counts {
    passed: usize,
    failed: usize,
    skipped: usize,
}

enum Outcome {
    Pass { ms: u64 },
    Fail { ms: u64, msg: String },
    Skip { msg: String },
}

enum State {
    Pending,
    Running,
    Done(Outcome),
}

/// Write one serial line with a single write, so it is never split, starting with
/// a newline so it always begins a line of its own.
fn emit(line: &str) { emit_to(Fd::STDOUT, line) }

fn emit_to(fd: Fd, line: &str) {
    let mut bytes = Vec::with_capacity(line.len() + 2);
    bytes.push(b'\n');
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        match io::write(fd, rest) {
            Ok(n) if n > 0 => rest = &rest[n..],
            _ => return,
        }
    }
}

/// The longest message a serial line carries.
const MSG_MAX: usize = 160;

/// A message fit for a serial line: one line, plain spacing, at most `MSG_MAX` bytes.
fn plain(msg: &str) -> String {
    let mut out = String::new();
    for word in msg.split_whitespace() {
        if !out.is_empty() { out.push(' '); }
        out.push_str(word);
    }
    if out.len() > MSG_MAX {
        let mut end = MSG_MAX;
        while !out.is_char_boundary(end) { end -= 1; }
        out.truncate(end);
    }
    if out.is_empty() { out.push_str("no reason given"); }
    out
}

fn monotonic_ns() -> Option<i128> { time::now_monotonic().ok().map(|ts| ts.as_nanos()) }

fn ms_since(start: Option<i128>) -> u64 {
    match (start, monotonic_ns()) {
        (Some(start), Some(now)) if now >= start => ((now - start) / 1_000_000) as u64,
        _ => 0,
    }
}

/// Run a case in this process (the case's forked child).
fn run_inline(case: &Case) -> Outcome {
    let start = monotonic_ns();
    let result = (case.run)();
    let ms = ms_since(start);
    match result {
        Ok(()) => Outcome::Pass { ms },
        Err(CaseError::Fail(msg)) => Outcome::Fail { ms, msg: plain(&msg) },
        Err(CaseError::Skip(msg)) => Outcome::Skip { msg: plain(&msg) },
    }
}

/// The child's report on the result pipe: `P <ms>`, `F <ms> <msg>` or `S <msg>`.
fn report(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Pass { ms } => std::format!("P {ms}"),
        Outcome::Fail { ms, msg } => std::format!("F {ms} {msg}"),
        Outcome::Skip { msg } => std::format!("S {msg}"),
    }
}

fn parse_report(text: &str) -> Option<Outcome> {
    let (kind, rest) = text.split_once(' ')?;
    match kind {
        "P" => Some(Outcome::Pass { ms: rest.trim().parse().ok()? }),
        "F" => {
            let (ms, msg) = rest.split_once(' ').unwrap_or((rest, ""));
            Some(Outcome::Fail { ms: ms.parse().ok()?, msg: plain(msg) })
        }
        "S" => Some(Outcome::Skip { msg: plain(rest) }),
        _ => None,
    }
}

/// Read the child's report once it has exited. Poll first: a grandchild may still
/// hold the write end, so a bare read could block.
fn read_report(reader: Fd) -> Option<Outcome> {
    let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
    if io::poll(&mut fds, 0).ok()? == 0 || fds[0].revents & poll_events::POLLIN == 0 {
        return None;
    }
    let mut buf = [0u8; 256];
    let n = io::read(reader, &mut buf).ok().filter(|&n| n > 0)?;
    parse_report(std::str::from_utf8(&buf[..n]).ok()?)
}

/// Polls in a row that may see the monotonic clock stand still before the suite
/// decides it has stopped. A poll is a waitpid, a clock read and a yield, so a
/// working clock moves long before this many.
const CLOCK_STALL_POLLS: u32 = 1_000_000;

/// How a wait for a child ended.
enum Waited {
    /// It exited, with this wait status.
    Exited(i32),
    /// It was still running when the limit passed, after this many ms.
    TimedOut(u64),
    /// The monotonic clock stopped moving, this many ms in.
    ClockStopped(u64),
    /// waitpid failed.
    Failed(Error),
}

/// How often the live strip is redrawn while the runner waits for a case.
const FRAME_MS: i128 = 66;

/// Wait up to `limit_ms` after `start` for `pid` to exit, calling `frame` about every
/// `FRAME_MS` meanwhile. Polls and yields rather than sleeping: a sleep's wake-up is
/// timed by the monotonic clock, so if that clock stopped the suite would never wake
/// to report it. Instead, polls that see the clock stand still are counted, and
/// `CLOCK_STALL_POLLS` of them end the wait.
fn wait_exit(pid: i32, start: Option<i128>, limit_ms: u64, frame: &mut dyn FnMut()) -> Waited {
    let mut last = monotonic_ns();
    let mut still = 0;
    let mut drawn = last;
    loop {
        let mut status = 0;
        match process::waitpid(pid, &mut status, WNOHANG) {
            Ok(done) if done.raw() as i32 == pid => return Waited::Exited(status),
            Err(error) => return Waited::Failed(error),
            _ => {}
        }
        let now = monotonic_ns();
        let ms = ms_since(start);
        if ms >= limit_ms {
            return Waited::TimedOut(ms);
        }
        if now.is_none() || now == last {
            still += 1;
            if still >= CLOCK_STALL_POLLS {
                return Waited::ClockStopped(ms);
            }
        } else {
            still = 0;
            last = now;
        }
        if let (Some(now), Some(then)) = (now, drawn) {
            if now - then >= FRAME_MS * 1_000_000 {
                frame();
                drawn = Some(now);
            }
        }
        let _ = process::yield_now();
    }
}

/// Kill a child that has run too long and reap it. Returns a note for the FAIL
/// message when it could not be reaped.
fn stop_child(pid: i32) -> &'static str {
    if signal::kill(pid, SIGKILL).is_err() {
        return "; SIGKILL failed, the case may still be running";
    }
    match wait_exit(pid, monotonic_ns(), 1000, &mut || {}) {
        Waited::Exited(_) => "",
        Waited::Failed(_) => "; waitpid failed after SIGKILL",
        Waited::TimedOut(_) | Waited::ClockStopped(_) => "; still running after SIGKILL",
    }
}

/// How long the runner waits for the processes a case left behind to die once killed.
const SWEEP_MS: u64 = 2000;
/// How often the sweep repeats its kill(-1), for a process forked after the last one.
const SWEEP_REKILL_MS: u64 = 250;

/// Kill and reap every process the case left behind. Running as PID 1, the runner
/// is by now the parent of each of them, or of an ancestor that is; as any other
/// PID, the case's orphans went to init and are left alone. Returns a note for the
/// case's result when some outlived SIGKILL.
fn sweep() -> Option<&'static str> {
    let mut status = 0;
    if !process::getpid().is_ok_and(|pid| pid.raw() == 1)
        || process::waitpid(-1, &mut status, WNOHANG).is_err()
    {
        return None;
    }
    let start = monotonic_ns();
    let mut killed: Option<u64> = None;
    loop {
        let ms = ms_since(start);
        if killed.map_or(true, |at| ms >= at + SWEEP_REKILL_MS) {
            let _ = signal::kill(-1, SIGKILL);
            killed = Some(ms);
        }
        if process::waitpid(-1, &mut status, WNOHANG).is_err() {
            return None;
        }
        if ms >= SWEEP_MS {
            return Some("processes it started outlived SIGKILL");
        }
        let _ = process::yield_now();
    }
}

/// `outcome`, failed with `note` added.
fn with_note(outcome: Outcome, note: &str) -> Outcome {
    match outcome {
        Outcome::Pass { ms } => Outcome::Fail { ms, msg: plain(&std::format!("the case passed, but {note}")) },
        Outcome::Fail { ms, msg } => Outcome::Fail { ms, msg: plain(&std::format!("{msg}; {note}")) },
        Outcome::Skip { msg } => Outcome::Fail { ms: 0, msg: plain(&std::format!("skipped ({msg}), but {note}")) },
    }
}

/// In the child: send stdout and stderr to /dev/null, so nothing the case prints
/// reaches the suite's serial lines.
fn silence_output() -> Result<(), Error> {
    let null = fs::open("/dev/null", fs::O_WRONLY)?;
    io::dup2(null, Fd::STDOUT)?;
    io::dup2(null, Fd::STDERR)?;
    if null != Fd::STDOUT && null != Fd::STDERR {
        let _ = io::close(null);
    }
    Ok(())
}

/// In the child: keep a close-on-exec copy of stdout for the case's VALUE and WAIT
/// records, named for the suite and the case.
fn keep_record_output(suite: &str, name: &str) {
    if let Ok(fd) = io::fcntl(Fd::STDOUT, io::fcntl_cmd::F_DUPFD_CLOEXEC, 3) {
        RECORD_FD.store(fd as i32, Ordering::Relaxed);
        let _ = RECORD_NAMES.set((std::format!("SUITE {suite} "), String::from(name)));
    }
}

/// Run a case in a forked child, giving it `limit_ms`, and keep the live strip up to
/// date while it runs.
fn run_case(case: &Case, limit_ms: u64, suite: &str, name: &str, screen: &mut Screen, feed: &mut Feed) -> Outcome {
    // The live strip's copy of the case's records, when the strip is on.
    let live = if screen.live_on() {
        io::pipe2(status_flags::O_CLOEXEC | status_flags::O_NONBLOCK).ok()
    } else {
        None
    };
    // Close-on-exec so a case that execs never leaves the report pipe open.
    let (reader, writer) = match io::pipe2(status_flags::O_CLOEXEC) {
        Ok(ends) => ends,
        Err(error) => {
            if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
            return Outcome::Fail { ms: 0, msg: plain(&std::format!("could not start the case: pipe failed: {error}")) };
        }
    };
    let start = monotonic_ns();
    match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(reader);
            if let Some((live_reader, live_writer)) = live {
                let _ = io::close(live_reader);
                LIVE_FD.store(live_writer.raw() as i32, Ordering::Relaxed);
            }
            let deadline = start.map_or(0, |start| (start + limit_ms as i128 * 1_000_000) as u64);
            CASE_DEADLINE_NS.store(deadline, Ordering::Relaxed);
            keep_record_output(suite, name);
            let outcome = match silence_output() {
                Ok(()) => run_inline(case),
                Err(error) => Outcome::Fail {
                    ms: 0,
                    msg: plain(&std::format!("could not send the case's output to /dev/null: {error}")),
                },
            };
            let line = report(&outcome);
            let _ = io::write(writer, line.as_bytes());
            process::exit(0)
        }
        Ok(ForkResult::Parent(pid)) => {
            let pid = pid.raw() as i32;
            let _ = io::close(writer);
            let live_reader = live.map(|(live_reader, live_writer)| {
                let _ = io::close(live_writer);
                live_reader
            });
            let mut frame = || {
                if let Some(fd) = live_reader { feed.drain(fd); }
                screen.frame(feed);
            };
            let outcome = match wait_exit(pid, start, limit_ms, &mut frame) {
                Waited::Exited(status) => exited(status, reader, ms_since(start)),
                Waited::TimedOut(ms) => {
                    let note = stop_child(pid);
                    Outcome::Fail { ms, msg: plain(&std::format!("timed out after {limit_ms} ms{note}")) }
                }
                Waited::ClockStopped(ms) => {
                    let note = stop_child(pid);
                    Outcome::Fail {
                        ms,
                        msg: plain(&std::format!("the monotonic clock stopped {ms} ms into the case; it was killed{note}")),
                    }
                }
                Waited::Failed(error) => {
                    let note = stop_child(pid);
                    Outcome::Fail { ms: ms_since(start), msg: plain(&std::format!("waitpid failed: {error}{note}")) }
                }
            };
            let _ = io::close(reader);
            if let Some(fd) = live_reader {
                feed.drain(fd);
                let _ = io::close(fd);
            }
            match sweep() {
                Some(note) => with_note(outcome, note),
                None => outcome,
            }
        }
        Err(error) => {
            let _ = io::close(reader);
            let _ = io::close(writer);
            if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
            Outcome::Fail { ms: 0, msg: plain(&std::format!("could not start the case: fork failed: {error}")) }
        }
    }
}

/// The outcome of a child that has exited: its report, or what happened to it.
fn exited(status: i32, reader: Fd, ms: u64) -> Outcome {
    let reported = read_report(reader);
    if process::wifexited(status) && process::wexitstatus(status) == 0 {
        if let Some(outcome) = reported { return outcome; }
        return Outcome::Fail { ms, msg: String::from("the case exited without a result") };
    }
    let msg = if process::wifsignaled(status) {
        std::format!("the case crashed: killed by signal {}", process::wtermsig(status))
    } else if process::wifexited(status) {
        std::format!("the case exited with status {} before reporting", process::wexitstatus(status))
    } else {
        String::from("the case ended abnormally")
    };
    Outcome::Fail { ms, msg }
}

/// Reap any children forever; the final panel stays on screen, its clocks ticking.
fn idle(screen: &mut Screen, feed: &Feed) -> ! {
    loop {
        let mut status = 0;
        while process::waitpid(-1, &mut status, WNOHANG).is_ok_and(|pid| pid.raw() != 0) {}
        screen.frame(feed);
        let _ = time::sleep_ms(250);
    }
}

/// A VALUE record as the live strip holds it.
struct Value {
    name: String,
    number: i64,
    unit: String,
    expect: Option<(i64, i64)>,
}

/// The most values the live strip keeps for one case.
const FEED_VALUES: usize = 32;

/// What the live strip shows: the running case, fed by its VALUE and WAIT records.
/// The last case's ended wait and its values stay up until the next case reports.
#[derive(Default)]
struct Feed {
    case: String,
    started: Option<i128>,
    /// The case `wait` and `values` came from.
    from: String,
    wait: Option<diagnostics::Wait>,
    values: Vec<Value>,
    /// Shown when no case runs.
    idle: String,
    /// The start of a record not yet ended by its newline.
    partial: Vec<u8>,
}

/// `text` as a whole number and its unit: `-12us` is (-12, "us"). A fraction, which
/// this runner's cases never print, is not taken.
fn number_unit(text: &str) -> Option<(i64, &str)> {
    let digits = text.char_indices().find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && c == '-')))
        .map_or(text.len(), |(i, _)| i);
    let unit = &text[digits..];
    if !unit.bytes().all(|b| b.is_ascii_lowercase()) { return None; }
    Some((text[..digits].parse().ok()?, unit))
}

impl Feed {
    fn start(&mut self, case: &str) {
        self.carry(String::from(case), String::new());
        self.started = monotonic_ns();
    }

    fn finish(&mut self, idle: String) {
        self.carry(String::new(), idle);
        self.started = None;
    }

    /// Move on to `case`, keeping what the last one measured, its wait only if it ended.
    fn carry(&mut self, case: String, idle: String) {
        if self.wait.is_some_and(|wait| wait.result.is_none()) {
            self.wait = None;
        }
        self.case = case;
        self.idle = idle;
        self.partial.clear();
    }

    /// The running case reports: drop what the last one left on screen.
    fn own(&mut self) {
        if self.from != self.case {
            self.from = self.case.clone();
            self.wait = None;
            self.values.clear();
        }
    }

    /// Read what the case has sent so far, without blocking.
    fn drain(&mut self, fd: Fd) {
        let mut buf = [0u8; 512];
        while let Ok(n) = io::read(fd, &mut buf) {
            if n == 0 { break; }
            self.partial.extend_from_slice(&buf[..n]);
        }
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            if let Ok(line) = std::str::from_utf8(&line) { self.take(line.trim()); }
        }
    }

    /// Take one record: `SUITE <id> VALUE <name> <n>=<number><unit> [expect=<low>..<high><unit>]`
    /// or `SUITE <id> WAIT <name> until=<ms> for=<ms>`.
    fn take(&mut self, line: &str) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["SUITE", _, "VALUE", _, measure, rest @ ..] => {
                let Some((name, quantity)) = measure.split_once('=') else { return };
                let Some((number, unit)) = number_unit(quantity) else { return };
                self.own();
                let expect = rest.first().and_then(|e| e.strip_prefix("expect="))
                    .and_then(|range| range.split_once(".."))
                    .and_then(|(low, high)| Some((low.parse().ok()?, number_unit(high)?.0)));
                if self.values.len() == FEED_VALUES { return; }
                self.values.push(Value { name: String::from(name), number, unit: String::from(unit), expect });
                if let Some(wait) = self.wait.as_mut().filter(|wait| wait.result.is_none()) {
                    wait.result = Some(self.values.len() - 1);
                }
            }
            ["SUITE", _, "WAIT", _, until, length] => {
                let until = until.strip_prefix("until=").and_then(|ms| ms.parse().ok());
                let length = length.strip_prefix("for=").and_then(|ms| ms.parse().ok());
                if let (Some(until_ms), Some(for_ms)) = (until, length) {
                    self.own();
                    self.wait = Some(diagnostics::Wait { until_ms, for_ms, result: None });
                }
            }
            _ => {}
        }
    }
}

/// The suite's view of the display, when there is one to draw on.
struct Screen {
    fb: Option<FrameBuf>,
    /// Whether the live strip is drawn (see `/etc/breenix/suite-live`).
    live: bool,
    /// Where the last full draw put the live strip.
    area: Option<LiveArea>,
}

impl Screen {
    /// Take the display and map all of it. Without a framebuffer the suite still
    /// runs and prints its serial lines; on a small one the panel draws what fits.
    fn open() -> Screen {
        let fb = (|| {
            let info = graphics::fbinfo().ok()?;
            if !(3..=4).contains(&info.bytes_per_pixel) {
                return None;
            }
            graphics::take_over_display().ok()?;
            let (ptr, width) = graphics::fb_mmap_pane().ok()?;
            if width == 0 { return None; }
            Some(unsafe {
                FrameBuf::from_raw(ptr, width as usize, info.height as usize,
                    (width * info.bytes_per_pixel) as usize, info.bytes_per_pixel as usize, info.is_bgr())
            })
        })();
        let off = std::fs::read_to_string("/etc/breenix/suite-live").is_ok_and(|text| text.trim() == "off");
        Screen { live: fb.is_some() && !off, fb, area: None }
    }

    fn live_on(&self) -> bool { self.live }

    /// The live strip's readings: the clocks now and the case's records.
    fn with_live<R>(feed: &Feed, f: impl FnOnce(&Live<'_>) -> R) -> R {
        let now = monotonic_ns().unwrap_or(0);
        let realtime = time::now_realtime().map_or(0, |ts| ts.as_nanos());
        let values: Vec<Reading<'_>> = feed.values.iter()
            .map(|value| Reading { name: &value.name, number: value.number, unit: &value.unit, expect: value.expect })
            .collect();
        f(&Live {
            check: &feed.case,
            monotonic_ns: now as i64,
            realtime_ns: realtime as i64,
            check_ms: feed.started.map_or(0, |start| ((now - start) / 1_000_000) as i64),
            wait: feed.wait,
            values: &values,
            values_from: &feed.from,
            idle: &feed.idle,
        })
    }

    /// Redraw only the live strip and flush it.
    fn frame(&mut self, feed: &Feed) {
        let (Some(fb), Some(area), true) = (self.fb.as_mut(), self.area, self.live) else { return };
        Self::with_live(feed, |live| diagnostics::draw_live(fb, area, live));
        let _ = graphics::take_over_display();
        let _ = graphics::fb_flush_rect(area.x, area.y, area.w, area.h);
    }

    fn draw(&mut self, suite: &Suite, title: &str, subtitle: &str, states: &[Vec<State>], verdict: Verdict<'_>,
        feed: &Feed) {
        let live_on = self.live;
        let Some(fb) = self.fb.as_mut() else { return; };
        let details: Vec<Vec<String>> = states.iter()
            .map(|cases| cases.iter().map(|state| match state {
                State::Done(Outcome::Pass { ms }) => {
                    let mut text = String::new();
                    let _ = write!(text, "{ms} ms");
                    text
                }
                State::Done(Outcome::Fail { msg, .. } | Outcome::Skip { msg }) => msg.clone(),
                State::Pending | State::Running => String::new(),
            }).collect())
            .collect();
        let checks: Vec<Vec<Check<'_>>> = suite.categories.iter().zip(states).zip(&details)
            .map(|((category, cases), details)| category.cases.iter().zip(cases).zip(details)
                .map(|((case, state), detail)| Check {
                    name: case.id,
                    state: match state {
                        State::Pending => CheckState::Pending,
                        State::Running => CheckState::Running,
                        State::Done(Outcome::Pass { .. }) => CheckState::Ok(detail),
                        State::Done(Outcome::Fail { .. }) => CheckState::Fail(detail),
                        State::Done(Outcome::Skip { .. }) => CheckState::Skip(detail),
                    },
                }).collect())
            .collect();
        let headings: Vec<String> = suite.categories.iter().map(|category| category.title.to_uppercase()).collect();
        let groups: Vec<Group<'_>> = headings.iter().zip(&checks)
            .map(|(title, checks)| Group { title, checks })
            .collect();
        self.area = Self::with_live(feed, |live| diagnostics::draw(fb, &Panel {
            title, subtitle, groups: &groups, output: &[], verdict, scored: true, live: live_on.then_some(live),
        }));
        // A case may have taken the display (its own take_over_display); take it
        // back, or the flush is refused and the panel stops updating.
        let _ = graphics::take_over_display();
        let _ = graphics::fb_flush();
    }
}
