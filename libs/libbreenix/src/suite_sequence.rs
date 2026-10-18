//! Several suites in one boot (docs/suites/README.md, "Several suites in one boot").
//!
//! The launcher writes the suite ids, comma-separated, to [`LIST`] and boots the first
//! as PID 1. Each suite runs exactly as it does alone and prints the same lines. Around
//! them the runner prints `SUITE_SEQUENCE` records, and once a suite's DONE is out it
//! hands PID 1 to the next suite with exec:
//!
//! ```text
//! SUITE_SEQUENCE BEGIN suites=<id>,<id>,... baseline=<entries> memfree=<kB> heapfree=<kB>
//! SUITE_SEQUENCE RESET <id> removed=<n> changed=<n>[ paths=<path>,...] tcp=<n> tcpwait=<ms> memfree=<kB> heapfree=<kB>
//! SUITE_SEQUENCE RESET <id> FAIL <why>
//! SUITE_SEQUENCE READY <id>
//! SUITE_SEQUENCE STALLED <id> cases=<reported>/<total>
//! SUITE_SEQUENCE FAIL exec <id>
//! SUITE_SEQUENCE DONE suites=<n>
//! ```
//!
//! - Before the first suite's START the runner records the root filesystem as the boot
//!   left it in [`BASELINE`]: each path's type, permission bits, owner and size, with a
//!   content hash for symlinks and regular files of at most [`HASHED_BYTES`] and the
//!   modification time for larger files. `/proc` and `/dev` are left out, except the
//!   POSIX shared memory and message queue names in `/dev/shm` and `/dev/mqueue`.
//! - Before each later suite's START it puts the root back: what earlier suites created
//!   is removed, and a recorded path that is missing or differs is named in RESET (the
//!   runner kept no copy to restore it from). A RESET with `changed` above 0, or a
//!   RESET FAIL, means the suite did not start from the state its own boot gives it.
//! - Before each later suite's START it also waits, up to [`TCP_DRAIN_MS`], until
//!   `/proc/net/tcp` lists no listener or connection: an earlier suite's connections stay
//!   in TIME_WAIT for 60 seconds and keep their ports. RESET says how many were left
//!   (`tcp`, which must be 0) and how long it waited (`tcpwait`).
//! - Between suites PID 1 is the only process, descriptors 0-2 the only open ones, each
//!   signal has its default action and is unblocked, and no interval timer is armed.
//!   Exec gives the next suite a fresh address space and the environment the kernel gave
//!   the first. PID numbers, the page cache and kernel memory carry over. BEGIN and RESET report the free memory and kernel heap
//!   `/proc/meminfo` shows as each suite starts, so memory an earlier suite left
//!   allocated is visible.
//! - A watchdog ends a suite that makes no progress (no case starts or finishes) for
//!   [`STALL_MS`]: SIGALRM from ITIMER_REAL prints STALLED with the cases reported so far
//!   and hands off to the next suite. The handler uses system calls only, since the
//!   runner may have been anywhere when it fired. A suite whose kernel stops altogether
//!   cannot be moved past; the launcher's own stall watchdog ends that boot.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::string::String;
use std::sync::OnceLock;
use std::vec::Vec;

use crate::io;
use crate::process::{self, WNOHANG};
use crate::signal::{self, Itimerval, Sigaction, Timeval, ITIMER_REAL, SIGALRM, SIGKILL, SIGSTOP, SIG_SETMASK};
use crate::types::Fd;

/// The suite ids, comma-separated, in the order they run.
pub const LIST: &str = "/etc/breenix/suite-sequence";
/// The root filesystem as the first suite found it.
pub const BASELINE: &str = "/etc/breenix/suite-baseline";
/// How long a suite may go without starting or finishing a case. Below the launchers'
/// 120-second stall watchdog, and above the longest case allowance (25 s) plus its kill
/// and sweep.
pub const STALL_MS: u64 = 90_000;
/// Regular files up to this size are compared by content; larger ones by size and
/// modification time.
pub const HASHED_BYTES: u64 = 64 * 1024;
/// How many changed paths a RESET record names.
const PATHS_SHOWN: usize = 8;
/// The descriptors closed before exec: each one a process can hold (below the kernel's MAX_FDS).
const MAX_FDS: u64 = 4096;
/// How long the hand-off waits for killed processes to be reaped.
const REAP_MS: u64 = 2000;
/// How long a reset waits for the earlier suites' TCP connections to end: TIME_WAIT and
/// an orphan's FIN_WAIT2 each last 60 seconds.
pub const TCP_DRAIN_MS: u64 = 70_000;

/// Set once this suite is part of a sequence: cases' children then reset SIGALRM.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// When the watchdog may end the suite, in monotonic nanoseconds.
static STALL_AT_NS: AtomicU64 = AtomicU64::new(0);
/// Cases reported so far and the suite's total, for STALLED.
static REPORTED: AtomicU32 = AtomicU32::new(0);
static TOTAL: AtomicU32 = AtomicU32::new(0);
/// This suite's id, and the suites after it as (id, NUL-terminated binary path).
static ID: OnceLock<String> = OnceLock::new();
static AFTER: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();
/// The number of suites in the sequence.
static COUNT: AtomicU32 = AtomicU32::new(0);

/// This suite's place in the sequence the launcher wrote, or no place when the boot runs
/// it alone (no list, or a list without it).
pub fn read(id: &str) -> Option<(Vec<String>, usize)> {
    let text = std::fs::read_to_string(LIST).ok()?;
    let ids: Vec<String> = text.trim_end().split(',').map(String::from).collect();
    let index = ids.iter().position(|other| other == id)?;
    Some((ids, index))
}

/// Before the suite's START: record the baseline (the first suite) or put the root back
/// to it (any later one), then arm the watchdog for a suite of `total` cases.
pub fn prepare(ids: &[String], index: usize, total: usize) {
    let _ = ID.set(ids[index].clone());
    let _ = AFTER.set(ids[index + 1..].iter()
        .map(|id| (id.clone(), std::format!("/sbin/suite-{id}\0").into_bytes()))
        .collect());
    COUNT.store(ids.len() as u32, Ordering::Relaxed);
    TOTAL.store(total as u32, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    let _ = signal::sigaction(SIGALRM, Some(&Sigaction::new(on_alarm)), None);
    progress();
    if index == 0 {
        let entries = record_baseline();
        emit(&std::format!("SUITE_SEQUENCE BEGIN suites={} baseline={}{}", ids.join(","), entries, memory()));
    } else {
        emit(&match reset() {
            Ok(reset) => {
                // The wait is bounded below the watchdog's period, which restarts first.
                progress();
                reset.record(&ids[index]) + &drain_tcp() + &memory()
            }
            Err(why) => std::format!("SUITE_SEQUENCE RESET {} FAIL {why}", ids[index]),
        });
    }
    // The walk is not the suite's progress: its own watchdog period starts at START.
    progress();
}

/// Wait until `/proc/net/tcp` lists no listener or connection, up to [`TCP_DRAIN_MS`]:
/// ` tcp=<rows left> tcpwait=<ms>`, or an empty string when the kernel has no such file.
fn drain_tcp() -> String {
    let start = monotonic_ns();
    loop {
        let Ok(text) = std::fs::read_to_string("/proc/net/tcp") else { return String::new() };
        let rows = text.lines().skip(1).filter(|line| !line.trim().is_empty()).count();
        let waited = (monotonic_ns() - start) / 1_000_000;
        if rows == 0 || waited >= TCP_DRAIN_MS {
            return std::format!(" tcp={rows} tcpwait={waited}");
        }
        let _ = crate::time::sleep_ms(250);
    }
}

/// ` memfree=<kB> heapfree=<kB>` from `/proc/meminfo` (MemFree, KernelHeapFree), or
/// an empty string when it cannot be read.
fn memory() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else { return String::new() };
    let field = |name: &str| text.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok());
    match (field("MemFree:"), field("KernelHeapFree:")) {
        (Some(free), Some(heap)) => std::format!(" memfree={free}kB heapfree={heap}kB"),
        _ => String::new(),
    }
}

/// The suite started or finished a case: restart the watchdog. Counts a finished case
/// when `reported` is set.
pub fn progress_after(reported: usize) {
    REPORTED.store(reported as u32, Ordering::Relaxed);
    progress();
}

fn progress() {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    STALL_AT_NS.store(monotonic_ns() + STALL_MS * 1_000_000, Ordering::Relaxed);
    arm(STALL_MS);
}

/// In a case's child: SIGALRM back to its default action, as a case run alone finds it.
pub fn in_child() {
    if ACTIVE.load(Ordering::Relaxed) {
        let _ = signal::sigaction(SIGALRM, Some(&Sigaction::default()), None);
    }
}

/// After the suite's DONE: run the next suite in PID 1. Returns only after the last suite,
/// once DONE for the whole sequence is printed.
pub fn finish() {
    hand_off();
}

extern "C" fn on_alarm(_: i32) {
    // A case's child before it reset SIGALRM, or a SIGALRM some case sent PID 1, is
    // not the watchdog.
    if !process::getpid().is_ok_and(|pid| pid.raw() == 1) {
        return;
    }
    let now = monotonic_ns();
    let due = STALL_AT_NS.load(Ordering::Relaxed);
    if now < due {
        arm((due - now).div_ceil(1_000_000));
        return;
    }
    let mut line = Line::new();
    line.text("SUITE_SEQUENCE STALLED ").text(ID.get().map_or("", String::as_str)).text(" cases=")
        .number(REPORTED.load(Ordering::Relaxed) as u64).text("/").number(TOTAL.load(Ordering::Relaxed) as u64);
    line.emit();
    hand_off();
    // The last suite stalled: DONE is out, and PID 1 stays, reaping.
    loop {
        let mut status = 0;
        while process::waitpid(-1, &mut status, WNOHANG).is_ok_and(|pid| pid.raw() != 0) {}
        let _ = crate::time::sleep_ms(250);
    }
}

/// Clean PID 1 and exec the next suite that starts; system calls only, so it is safe
/// from the watchdog's handler. Returns after printing DONE when no suite is left.
fn hand_off() {
    arm(0);
    ACTIVE.store(false, Ordering::Relaxed);
    let after = AFTER.get().map_or(&[][..], Vec::as_slice);
    if let Some((next, _)) = after.first() {
        if !reap_all() {
            let mut line = Line::new();
            line.text("SUITE_SEQUENCE RESET ").text(next).text(" FAIL processes outlived SIGKILL");
            line.emit();
        }
        let default = Sigaction::default();
        for sig in 1..=64 {
            if sig != SIGKILL && sig != SIGSTOP {
                let _ = signal::sigaction(sig, Some(&default), None);
            }
        }
        for fd in 3..MAX_FDS {
            let _ = io::close(Fd::from_raw(fd));
        }
        let none = 0u64;
        let _ = signal::sigprocmask(SIG_SETMASK, Some(&none), None);
    }
    for (id, path) in after {
        // Exec returns only when the next suite could not start; try the one after it.
        let _ = process::exec(path);
        let mut line = Line::new();
        line.text("SUITE_SEQUENCE FAIL exec ").text(id);
        line.emit();
    }
    let mut line = Line::new();
    line.text("SUITE_SEQUENCE DONE suites=").number(COUNT.load(Ordering::Relaxed) as u64);
    line.emit();
}

/// Kill each process but PID 1 and reap them; false when some outlived SIGKILL.
fn reap_all() -> bool {
    let start = monotonic_ns();
    let mut killed_at = 0;
    loop {
        let now = monotonic_ns();
        if killed_at == 0 || now >= killed_at + 250_000_000 {
            let _ = signal::kill(-1, SIGKILL);
            killed_at = now;
        }
        let mut status = 0;
        match process::waitpid(-1, &mut status, WNOHANG) {
            Err(_) => return true,
            Ok(pid) if pid.raw() != 0 => continue,
            Ok(_) => {}
        }
        if now >= start + REAP_MS * 1_000_000 {
            return false;
        }
        let _ = process::yield_now();
    }
}

/// Arm ITIMER_REAL to fire once after `ms`, or disarm it with 0.
fn arm(ms: u64) {
    let value = Itimerval {
        it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
        it_value: Timeval { tv_sec: (ms / 1000) as i64, tv_usec: (ms % 1000 * 1000) as i64 },
    };
    let _ = signal::setitimer(ITIMER_REAL, &value, None);
}

fn monotonic_ns() -> u64 {
    crate::time::now_monotonic().map_or(0, |ts| ts.as_nanos() as u64)
}

fn emit(line: &str) {
    let mut out = Line::new();
    out.text(line);
    out.emit();
}

/// One serial record built without allocating, written with one write that starts
/// with a newline (as the suite's own records are). Longer text is cut.
struct Line {
    bytes: [u8; 512],
    len: usize,
}

impl Line {
    fn new() -> Line {
        let mut line = Line { bytes: [0; 512], len: 1 };
        line.bytes[0] = b'\n';
        line
    }

    fn text(&mut self, text: &str) -> &mut Line {
        let room = self.bytes.len() - 1 - self.len;
        let take = text.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        self
    }

    fn number(&mut self, mut value: u64) -> &mut Line {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.text(core::str::from_utf8(&digits[at..]).unwrap_or("0"))
    }

    fn emit(&mut self) {
        self.bytes[self.len] = b'\n';
        let mut rest = &self.bytes[..self.len + 1];
        while !rest.is_empty() {
            match io::write(Fd::STDOUT, rest) {
                Ok(n) if n > 0 => rest = &rest[n..],
                _ => return,
            }
        }
    }
}

/// One recorded path: its type (`d`, `f`, `l` or `o`), permission bits, owner, size,
/// and the content hash or modification time that stands for its content.
#[derive(PartialEq)]
struct Entry {
    kind: char,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    stamp: u64,
}

fn describe(path: &Path, meta: &std::fs::Metadata) -> Entry {
    let kind = if meta.is_dir() { 'd' } else if meta.file_type().is_symlink() { 'l' } else if meta.is_file() { 'f' } else { 'o' };
    let stamp = match kind {
        'f' if meta.size() <= HASHED_BYTES => std::fs::read(path).map_or(0, |bytes| fnv(&bytes)),
        'f' => (meta.mtime() as u64).wrapping_mul(1_000_000_000).wrapping_add(meta.mtime_nsec() as u64),
        'l' => std::fs::read_link(path).map_or(0, |target| fnv(target.as_os_str().as_encoded_bytes())),
        _ => 0,
    };
    // A directory's size and times change with its entries, which are compared themselves.
    let size = if kind == 'd' { 0 } else { meta.size() };
    Entry { kind, mode: meta.mode() & 0o7777, uid: meta.uid(), gid: meta.gid(), size, stamp }
}

/// 64-bit FNV-1a.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Whether the walk leaves `path` out: the kernel's pseudo-filesystems (but for the
/// IPC names, walked as roots of their own) and the baseline itself.
fn skipped(path: &str) -> bool {
    path == "/proc" || path == "/dev" || path == BASELINE
}

/// Visit each path under `/`, `/dev/shm` and `/dev/mqueue`; `visit` returns whether to
/// descend into a directory.
fn walk(visit: &mut dyn FnMut(&str, Option<std::fs::Metadata>) -> bool) {
    fn under(dir: &str, visit: &mut dyn FnMut(&str, Option<std::fs::Metadata>) -> bool) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut names: Vec<String> = entries.filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let path = if dir == "/" { std::format!("/{name}") } else { std::format!("{dir}/{name}") };
            if skipped(&path) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).ok();
            let is_dir = meta.as_ref().is_some_and(|meta| meta.is_dir());
            if visit(&path, meta) && is_dir {
                under(&path, visit);
            }
        }
    }
    for root in ["/", "/dev/shm", "/dev/mqueue"] {
        under(root, visit);
    }
}

/// Write the baseline; returns how many paths it holds.
fn record_baseline() -> usize {
    let mut text = String::new();
    let mut count = 0;
    walk(&mut |path, meta| {
        // A name that cannot be written on one line is not part of the image.
        if path.contains(['\n', '\r']) {
            return false;
        }
        let entry = meta.map_or(Entry { kind: '?', mode: 0, uid: 0, gid: 0, size: 0, stamp: 0 },
                                |meta| describe(Path::new(path), &meta));
        text.push_str(&std::format!("{} {:o} {} {} {} {:x} {path}\n",
            entry.kind, entry.mode, entry.uid, entry.gid, entry.size, entry.stamp));
        count += 1;
        true
    });
    match std::fs::write(BASELINE, text) {
        Ok(()) => count,
        Err(_) => 0,
    }
}

fn load_baseline() -> Result<BTreeMap<String, Entry>, String> {
    let text = std::fs::read_to_string(BASELINE).map_err(|error| std::format!("cannot read {BASELINE}: {error}"))?;
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(7, ' ');
        let mut next = || fields.next().ok_or_else(|| std::format!("malformed baseline line {line:?}"));
        let kind = next()?.chars().next().unwrap_or('?');
        let mode = u32::from_str_radix(next()?, 8).map_err(|_| std::format!("malformed baseline line {line:?}"))?;
        let uid = next()?.parse().map_err(|_| std::format!("malformed baseline line {line:?}"))?;
        let gid = next()?.parse().map_err(|_| std::format!("malformed baseline line {line:?}"))?;
        let size = next()?.parse().map_err(|_| std::format!("malformed baseline line {line:?}"))?;
        let stamp = u64::from_str_radix(next()?, 16).map_err(|_| std::format!("malformed baseline line {line:?}"))?;
        let path = next()?;
        entries.insert(String::from(path), Entry { kind, mode, uid, gid, size, stamp });
    }
    if entries.is_empty() {
        return Err(String::from("the baseline is empty"));
    }
    Ok(entries)
}

/// What a reset did: paths removed, and recorded paths it could not put back.
struct Reset {
    removed: usize,
    changed: Vec<String>,
}

impl Reset {
    fn record(&self, id: &str) -> String {
        let mut line = std::format!("SUITE_SEQUENCE RESET {id} removed={} changed={}", self.removed, self.changed.len());
        if !self.changed.is_empty() {
            let shown: Vec<&str> = self.changed.iter().take(PATHS_SHOWN).map(String::as_str).collect();
            line.push_str(&std::format!(" paths={}", shown.join(",")));
        }
        line
    }
}

/// Remove what earlier suites created; name recorded paths that are missing or differ.
fn reset() -> Result<Reset, String> {
    let baseline = load_baseline()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Reset { removed: 0, changed: Vec::new() };
    walk(&mut |path, meta| {
        let Some(recorded) = baseline.get(path) else {
            let removed = match &meta {
                Some(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
                _ => std::fs::remove_file(path),
            };
            match removed {
                Ok(()) => result.removed += 1,
                Err(_) => result.changed.push(std::format!("{path}(left)")),
            }
            return false;
        };
        seen.insert(String::from(path));
        let now = meta.map(|meta| describe(Path::new(path), &meta));
        if now.as_ref() != Some(recorded) {
            result.changed.push(String::from(path));
        }
        true
    });
    for path in baseline.keys() {
        // A path under a directory that changed type was not walked; the directory is named.
        if !seen.contains(path) && !result.changed.iter().any(|changed| path.starts_with(changed.as_str())) {
            result.changed.push(std::format!("{path}(missing)"));
        }
    }
    Ok(result)
}
