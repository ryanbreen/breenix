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
//! Each case runs in a forked child, so a case that crashes or runs past the suite's
//! time limit is reported as a FAIL saying so, and the suite goes on to the next case.
//! A case that cannot be forked runs in the suite's own process.
//!
//! The manifest `docs/suites/<id>.json` lists the same categories and cases in the same
//! order, with the same titles; a host test (`tests/suite_manifests.rs`) checks it
//! against the `category(...)` and `case(...)` calls in the suite's source.
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

use std::fmt::Write as _;
use std::string::String;
use std::vec::Vec;

use libgfx::diagnostics::{self, Check, CheckState, Group, Panel, Verdict};
use libgfx::framebuf::FrameBuf;

use crate::error::Error;
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

        emit(&std::format!("SUITE {} START cases={}", self.id, total));
        let mut done = 0;
        for (c, category) in self.categories.iter().enumerate() {
            for (k, case) in category.cases.iter().enumerate() {
                states[c][k] = State::Running;
                let progress = std::format!("Running {}/{} ({} of {})", category.id, case.id, done + 1, total);
                screen.draw(self, &title, &subtitle, &states, Verdict::Running(&progress));

                let outcome = run_case(case, self.case_limit_ms);
                let name = std::format!("{}/{}", category.id, case.id);
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
        screen.draw(self, &title, &subtitle, &states, verdict);
        idle()
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

/// Write one serial line with a single write, so it is never split.
fn emit(line: &str) {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        match io::write(Fd::STDOUT, rest) {
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

/// Run a case in the suite's own process.
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

/// Kill a child that has run too long and reap it.
fn stop_child(pid: i32) -> Option<&'static str> {
    if signal::kill(pid, SIGKILL).is_err() {
        return Some("; SIGKILL failed, the case may still be running");
    }
    for _ in 0..500 {
        let mut status = 0;
        match process::waitpid(pid, &mut status, WNOHANG) {
            Ok(done) if done.raw() as i32 == pid => return None,
            Err(_) => return Some("; waitpid failed after SIGKILL"),
            _ => {}
        }
        let _ = time::sleep_ms(1);
    }
    Some("; still running after SIGKILL")
}

/// Run a case in a forked child, giving it `limit_ms`.
fn run_case(case: &Case, limit_ms: u64) -> Outcome {
    // Close-on-exec so a case that execs never leaves the report pipe open.
    let Ok((reader, writer)) = io::pipe2(status_flags::O_CLOEXEC) else {
        return run_inline(case);
    };
    let start = monotonic_ns();
    match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(reader);
            let line = report(&run_inline(case));
            let _ = io::write(writer, line.as_bytes());
            process::exit(0)
        }
        Ok(ForkResult::Parent(pid)) => {
            let pid = pid.raw() as i32;
            let _ = io::close(writer);
            // A backstop in case the clock stops: each poll sleeps at least a millisecond.
            let max_polls = limit_ms.saturating_mul(4).max(1);
            let mut polls = 0;
            let outcome = loop {
                let mut status = 0;
                match process::waitpid(pid, &mut status, WNOHANG) {
                    Ok(done) if done.raw() as i32 == pid => break exited(status, reader, ms_since(start)),
                    Err(error) => {
                        let note = stop_child(pid).unwrap_or("");
                        break Outcome::Fail { ms: ms_since(start), msg: plain(&std::format!("waitpid failed: {error}{note}")) };
                    }
                    _ => {}
                }
                let ms = ms_since(start);
                polls += 1;
                if ms >= limit_ms || polls >= max_polls {
                    let note = stop_child(pid).unwrap_or("");
                    break Outcome::Fail { ms, msg: plain(&std::format!("timed out after {limit_ms} ms{note}")) };
                }
                let _ = time::sleep_ms(1);
            };
            let _ = io::close(reader);
            outcome
        }
        Err(_) => {
            let _ = io::close(reader);
            let _ = io::close(writer);
            run_inline(case)
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

/// Reap any children forever; the final panel stays on screen.
fn idle() -> ! {
    loop {
        let mut status = 0;
        while process::waitpid(-1, &mut status, WNOHANG).is_ok_and(|pid| pid.raw() != 0) {}
        let _ = time::sleep_ms(1000);
    }
}

/// The suite's view of the display, when there is one to draw on.
struct Screen {
    fb: Option<FrameBuf>,
}

impl Screen {
    /// Take the display and map all of it. Without a framebuffer (or one too small
    /// for the panel) the suite still runs and prints its serial lines.
    fn open() -> Screen {
        let fb = (|| {
            let info = graphics::fbinfo().ok()?;
            if info.width < 480 || info.height < 400 || !(3..=4).contains(&info.bytes_per_pixel) {
                return None;
            }
            graphics::take_over_display().ok()?;
            let (ptr, width) = graphics::fb_mmap_pane().ok()?;
            if width < 240 { return None; }
            Some(unsafe {
                FrameBuf::from_raw(ptr, width as usize, info.height as usize,
                    (width * info.bytes_per_pixel) as usize, info.bytes_per_pixel as usize, info.is_bgr())
            })
        })();
        Screen { fb }
    }

    fn draw(&mut self, suite: &Suite, title: &str, subtitle: &str, states: &[Vec<State>], verdict: Verdict<'_>) {
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
        diagnostics::draw(fb, &Panel { title, subtitle, groups: &groups, output: &[], verdict, scored: true });
        let _ = graphics::fb_flush();
    }
}
