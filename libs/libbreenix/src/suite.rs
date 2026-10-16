//! Effort-suite runner.
//!
//! A suite binary (`/sbin/suite-<id>`) declares its categories and cases as a static
//! table and calls [`Suite::run`]. The runner prints the suite's serial lines on
//! stdout, one per line, exactly:
//!
//! ```text
//! SUITE <id> START cases=<total>
//! SUITE <id> CASE <category>/<case> PASS ms=<n> [sha256=<64 lowercase hex>]
//! SUITE <id> CASE <category>/<case> FAIL ms=<n> [sha256=<64 lowercase hex>] msg=<short plain reason>
//! SUITE <id> CASE <category>/<case> SKIP msg=<why>
//! SUITE <id> DONE passed=<p> failed=<f> skipped=<s> total=<n>
//! ```
//!
//! The optional sha256 field appears only when a PASS or FAIL has a verified judged frame.
//!
//! The runner draws a benchmark-style panel on the framebuffer (libgfx's diagnostics panel,
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
//! optional ` expect=<low>..<high><unit>`; [`wait_for`] prints
//! `SUITE <id> WAIT <category>/<case> until=<ms> for=<ms> result=<name>` as it starts a
//! wait, `until` in CLOCK_MONOTONIC milliseconds (the time since boot) and `result` the
//! VALUE that will say how the wait ended; and [`pulses`] prints
//! `SUITE <id> PULSES <category>/<case> every=<us> limit=<us> at=<us>,...`, the times a
//! periodic timer fired after it was armed. They are written, like the runner's own
//! lines, with one write that starts with a newline, through a copy of the suite's
//! stdout the case keeps (close-on-exec) when its own output goes to /dev/null. A case
//! process prints at most [`RECORDS_PER_CASE`] of them; the rest are dropped.
//!
//! The panel shows those records live: under the groups, a strip with the clocks
//! (CLOCK_MONOTONIC to the millisecond and CLOCK_REALTIME as wall time), a countdown
//! for the case's latest WAIT that snaps to the VALUE it names once the case reports
//! it, the case's latest VALUEs on gauges against the ranges they accept, and its
//! PULSES as a pulse train (libgfx's `diagnostics::draw_live`). The case sends each record to the runner as well, down a
//! pipe; the runner, not the case, redraws the strip about 15 times a second while it
//! waits for the case, so drawing never runs in the case's process or inside anything
//! it times. `/etc/breenix/suite-live` reading `off` turns the strip, and the pipe, off.
//!
//! A case that sets CLOCK_REALTIME must put it back. After every case the runner
//! checks: when CLOCK_REALTIME has moved more than [`REALTIME_SLACK_MS`] against
//! CLOCK_MONOTONIC since the case started, the runner sets it back and the case fails
//! saying so, whether it passed, failed or was killed.
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
//! This example is compile-checked on the host; running a suite requires Breenix.
//!
//! ```rust,no_run
//! use libbreenix::suite::{case, category, check, suite, CaseResult, Suite};
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

#[path = "suite_prerequisites.rs"]
pub mod prerequisites;

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::fmt::Write as _;
use std::string::String;
use std::vec::Vec;

use libgfx::diagnostics::{self, Check, CheckState, Group, Live, LiveArea, Panel, Pulses, Reading, Verdict};
use libgfx::framebuf::FrameBuf;

use crate::error::Error;
pub use crate::suite_spawn::{spawn, Keep, Run};
pub use crate::ui_clock;
use crate::fs;
use crate::io::{self, poll_events, status_flags, PollFd};
use crate::process::{self, ForkResult, WNOHANG};
use crate::signal::{self, SIGKILL};
use crate::types::{clock, Fd, Timespec};
use crate::{graphics, time};

/// Why a case did not pass.
#[derive(Debug)]
pub enum CaseError {
    /// The case ran and something was wrong.
    Fail(String),
    /// The case was not run, and why. No helper makes one: a case missing a
    /// prerequisite fails with it. The runner still reports this as a SKIP
    /// record, which suite acceptance rejects.
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

/// Pass if `condition` holds, otherwise fail with `msg`.
pub fn check(condition: bool, msg: &str) -> CaseResult {
    if condition { Ok(()) } else { fail(msg) }
}

/// Canonical pixels handed to the runner by a picture case, with their content hash.
/// The stage consumes this snapshot, never a later redraw of the case's surface.
pub struct JudgedFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub sha256: [u8; 32],
    pub expected: Option<Vec<u8>>,
    pub bound: usize,
}

struct PendingFrame { width: usize, height: usize, pixels: Vec<u8>, reference: Option<Vec<u8>>, bound: usize }
static PENDING_FRAME: std::sync::Mutex<Option<PendingFrame>> = std::sync::Mutex::new(None);

static JUDGED_ENABLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

static JUDGED_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static JUDGED_RUNNER: AtomicU64 = AtomicU64::new(0);

fn judged_path() -> String {
    let dir = if cfg!(target_os = "breenix") { "/dev/shm" } else { "/tmp" };
    std::format!("{dir}/.suite-judged-{}-{}", JUDGED_RUNNER.load(Ordering::Relaxed), JUDGED_SEQUENCE.load(Ordering::Relaxed))
}

/// Hand off the exact surface just judged, before it is modified or dropped.
/// Call after the comparison even when it failed. In-memory shared-file transport avoids a full-frame
/// pipe blocking the child while its parent waits for it to exit; no disk I/O is measured. Collection is outside
/// the measured case window; the parent verifies length, canonical form and SHA-256.
pub fn judged(frame: &FrameBuf) -> CaseResult { judged_with_reference(frame,None) }

/// Hand off an independently constructed reference with the exact judged surface.
pub fn judged_with_reference(frame:&FrameBuf,expected:Option<&FrameBuf>)->CaseResult {
    judged_with_bound(frame, expected, 0)
}
/// Snapshot the pixels and the pixel-difference bound actually used by the comparison.
pub fn judged_with_bound(frame:&FrameBuf,expected:Option<&FrameBuf>,bound:usize)->CaseResult {
    check(JUDGED_ENABLED.load(Ordering::Relaxed), "judged transport is not enabled for this suite")?;
    check(running_case().is_some(), "judged() called outside a suite case")?;
    let size = frame.width.checked_mul(frame.height).and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= i32::MAX as usize).ok_or("judged frame dimensions overflow")?;
    check(frame.width > 0 && frame.height > 0 && frame.width <= 16384 && frame.height <= 16384,
        "judged frame dimensions outside surface contract")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(size).map_err(|_| "judged frame allocation failed")?;
    for y in 0..frame.height { for x in 0..frame.width {
        let p = frame.get_pixel(x, y);
        pixels.extend_from_slice(&[p.b, p.g, p.r, 255]);
    } }
    let reference=if let Some(e)=expected {
        check((e.width,e.height)==(frame.width,frame.height),"reference dimensions differ from judged surface")?;
        let mut bytes=Vec::with_capacity(size);
        for y in 0..e.height {for x in 0..e.width {let p=e.get_pixel(x,y);bytes.extend_from_slice(&[p.b,p.g,p.r,255]);}}
        Some(bytes)
    }else{None};
    let mut pending = PENDING_FRAME.lock().unwrap();
    check(pending.is_none(), "a case may hand off only one judged frame")?;
    *pending = Some(PendingFrame { width: frame.width, height: frame.height, pixels, reference, bound });
    Ok(())
}
fn publish_judged() -> CaseResult {
    use std::io::Write;
    let Some(PendingFrame { width, height, pixels, reference, bound }) = PENDING_FRAME.lock().unwrap().take() else { return Ok(()); };
    let digest = libpixcheck::sha256(&pixels);
    let path = judged_path();
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        file.write_all(b"JDG3")?;
        file.write_all(&(width as u32).to_le_bytes())?;
        file.write_all(&(height as u32).to_le_bytes())?;
        file.write_all(&digest)?;
        file.write_all(&(bound as u64).to_le_bytes())?;
        file.write_all(&u32::from(reference.is_some()).to_le_bytes())?;
        file.write_all(&pixels)?;
        if let Some(bytes)=&reference {file.write_all(&libpixcheck::sha256(bytes))?;file.write_all(bytes)?;}
        drop(file);
        Ok(())
    };
    write().map_err(|error| CaseError::Fail(std::format!("judged frame handoff: {error}")))
}

fn collect_judged() -> Result<Option<JudgedFrame>, String> {
    use std::io::Read;
    if !JUDGED_ENABLED.load(Ordering::Relaxed) { return Ok(None); }
    let path = judged_path();
    let result = (|| {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(std::format!("judged frame open: {e}")),
        };
        let mut header = [0u8; 44];
        file.read_exact(&mut header).map_err(|_| "judged frame header truncated")?;
        if &header[..4] != b"JDG1" && &header[..4] != b"JDG2" && &header[..4] != b"JDG3" { return Err(String::from("judged frame header invalid")); }
        let width = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let height = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(String::from("judged frame dimensions invalid"));
        }
        let size = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("judged frame size overflow")?;
        let (bound, has_reference, extra) = if &header[..4] == b"JDG3" {
            let mut more = [0u8; 12]; file.read_exact(&mut more).map_err(|_| "judged metadata truncated")?;
            let bound = u64::from_le_bytes(more[..8].try_into().unwrap());
            let flag = u32::from_le_bytes(more[8..].try_into().unwrap());
            if flag > 1 || bound > width as u64 * height as u64 { return Err("judged bound or reference flag invalid".into()); }
            (bound as usize, flag == 1, 12)
        } else { (0, &header[..4] == b"JDG2", 0) };
        let total=44+extra+size+if has_reference{32+size}else{0};
        if file.metadata().map_err(|_| "judged frame metadata unavailable")?.len() != total as u64 {
            return Err(String::from("judged frame length differs from header"));
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(size).map_err(|_| "judged frame allocation failed")?;
        pixels.resize(size, 0);
        file.read_exact(&mut pixels).map_err(|_| "judged frame pixels truncated")?;
        if pixels.chunks_exact(4).any(|p| p[3] != 255) { return Err(String::from("judged frame is not canonical")); }
        let sha256 = libpixcheck::sha256(&pixels);
        if header[12..44] != sha256 { return Err(String::from("judged frame SHA-256 differs from handoff")); }
        let expected=if has_reference {
            let mut hash=[0u8;32];file.read_exact(&mut hash).map_err(|_|"reference hash truncated")?;
            let mut bytes=std::vec![0;size];file.read_exact(&mut bytes).map_err(|_|"reference pixels truncated")?;
            if bytes.chunks_exact(4).any(|p|p[3]!=255) || libpixcheck::sha256(&bytes)!=hash {return Err(String::from("reference snapshot invalid"));}
            Some(bytes)
        }else{None};
        Ok(Some(JudgedFrame { width, height, pixels, sha256, expected, bound }))
    })();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(std::format!("{}.new", judged_path()));
    result
}

/// Measured frame times for the timing spotlight (microseconds), never UI animation time.
#[derive(Clone)]
pub struct FrameTiming {pub frame_us:Vec<u32>,pub budget_us:u32}
pub fn timing(frame_us:&[u32],budget_us:u32)->CaseResult {
    check(JUDGED_ENABLED.load(Ordering::Relaxed) && running_case().is_some(),"timing outside BreenGL case")?;
    check(!frame_us.is_empty() && frame_us.len()<=64 && budget_us>0,"invalid frame timing samples")?;
    let mut text=std::format!("{budget_us}");for t in frame_us {let _=write!(text," {t}");}
    std::fs::write(std::format!("{}.time",judged_path()),text).map_err(|e|CaseError::Fail(e.to_string()))
}
fn collect_timing()->Result<Option<FrameTiming>,String> {
    if !JUDGED_ENABLED.load(Ordering::Relaxed){return Ok(None);}
    let path=std::format!("{}.time",judged_path());
    let bytes=match std::fs::read_to_string(&path){Ok(b)=>b,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(e.to_string())};
    let _=std::fs::remove_file(path);
    let mut words=bytes.split_whitespace();let budget_us=words.next().and_then(|v|v.parse().ok()).ok_or("invalid timing budget")?;
    let frame_us=words.map(str::parse).collect::<Result<Vec<u32>,_>>().map_err(|_|"invalid timing sample")?;
    if budget_us==0 || frame_us.is_empty() || frame_us.len()>64{return Err("invalid timing snapshot".into());}
    Ok(Some(FrameTiming{frame_us,budget_us}))
}

/// Input evidence captured at the handler that the case actually exercises.
#[derive(Clone, Default)]
pub struct InputTrace { pub points: Vec<libgfx::stage::InputPoint>, pub hit: String }
static INPUT_TRACE: std::sync::Mutex<Option<InputTrace>> = std::sync::Mutex::new(None);
fn input_label(hit:&str)->String {
    let mut label=hit.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.len()>192 {let mut end=189;while !label.is_char_boundary(end){end-=1;}label.truncate(end);label.push_str("...");}
    label
}
pub fn input_step(x:i32,y:i32,pressed:bool,hit:&str) {
    if !JUDGED_ENABLED.load(Ordering::Relaxed) {return;}
    let mut trace=INPUT_TRACE.lock().unwrap();
    let t=trace.get_or_insert_with(InputTrace::default);
    if t.points.len()<256 {t.points.push(libgfx::stage::InputPoint{x,y,pressed});}
    if !hit.is_empty() {t.hit=input_label(hit);}
}
pub fn input_key(hit:&str) {
    if !JUDGED_ENABLED.load(Ordering::Relaxed) {return;}
    INPUT_TRACE.lock().unwrap().get_or_insert_with(InputTrace::default).hit=input_label(hit);
}
fn publish_input()->CaseResult {
    let Some(t)=INPUT_TRACE.lock().unwrap().take() else{return Ok(());};
    let hit=if t.hit.is_empty(){std::format!("{}: no widget response",running_case().unwrap_or("input"))}else{t.hit};
    let mut body=std::format!("{hit}\n");
    for p in t.points {let _=writeln!(body,"{} {} {}",p.x,p.y,u8::from(p.pressed));}
    let digest=libpixcheck::sha256(body.as_bytes());
    let mut bytes=digest.to_vec();bytes.extend_from_slice(body.as_bytes());
    std::fs::write(std::format!("{}.input",judged_path()),bytes).map_err(|e|CaseError::Fail(e.to_string()))
}
pub(super) fn collect_input()->Result<Option<InputTrace>,String> {
    let path=std::format!("{}.input",judged_path());
    let bytes=match std::fs::read(&path){Ok(v)=>v,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(e.to_string())};
    let _=std::fs::remove_file(path);
    if bytes.len()<32 || libpixcheck::sha256(&bytes[32..])!=bytes[..32] {return Err("input snapshot hash differs".into());}
    let text=core::str::from_utf8(&bytes[32..]).map_err(|_|"input snapshot not text")?;
    let mut lines=text.lines();let hit=String::from(lines.next().ok_or("input target missing")?);
    let mut points=Vec::new();
    for line in lines {
        let words=line.split_whitespace().map(str::parse).collect::<Result<Vec<i32>,_>>().map_err(|_|"invalid input point")?;
        if words.len()!=3 || !(0..=1).contains(&words[2]) || words[..2].iter().any(|v|v.abs_diff(0)>1_000_000) || points.len()==256 {return Err("invalid input point".into());}
        points.push(libgfx::stage::InputPoint{x:words[0],y:words[1],pressed:words[2]==1});
    }
    Ok(Some(InputTrace{points,hit}))
}

fn publish_values() -> CaseResult {
    if !JUDGED_ENABLED.load(Ordering::Relaxed) { return Ok(()); }
    let lines = VALUE_LINES.lock().unwrap();
    let mut bytes = libpixcheck::sha256(lines.as_bytes()).to_vec();
    bytes.extend_from_slice(lines.as_bytes());
    std::fs::write(std::format!("{}.values", judged_path()), bytes).map_err(|e| CaseError::Fail(e.to_string()))
}
fn collect_values(suite: &str, case: &str) -> Result<Vec<Value>, String> {
    let path = std::format!("{}.values", judged_path());
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let _ = std::fs::remove_file(path);
    if bytes.len() < 32 || libpixcheck::sha256(&bytes[32..]) != bytes[..32] { return Err("VALUE snapshot hash differs".into()); }
    let lines = core::str::from_utf8(&bytes[32..]).map_err(|_| "VALUE snapshot is not text")?;
    let mut values = Vec::new();
    for line in lines.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let ["SUITE", id, "VALUE", name, quantity, rest @ ..] = words.as_slice() else { return Err("invalid VALUE record".into()); };
        if *id != suite || *name != case || values.len() >= RECORDS_PER_CASE as usize { return Err("VALUE record belongs to another case or exceeds the serial cap".into()); }
        let (name, quantity) = quantity.split_once('=').ok_or("invalid VALUE quantity")?;
        let (number, unit) = number_unit(quantity).ok_or("invalid VALUE number")?;
        let expect = match rest {
            [] => None,
            [range] => {
                let (lo, hi) = range.strip_prefix("expect=").and_then(|v|v.split_once("..")).ok_or("invalid VALUE bound")?;
                let lo = lo.parse().map_err(|_| "invalid VALUE lower bound")?;
                let (hi, bound_unit) = number_unit(hi).ok_or("invalid VALUE upper bound")?;
                if bound_unit != unit { return Err("VALUE bound unit differs".into()); }
                Some((lo, hi))
            }
            _ => return Err("invalid VALUE suffix".into()),
        };
        values.push(Value { name: name.into(), number, unit: unit.into(), expect });
    }
    Ok(values)
}

fn frame_hash(frame: Option<&JudgedFrame>) -> String {
    let mut text = String::new();
    if let Some(frame) = frame {
        text.push_str(" sha256=");
        for b in frame.sha256 { let _ = write!(text, "{b:02x}"); }
    }
    text
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
/// A named allowance overrides both this default and the suite's own limit.
pub const DEFAULT_CASE_LIMIT_MS: u64 = 10_000;

/// Named exceptions to case limits, checked by host tests against the stall deadline.
/// RTC edge selection can span sixteen seconds plus three starting edges;
/// allow reporter headroom without extending any other time-suite case.
/// The breengel workloads plan section 19 names (the 64-file font corpus, inflate fed at
/// every byte split, the PngSuite fixtures, 100 window churns and 300 timed commits) get
/// the workload plus the 1.5 s collection reserve, rounded up.
pub const NAMED_CASE_ALLOWANCES: &[(&str, &str, u64)] = &[
    ("time", "clocks/rate-counter", 25_000),
    ("breengel", "font-parser/corpus", 20_000),
    ("breengel", "inflate/byte-split", 20_000),
    ("breengel", "png-decode/independent-fixtures", 20_000),
    ("breengel", "windows/churn", 20_000),
    ("breengel", "composition/present-rate", 20_000),
];

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

/// In a case's process, the case's `<category>/<case>`.
static CASE_NAME: OnceLock<String> = OnceLock::new();

/// In a case's process, the running case's `<category>/<case>`; `None` outside a case.
/// Lets one function serve many cases that differ only by name, such as a case that
/// runs the program of the same name.
pub fn running_case() -> Option<&'static str> {
    CASE_NAME.get().map(String::as_str)
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
// Exact VALUE lines sent to serial, transported after the case timer stops.
static VALUE_LINES: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

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
    if kind == "VALUE" && JUDGED_ENABLED.load(Ordering::Relaxed) {
        let mut lines = VALUE_LINES.lock().unwrap();
        lines.push_str(&line);
        lines.push('\n');
    }
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
/// `SUITE <id> WAIT <category>/<case> until=<ms> for=<ms> result=<name>`, `until` being
/// when the wait should end in CLOCK_MONOTONIC milliseconds and `result` the name of
/// the VALUE the case reports to say how the wait ended. `result` is a record word, as
/// a VALUE's name is; a record with any other is not printed.
pub fn wait_for(ms: u64, result: &str) {
    if !is_word(result) {
        return;
    }
    let now = monotonic_ns().map_or(0, |ns| (ns / 1_000_000) as u64);
    record("WAIT", &std::format!("until={} for={} result={result}", now + ms, ms));
}

/// The most expiry times one PULSES record carries.
pub const PULSES_MAX: usize = 32;

/// Report when a periodic timer of `every_us` fired, each in microseconds after it was
/// armed, and how late an expiry may be, `limit_us`: `SUITE <id> PULSES
/// <category>/<case> every=<us> limit=<us> at=<us>,<us>,...`. The first [`PULSES_MAX`]
/// times are printed.
pub fn pulses(every_us: i64, limit_us: i64, at_us: &[i64]) {
    let mut rest = std::format!("every={every_us} limit={limit_us} at=");
    for (i, at) in at_us.iter().take(PULSES_MAX).enumerate() {
        let _ = write!(rest, "{}{at}", if i == 0 { "" } else { "," });
    }
    record("PULSES", &rest);
}

/// A suite: its id, title and categories, run in order.
pub struct Suite {
    pub id: &'static str,
    pub title: &'static str,
    pub categories: &'static [Category],
    pub case_limit_ms: u64,
    pub gui: bool,
}

pub const fn suite(id: &'static str, title: &'static str, categories: &'static [Category]) -> Suite {
    Suite { id, title, categories, case_limit_ms: DEFAULT_CASE_LIMIT_MS, gui:false }
}

impl Suite {
    /// Hand the display to each isolated case; draw only between measured windows.
    pub const fn gui(self)->Suite {Suite{gui:true,..self}}

    /// The same suite with a different per-case time limit.
    pub const fn case_limit_ms(self, ms: u64) -> Suite {
        Suite { case_limit_ms: ms, ..self }
    }

    fn total(&self) -> usize { self.categories.iter().map(|category| category.cases.len()).sum() }

    /// Run the cases in manifest order, or only argv's `--case category/case`,
    /// then leave the final panel up and idle. One-case START/DONE carry `only=`;
    /// a one-case run stops before any suite sequence and is not full-suite
    /// evidence. An unknown case or unusable argv ends in a failed DONE. Does not return.
    pub fn run(&self) -> ! {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let (only, usable_args) = match args.as_slice() {
            [] => (None, true),
            [flag, case] if flag == "--case" => (Some(case.as_str()), true),
            _ => (None, false),
        };
        let names: Vec<String> = self.categories.iter().flat_map(|category| {
            category.cases.iter().map(move |case| std::format!("{}/{}", category.id, case.id))
        }).collect();
        let error = if !usable_args {
            Some(std::format!("expected no arguments or --case <category>/<case>, found {:?}", args))
        } else {
            only.filter(|name| !names.iter().any(|valid| valid == name))
                .map(|name| std::format!("unknown case {}; valid ids: {}", name, names.join(", ")))
        };
        // A selection, or an argv that could not be read, counts as one case.
        let total = if only.is_some() || error.is_some() { 1 } else { self.total() };
        let suffix = only.map_or(String::new(), |name| std::format!(" only={name}"));
        let mut states: Vec<Vec<State>> = self.categories.iter()
            .map(|category| category.cases.iter().map(|_| State::Pending).collect())
            .collect();
        let mut screen = Screen::open(self.gui);
        let pace_ms = if self.gui {crate::suite_stage::init(self)} else {0};
        let title = std::format!("BREENIX / {} SUITE", self.title.to_uppercase());
        let subtitle = match only {
            Some(name) => std::format!("suite {}: one case {name} (not suite evidence)", self.id),
            None => std::format!("suite {}: {} cases in {} categories", self.id, self.total(), self.categories.len()),
        };
        let mut counts = Counts::default();
        let mut feed = Feed::default();
        let suite_start = monotonic_ns();

        let pace_suffix = if pace_ms > 0 { std::format!(" pace_ms={pace_ms}") } else { String::new() };
        emit(&std::format!("SUITE {} START cases={}{}{}", self.id, total, suffix, pace_suffix));
        if let Some(message) = &error {
            emit(&std::format!("SUITE {} ERROR {}", self.id, message));
            emit(&std::format!("SUITE {} DONE passed=0 failed=1 skipped=0 total={}{}", self.id, total, suffix));
            screen.draw(self, &title, &subtitle, &states, Verdict::Failed(message), &feed, only);
            idle(&mut screen, &feed);
        }
        let mut done = 0;
        for (c, category) in self.categories.iter().enumerate() {
            for (k, case) in category.cases.iter().enumerate() {
                let name = std::format!("{}/{}", category.id, case.id);
                if only.is_some_and(|selected| selected != name) { continue; }
                states[c][k] = State::Running;
                let progress = std::format!("Running {}/{} ({} of {})", category.id, case.id, done + 1, total);
                feed.start(&name);
                screen.draw(self, &title, &subtitle, &states, Verdict::Running(&progress), &feed, only);

                let limit_ms = NAMED_CASE_ALLOWANCES.iter()
                    .find(|&&(suite, case, _)| suite == self.id && case == name)
                    .map_or(self.case_limit_ms, |&(_, _, limit)| limit);
                let mut outcome = run_case(case, limit_ms, self.id, &name, &mut screen, &mut feed);
                if let Some(error) = screen.stage_error.take() { outcome = with_note(outcome, &error); }
                let picture_hash = frame_hash(screen.judged.as_ref());
                let make_line = |outcome: &Outcome| match outcome {
                    Outcome::Pass { ms } => std::format!("SUITE {} CASE {} PASS ms={}{}", self.id, name, ms, picture_hash),
                    Outcome::Fail { ms, msg } => std::format!("SUITE {} CASE {} FAIL ms={}{} msg={}", self.id, name, ms, picture_hash, msg),
                    Outcome::Skip { msg } => std::format!("SUITE {} CASE {} SKIP msg={}", self.id, name, msg),
                };
                let mut line = make_line(&outcome);
                if self.gui {
                    // Use the exact emitted VALUE records, not the best-effort live pipe.
                    let readings = match collect_values(self.id, &name) {
                        Ok(values) => values,
                        Err(error) => { outcome = with_note(outcome, &error); line = make_line(&outcome); Vec::new() }
                    };
                    let readings = readings.as_slice();
                    let fps=readings.iter().rev().find(|v|v.name=="composites-per-second").and_then(|v|u32::try_from(v.number).ok());
                    let input = collect_input();
                    let recorded = input.and_then(|input| crate::suite_stage::record(&line,&outcome,screen.judged.take(),screen.timing.take(),fps,readings,limit_ms,input));
                    let result = recorded.and_then(|()| crate::suite_stage::completed()).and_then(|()| if done + 1 == total {crate::suite_stage::finale()} else {Ok(())});
                    if let Err(error) = result {
                        outcome = with_note(outcome, &error);
                        line = make_line(&outcome);
                        screen.stage_result(crate::suite_stage::record(&line,&outcome,None,None,fps,readings,limit_ms,None));
                        screen.stage_result(crate::suite_stage::redraw());
                    }
                }
                match &outcome { Outcome::Pass { .. } => counts.passed += 1, Outcome::Fail { .. } => counts.failed += 1, Outcome::Skip { .. } => counts.skipped += 1 }
                emit(&line);
                states[c][k] = State::Done(outcome);
                done += 1;
            }
        }
        emit(&std::format!("SUITE {} DONE passed={} failed={} skipped={} total={}{}",
            self.id, counts.passed, counts.failed, counts.skipped, total, suffix));

        let summary = std::format!("{} passed, {} failed, {} skipped of {}",
            counts.passed, counts.failed, counts.skipped, total);
        let verdict = if counts.failed == 0 { Verdict::Passed(&summary) } else { Verdict::Failed(&summary) };
        let ms = ms_since(suite_start);
        feed.finish(std::format!("DONE {}.{} s", ms / 1000, ms % 1000 / 100));
        if !self.gui { screen.draw(self, &title, &subtitle, &states, verdict, &feed, only); }
        // A one-case boot stops here, before a sequence's next suite.
        if only.is_some() { idle(&mut screen, &feed); }
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

pub(super) enum Outcome {
    Pass { ms: u64 },
    Fail { ms: u64, msg: String },
    Skip { msg: String },
}

pub(super) enum State {
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
    let result = result.and(publish_judged()).and(publish_input()).and(publish_values());
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

/// How far CLOCK_REALTIME may move against CLOCK_MONOTONIC across a case before the
/// runner sets it back.
pub const REALTIME_SLACK_MS: i128 = 100;

/// CLOCK_REALTIME less CLOCK_MONOTONIC, in nanoseconds.
fn realtime_offset() -> Option<i128> {
    let real = time::now_realtime().ok()?.as_nanos();
    Some(real - monotonic_ns()?)
}

/// After a case: when CLOCK_REALTIME has moved more than REALTIME_SLACK_MS against
/// CLOCK_MONOTONIC since `before`, set it back and say so, and whether that took.
fn restore_realtime(before: Option<i128>) -> Option<String> {
    let before = before?;
    let moved = realtime_offset()? - before;
    if moved.abs() <= REALTIME_SLACK_MS * 1_000_000 {
        return None;
    }
    let target = monotonic_ns()? + before;
    let set = time::clock_settime(clock::REALTIME, &Timespec {
        tv_sec: target.div_euclid(1_000_000_000) as i64,
        tv_nsec: target.rem_euclid(1_000_000_000) as i64,
    });
    let after = realtime_offset().map(|offset| offset - before);
    let put_back = match (set, after) {
        (Ok(()), Some(after)) if after.abs() <= REALTIME_SLACK_MS * 1_000_000 => "the runner set it back",
        _ => "the runner could not set it back",
    };
    Some(std::format!("it left CLOCK_REALTIME {} ms off; {put_back}", moved / 1_000_000))
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
    screen.judged = None;screen.timing=None;
    // The live strip's copy of the case's records, when the strip is on.
    let live = if screen.live_on() || screen.gui {
        io::pipe2(status_flags::O_CLOEXEC | status_flags::O_NONBLOCK).ok()
    } else {
        None
    };
    let realtime = realtime_offset();
    // Close-on-exec so a case that execs never leaves the report pipe open.
    let (reader, writer) = match io::pipe2(status_flags::O_CLOEXEC) {
        Ok(ends) => ends,
        Err(error) => {
            if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
            return Outcome::Fail { ms: 0, msg: plain(&std::format!("could not start the case: pipe failed: {error}")) };
        }
    };
    JUDGED_RUNNER.store(process::getpid().map_or(0, |pid| pid.raw()) as u64, Ordering::Relaxed);
    JUDGED_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // A prior interrupted boot may have left this PID/sequence on a reused disk.
    JUDGED_ENABLED.store(screen.gui, Ordering::Relaxed);
    for path in [judged_path(), std::format!("{}.new", judged_path()), std::format!("{}.time",judged_path()), std::format!("{}.input",judged_path()), std::format!("{}.values",judged_path())].into_iter().filter(|_| screen.gui) {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                let _ = (io::close(reader),io::close(writer));
                if let Some((r,w)) = live { let _ = (io::close(r),io::close(w)); }
                return Outcome::Fail{ms:0,msg:std::format!("cannot clear judged transport: {e}")};
            }
        }
    }
    if screen.gui {
        screen.stage_result(crate::suite_stage::begin());
    }
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
            let _ = CASE_NAME.set(String::from(name));
            keep_record_output(suite, name);
            let ready = if screen.gui { graphics::take_over_display() } else { Ok(()) };
            let outcome = match ready.and_then(|()| silence_output()) {
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
            if screen.gui {screen.stage_result(crate::suite_stage::end());}
            let _ = io::close(reader);
            if let Some(fd) = live_reader {
                feed.drain(fd);
                let _ = io::close(fd);
            }
            let outcome = match sweep() {
                Some(note) => with_note(outcome, note),
                None => outcome,
            };
            let outcome = match collect_judged() {
                Ok(frame) => { screen.judged = frame; outcome }
                Err(note) => with_note(outcome, &note),
            };
            let outcome=match collect_timing(){Ok(timing)=>{screen.timing=timing;outcome},Err(note)=>with_note(outcome,&note)};
            match restore_realtime(realtime) {
                Some(note) => with_note(outcome, &note),
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
#[derive(Clone)]
pub(super) struct Value {
    pub(super) name: String,
    pub(super) number: i64,
    pub(super) unit: String,
    pub(super) expect: Option<(i64, i64)>,
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
    /// The VALUE that ends `wait`.
    wait_result: String,
    values: Vec<Value>,
    /// The case's PULSES: the programmed period, the lateness allowed and the expiry
    /// times, in microseconds.
    pulses: Option<(i64, i64, Vec<i64>)>,
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
            self.pulses = None;
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

    /// Take one record: `SUITE <id> VALUE <name> <n>=<number><unit> [expect=<low>..<high><unit>]`,
    /// `SUITE <id> WAIT <name> until=<ms> for=<ms> result=<n>` or
    /// `SUITE <id> PULSES <name> every=<us> limit=<us> at=<us>,...`.
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
                if name == self.wait_result {
                    if let Some(wait) = self.wait.as_mut().filter(|wait| wait.result.is_none()) {
                        wait.result = Some(self.values.len() - 1);
                    }
                }
            }
            ["SUITE", _, "WAIT", _, until, length, result] => {
                let until = until.strip_prefix("until=").and_then(|ms| ms.parse().ok());
                let length = length.strip_prefix("for=").and_then(|ms| ms.parse().ok());
                if let (Some(until_ms), Some(for_ms), Some(result)) = (until, length, result.strip_prefix("result=")) {
                    self.own();
                    self.wait = Some(diagnostics::Wait { until_ms, for_ms, result: None });
                    self.wait_result = String::from(result);
                }
            }
            ["SUITE", _, "PULSES", _, every, limit, at] => {
                let every = every.strip_prefix("every=").and_then(|us| us.parse().ok());
                let limit = limit.strip_prefix("limit=").and_then(|us| us.parse().ok());
                let at: Option<Vec<i64>> = at.strip_prefix("at=")
                    .map(|list| list.split(',').filter(|us| !us.is_empty()).map(|us| us.parse().ok()).collect())
                    .unwrap_or(None);
                if let (Some(every), Some(limit), Some(at)) = (every, limit, at) {
                    self.own();
                    self.pulses = Some((every, limit, at));
                }
            }
            _ => {}
        }
    }
}

/// The suite's view of the display, when there is one to draw on.
struct Screen {
    gui:bool,
    stage_error: Option<String>,
    judged: Option<JudgedFrame>,
    timing:Option<FrameTiming>,
    fb: Option<FrameBuf>,
    /// Whether the live strip is drawn (see `/etc/breenix/suite-live`).
    live: bool,
    /// Where the last full draw put the live strip.
    area: Option<LiveArea>,
}

impl Screen {
    /// Take the display and map all of it. Without a framebuffer the suite still
    /// runs and prints its serial lines; on a small one the panel draws what fits.
    fn open(gui:bool) -> Screen {
        if gui {
            let stage_error = graphics::take_over_display().and_then(|()| graphics::give_back_display()).err().map(|e| std::format!("initialize GUI display: {e}"));
            return Screen {gui,stage_error,live:false,fb:None,area:None,judged:None,timing:None};
        }
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
                    (width * info.bytes_per_pixel) as usize, info.bytes_per_pixel as usize, info.is_bgr()).ok()?
            })
        })();
        let off = std::fs::read_to_string("/etc/breenix/suite-live").is_ok_and(|text| text.trim() == "off");
        Screen { gui,stage_error:None,live: fb.is_some() && !off && !gui, fb, area: None, judged: None,timing:None }
    }

    fn stage_result(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            emit(&std::format!("GUI STAGE FAIL msg={}", plain(&error)));
            self.stage_error.get_or_insert(error);
        }
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
            pulses: feed.pulses.as_ref().map(|(every_us, limit_us, at_us)| Pulses { every_us: *every_us, limit_us: *limit_us, at_us }),
            values_from: &feed.from,
            idle: &feed.idle,
        })
    }

    /// Redraw only the live strip and flush it.
    fn frame(&mut self, feed: &Feed) {
        if self.gui {self.stage_result(crate::suite_stage::tick());return;}
        let (Some(fb), Some(area), true) = (self.fb.as_mut(), self.area, self.live) else { return };
        Self::with_live(feed, |live| diagnostics::draw_live(fb, area, live));
        let _ = graphics::take_over_display();
        let _ = graphics::fb_flush_rect(area.x, area.y, area.w, area.h);
    }

    fn draw(&mut self, suite: &Suite, title: &str, subtitle: &str, states: &[Vec<State>], verdict: Verdict<'_>,
        feed: &Feed, only: Option<&str>) {
        if self.gui {
            self.stage_result(crate::suite_stage::draw(states));
            return;
        }
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
                .filter(|((case, _), _)| only.is_none_or(|name| name == std::format!("{}/{}", category.id, case.id)))
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
            .filter(|(_, checks)| !checks.is_empty())
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

#[cfg(test)]
mod judged_transport_tests {
    use super::*;
    static TRANSPORT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_case_publishes_once_and_keeps_its_independent_reference() {
        let _serial = TRANSPORT.lock().unwrap();
        JUDGED_ENABLED.store(true,Ordering::Relaxed);
        JUDGED_RUNNER.store(std::process::id() as u64,Ordering::Relaxed);
        JUDGED_SEQUENCE.store(u64::MAX-1,Ordering::Relaxed);
        let _=std::fs::remove_file(judged_path());
        let _=CASE_NAME.set(String::from("transport/one"));
        let actual=FrameBuf::new_filled(2,1,libgfx::framebuf::Format::BGRX,libgfx::color::Color::rgb(1,2,3)).unwrap();
        let expected=FrameBuf::new_filled(2,1,libgfx::framebuf::Format::BGRX,libgfx::color::Color::BLACK).unwrap();
        judged_with_reference(&actual,Some(&expected)).unwrap();
        assert!(judged(&expected).is_err(),"a later handoff must not replace the first");
        publish_judged().unwrap();
        let frame=collect_judged().unwrap().unwrap();
        assert_eq!(frame.pixels,[3,2,1,255,3,2,1,255]);
        assert_eq!(frame.expected.unwrap(),[0,0,0,255,0,0,0,255]);
        JUDGED_ENABLED.store(false,Ordering::Relaxed);
        assert!(collect_judged().unwrap().is_none());
    }

    #[test]
    fn comparison_bound_survives_transport() {
        let _serial = TRANSPORT.lock().unwrap();
        JUDGED_ENABLED.store(true,Ordering::Relaxed);
        JUDGED_RUNNER.store(std::process::id() as u64,Ordering::Relaxed);
        JUDGED_SEQUENCE.store(u64::MAX-2,Ordering::Relaxed);
        let _=std::fs::remove_file(judged_path());
        let _=CASE_NAME.set(String::from("transport/bound"));
        let frame=FrameBuf::new(2,1,libgfx::framebuf::Format::BGRX).unwrap();
        judged_with_bound(&frame,Some(&frame),1).unwrap();
        assert!(!std::path::Path::new(&judged_path()).exists(),"snapshot hashing/publication must wait until the timer stops");
        publish_judged().unwrap();
        assert_eq!(collect_judged().unwrap().unwrap().bound,1);
        JUDGED_ENABLED.store(false,Ordering::Relaxed);
    }

    #[test]
    fn input_snapshot_keeps_handlers_and_rejects_changed_bytes() {
        let _serial = TRANSPORT.lock().unwrap();
        JUDGED_ENABLED.store(true,Ordering::Relaxed);
        JUDGED_RUNNER.store(std::process::id() as u64,Ordering::Relaxed);
        JUDGED_SEQUENCE.store(u64::MAX-3,Ordering::Relaxed);
        let path=std::format!("{}.input",judged_path());
        let _=std::fs::remove_file(&path);
        input_step(30,15,true,"button: Clicked");
        input_step(31,16,false,"");
        assert!(!std::path::Path::new(&path).exists(),"publication must wait for the case timer");
        publish_input().unwrap();
        let mut bytes=std::fs::read(&path).unwrap();
        let trace=collect_input().unwrap().unwrap();
        assert_eq!(trace.hit,"button: Clicked");
        assert_eq!(trace.points,[libgfx::stage::InputPoint{x:30,y:15,pressed:true},libgfx::stage::InputPoint{x:31,y:16,pressed:false}]);
        assert!(collect_input().unwrap().is_none());
        bytes[32]^=1;
        std::fs::write(&path,bytes).unwrap();
        assert!(collect_input().err().unwrap().contains("hash"));
        assert!(!std::path::Path::new(&path).exists());
        JUDGED_ENABLED.store(false,Ordering::Relaxed);
    }

    #[test]
    fn serial_value_snapshot_keeps_the_last_reading_and_checks_ownership_and_hash() {
        let _serial = TRANSPORT.lock().unwrap();
        JUDGED_ENABLED.store(true,Ordering::Relaxed);
        JUDGED_RUNNER.store(std::process::id() as u64,Ordering::Relaxed);
        JUDGED_SEQUENCE.store(u64::MAX-4,Ordering::Relaxed);
        let lines=(0..RECORDS_PER_CASE).map(|i|std::format!("SUITE breengel VALUE test/values count={i}px expect=0..12px\n")).collect::<String>();
        *VALUE_LINES.lock().unwrap()=lines.clone();
        publish_values().unwrap();
        let path=std::format!("{}.values",judged_path());
        let mut bytes=std::fs::read(&path).unwrap();
        assert_eq!(&bytes[32..],lines.as_bytes());
        let values=collect_values("breengel","test/values").unwrap();
        assert_eq!(values.len(),RECORDS_PER_CASE as usize);
        assert_eq!(values.last().unwrap().number,11);
        assert_eq!(values.last().unwrap().expect,Some((0,12)));
        std::fs::write(&path,&bytes).unwrap();
        assert!(collect_values("breengel","another/case").is_err());
        bytes[32]^=1;std::fs::write(&path,bytes).unwrap();
        assert!(collect_values("breengel","test/values").err().unwrap().contains("hash"));
        assert!(!std::path::Path::new(&path).exists());
        VALUE_LINES.lock().unwrap().clear();
        JUDGED_ENABLED.store(false,Ordering::Relaxed);
    }

    #[test]
    fn changed_truncated_and_noncanonical_snapshots_are_rejected() {
        let _serial = TRANSPORT.lock().unwrap();
        JUDGED_ENABLED.store(true, Ordering::Relaxed);
        JUDGED_RUNNER.store(std::process::id() as u64, Ordering::Relaxed);
        JUDGED_SEQUENCE.store(u64::MAX, Ordering::Relaxed);
        let pixels = [1u8, 2, 3, 255];
        let mut snapshot = Vec::from(&b"JDG1"[..]);
        snapshot.extend_from_slice(&1u32.to_le_bytes());
        snapshot.extend_from_slice(&1u32.to_le_bytes());
        snapshot.extend_from_slice(&libpixcheck::sha256(&pixels));
        snapshot.extend_from_slice(&pixels);
        std::fs::write(judged_path(), &snapshot).unwrap();
        let frame = collect_judged().unwrap().unwrap();
        assert_eq!((frame.width, frame.height), (1, 1));
        assert_eq!(frame.pixels, pixels);
        assert!(collect_judged().unwrap().is_none(), "collected evidence must be removed");

        let mut changed = snapshot.clone();
        changed[44] ^= 1;
        std::fs::write(judged_path(), &changed).unwrap();
        assert!(collect_judged().err().unwrap().contains("SHA-256"));
        assert!(!std::path::Path::new(&judged_path()).exists());

        std::fs::write(judged_path(), &snapshot[..47]).unwrap();
        assert!(collect_judged().err().unwrap().contains("length"));

        let mut noncanonical = snapshot;
        noncanonical[47] = 0;
        let digest = libpixcheck::sha256(&noncanonical[44..]);
        noncanonical[12..44].copy_from_slice(&digest);
        std::fs::write(judged_path(), &noncanonical).unwrap();
        assert!(collect_judged().err().unwrap().contains("canonical"));
    }
}

/// Validate the live runner stage against its stored CASE records and scanout.
pub fn showcase_check(name:&str)->CaseResult {crate::suite_stage::showcase_check(name)}

/// Same UI-clock boundary used for a stage fixture and the live stage.
pub fn render_stage_at_ui_time(fb: &mut FrameBuf, run: &mut libgfx::stage::Run<'_>) -> Result<(), libgfx::stage::StageError> {
    crate::suite_stage::render_at_ui_time(fb, run, 0)
}
