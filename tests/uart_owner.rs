//! The aarch64 UART owner (`kernel/src/uart_owner.rs`) under a stalled owner.
//!
//! Threads stand in for CPUs and a vector for the UART. A thread "stalls" by
//! sleeping inside the UART-ready wait, which is after its byte's ownership
//! check and before the byte, as a descheduled vCPU would. Time is scaled so
//! a waiter skips an owner after 10 ms rather than 100 ms. A race orders its
//! two CPUs by what each has written, not by how long each sleeps, so a host
//! that runs one thread late does not change which bytes interleave.

// uart_owner uses only the scanner's `contains`.
#[allow(dead_code)]
#[path = "../kernel/src/stage_lines.rs"]
mod stage_lines;
#[path = "../kernel/src/uart_owner.rs"]
mod uart_owner;

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use uart_owner::{Kind, Ownership, Port, State};

const STAGE: &str = "Kernel initialization complete";
const UDP: &str = "UDP: Delivered packet to socket on port 54321";
/// Sorted by last byte, as `stage_lines` scans them.
static MARKERS: [&str; 2] = [UDP, STAGE];

static STATE: State = State::new();
static WIRE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static OPEN: AtomicU8 = AtomicU8::new(0);
static NEXT_CPU: AtomicU32 = AtomicU32::new(1);
/// Bytes the second CPU of a race has written.
static SECOND_WROTE: AtomicUsize = AtomicUsize::new(0);
/// The second CPU's byte count that ends the first CPU's stall.
static WAKE_AFTER: AtomicUsize = AtomicUsize::new(0);
/// The second CPU waits before this byte of its own (1-based) until the first
/// CPU has written again; 0 for no wait.
static HOLD_AT: AtomicUsize = AtomicUsize::new(0);
/// The first CPU has written a byte since its stall ended.
static FIRST_RESUMED: AtomicBool = AtomicBool::new(false);
/// No race step waits longer than this for the other CPU.
const STEP_BOUND: Duration = Duration::from_secs(2);

thread_local! {
    static CPU: u32 = NEXT_CPU.fetch_add(1, Ordering::Relaxed);
    /// Bytes this thread has been cleared to write.
    static READIED: Cell<usize> = const { Cell::new(0) };
    /// Stall before this thread's byte with this number (1-based).
    static STALL_AT: Cell<usize> = const { Cell::new(0) };
    static STALL: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    /// How long the UART takes to accept each of this thread's bytes.
    static PER_BYTE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static STALLED: Cell<Option<&'static AtomicBool>> = const { Cell::new(None) };
    /// This thread is a race's second CPU.
    static SECOND: Cell<bool> = const { Cell::new(false) };
    /// This thread's stall has ended and it has not written since.
    static RESUMING: Cell<bool> = const { Cell::new(false) };
}

/// Sleep in 1 ms steps until `done` holds or `STEP_BOUND` passes.
fn wait_for(done: impl Fn() -> bool) {
    let deadline = Instant::now() + STEP_BOUND;
    while !done() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(done(), "race coordination timed out");
}

enum Test {}

impl Port for Test {
    type Writer = u8;

    fn state() -> &'static State {
        &STATE
    }

    fn cpu_tag() -> u32 {
        CPU.with(|cpu| *cpu)
    }

    fn counter() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    fn ticks(ns: u64) -> u64 {
        ns / 10
    }

    fn mask_interrupts() -> u64 {
        0
    }

    fn restore_interrupts(_: u64) {}

    fn ready() -> bool {
        let n = READIED.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        if n == STALL_AT.with(Cell::get) {
            if let Some(flag) = STALLED.with(Cell::get) {
                flag.store(true, Ordering::Release);
            }
            thread::sleep(STALL.with(Cell::get));
            // Stay stalled until the other CPU has skipped this one and
            // written its share.
            wait_for(|| SECOND_WROTE.load(Ordering::Acquire) >= WAKE_AFTER.load(Ordering::Acquire));
            RESUMING.with(|resuming| resuming.set(true));
        } else {
            if SECOND.with(Cell::get) && n == HOLD_AT.load(Ordering::Acquire) {
                wait_for(|| FIRST_RESUMED.load(Ordering::Acquire));
            }
            thread::sleep(PER_BYTE.with(Cell::get));
        }
        true
    }

    fn write(byte: u8, _: &Ownership<Self>) {
        WIRE.lock().unwrap().push(byte);
        if SECOND.with(Cell::get) {
            SECOND_WROTE.fetch_add(1, Ordering::Release);
        }
        if RESUMING.with(|resuming| resuming.replace(false)) {
            FIRST_RESUMED.store(true, Ordering::Release);
        }
    }

    fn capture(_: u8) {}

    fn lead(writer: u8, unit: &[u8]) -> &'static [u8] {
        let open = OPEN.load(Ordering::Relaxed);
        let ends_line = matches!(unit, [] | [b'\n', ..] | [b'\r', b'\n', ..]);
        if open == 0 || open == writer || ends_line {
            b""
        } else {
            b"\r\n"
        }
    }

    fn line_open() -> bool {
        OPEN.load(Ordering::Relaxed) != 0
    }

    fn wrote(writer: u8, byte: u8) {
        OPEN.store(if byte == b'\n' { 0 } else { writer }, Ordering::Relaxed);
    }

    fn markers() -> &'static [&'static str] {
        &MARKERS
    }
}

/// The wire is global: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn record(writer: u8, bytes: &[u8], kind: Kind) {
    uart_owner::write::<Test, false>(writer, bytes, kind, &mut Ownership::acquire());
}

/// CPU 1 writes `first`, stalling before its byte number `stall_at`. Once it
/// has stalled, CPU 2 writes `second` without per-byte sleeps. CPU 1 stays stalled for
/// at least 60 ms and until CPU 2 has skipped it and written `wake_after`
/// bytes, so CPU 2 is part way through `second` when CPU 1 wakes. With
/// `hold_at` nonzero, CPU 2 waits before its byte with that number until
/// CPU 1 has written again. Returns the wire's lines.
fn race(
    first: (&'static [u8], Kind),
    stall_at: usize,
    second: (&'static [u8], Kind),
    wake_after: usize,
    hold_at: usize,
) -> Vec<String> {
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    SECOND_WROTE.store(0, Ordering::Release);
    WAKE_AFTER.store(wake_after, Ordering::Release);
    HOLD_AT.store(hold_at, Ordering::Release);
    FIRST_RESUMED.store(false, Ordering::Release);
    let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let one = thread::spawn(move || {
        STALL_AT.with(|at| at.set(stall_at));
        STALL.with(|stall| stall.set(Duration::from_millis(60)));
        STALLED.with(|flag| flag.set(Some(stalled)));
        record(1, first.0, first.1);
    });
    wait_for(|| stalled.load(Ordering::Acquire));
    let two = thread::spawn(move || {
        SECOND.with(|second| second.set(true));
        PER_BYTE.with(|per| per.set(Duration::ZERO));
        record(2, second.0, second.1);
    });
    one.join().unwrap();
    two.join().unwrap();
    let wire = WIRE.lock().unwrap().clone();
    String::from_utf8(wire)
        .unwrap()
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

fn stage_line() -> &'static [u8] {
    Box::leak(format!("[ INFO] kernel: {STAGE}\n").into_bytes().into_boxed_slice())
}

#[test]
fn a_stale_byte_from_a_skipped_owner_does_not_leave_a_stage_line_broken() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let line = format!("[ INFO] kernel: {STAGE}");
    // CPU 1 wakes once CPU 2 is past "[ INFO] kernel: " (and the line break it
    // writes first), and CPU 2 waits a little later in its line for CPU 1's byte.
    let lines = race(
        (b"aaaaaaaaaaaaaaaaaaaa\n", Kind::Plain),
        5,
        (stage_line(), Kind::Plain),
        20,
        22,
    );
    // CPU 1's fifth byte, checked before CPU 2 skipped it, landed inside CPU 2's line.
    assert!(
        lines
            .iter()
            .any(|l| l.char_indices().any(|(at, byte)| {
                byte == 'a' && format!("{}{}", &l[..at], &l[at + 1..]) == line
            })),
        "the stalled CPU's byte never landed inside the stage line: {lines:?}"
    );
    assert!(lines.contains(&line), "no whole stage line: {lines:?}");
}

#[test]
fn a_stage_line_that_loses_its_ownership_is_written_again_whole() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let line = format!("[ INFO] kernel: {STAGE}");
    let lines = race(
        (stage_line(), Kind::Plain),
        12,
        (b"bbbbbbbbbbbbbbbbbbbb\n", Kind::Plain),
        5,
        0,
    );
    assert!(lines.contains(&line), "no whole stage line: {lines:?}");
}

#[test]
fn a_notice_stays_one_whole_line_when_its_owner_stalls_before_the_newline() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let notice: &'static [u8] = Box::leak(format!("{UDP}\n").into_bytes().into_boxed_slice());
    // Stall before the newline, the notice's last byte, while CPU 2 writes an
    // unterminated record.
    let lines = race((notice, Kind::Line), notice.len(), (b"bbbbbbbbbb", Kind::Plain), 5, 0);
    assert!(
        lines.iter().any(|l| l == UDP),
        "the notice is not a whole line of its own: {lines:?}"
    );
}

#[test]
fn a_notice_starts_its_own_line_after_the_same_writers_open_line() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    record(1, b"prompt> ", Kind::Plain);
    record(1, format!("{UDP}\n").as_bytes(), Kind::Line);
    let wire = String::from_utf8(WIRE.lock().unwrap().clone()).unwrap();
    assert_eq!(wire, format!("prompt> \r\n{UDP}\n"));
}

#[test]
fn an_ordinary_record_is_written_once_when_nothing_else_writes() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    record(1, stage_line(), Kind::Plain);
    let wire = String::from_utf8(WIRE.lock().unwrap().clone()).unwrap();
    assert_eq!(wire, format!("[ INFO] kernel: {STAGE}\n"));
}

#[test]
fn a_breadcrumb_does_not_wait_for_another_cpus_record() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (owned_tx, owned_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let _ownership = Ownership::<Test>::acquire();
        owned_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    owned_rx.recv().unwrap();
    assert!(Ownership::<Test>::try_acquire().is_none());
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(Ownership::<Test>::try_acquire().is_some());
}
