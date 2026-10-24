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
static CAPTURED: Mutex<Vec<u8>> = Mutex::new(Vec::new());
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
    static IRQ_MASKED: Cell<bool> = const { Cell::new(false) };
    static CPU: u32 = NEXT_CPU.fetch_add(1, Ordering::Relaxed);
    /// Bytes this thread has been cleared to write.
    static READIED: Cell<usize> = const { Cell::new(0) };
    /// Stall before this thread's byte with this number (1-based).
    static STALL_AT: Cell<usize> = const { Cell::new(0) };
    static STALL: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    /// How long the UART takes to accept each of this thread's bytes.
    static PER_BYTE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static STALL_GATE: Cell<Option<&'static AtomicBool>> = const { Cell::new(None) };
    static STALLED: Cell<Option<&'static AtomicBool>> = const { Cell::new(None) };
    /// This thread is a race's second CPU.
    static SECOND: Cell<bool> = const { Cell::new(false) };
    /// This thread's stall has ended and it has not written since.
    static RESUMING: Cell<bool> = const { Cell::new(false) };
}

/// Yield until `done` holds or `STEP_BOUND` passes. Not a sleep: this port's
/// counter makes `STALL_NS` 10 ms (`ticks`), and at background QoS a 1 ms sleep
/// woke about 50 ms late, so a race step that waited by sleeping let the other
/// CPU skip it as stalled.
fn wait_for(done: impl Fn() -> bool) {
    let deadline = Instant::now() + STEP_BOUND;
    while !done() && Instant::now() < deadline {
        thread::yield_now();
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
        IRQ_MASKED.with(|masked| u64::from(masked.replace(true)))
    }

    fn restore_interrupts(saved: u64) { IRQ_MASKED.with(|masked| masked.set(saved != 0)); }

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
            if let Some(gate) = STALL_GATE.with(Cell::get) { wait_for(|| gate.load(Ordering::Acquire)); }
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

    fn capture(byte: u8) {
        CAPTURED.lock().unwrap().push(byte);
    }

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
    race_with_open(first, stall_at, second, wake_after, hold_at, 0)
}

fn race_with_open(
    first: (&'static [u8], Kind),
    stall_at: usize,
    second: (&'static [u8], Kind),
    wake_after: usize,
    hold_at: usize,
    open: u8,
) -> Vec<String> {
    WIRE.lock().unwrap().clear();
    OPEN.store(open, Ordering::Relaxed);
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
fn a_late_carriage_return_in_the_lead_does_not_prefix_a_suite_record() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let suite = b"SUITE breengel VALUE text/cache-evict-correct replacements=2targets\r\n";
    // The TTY needs CR-LF after the kernel's open line. Stall before that CR
    // reaches the UART, then let the kernel skip the owner and finish its
    // notices. The late CR must not count as a successfully written lead.
    let lines = race_with_open(
        (suite, Kind::Plain),
        1,
        (b"watchdog notice\n", Kind::Line),
        b"watchdog notice\n".len() + 2,
        0,
        2,
    );
    let records: Vec<_> = lines.iter().filter(|line| line.contains("SUITE ")).collect();
    assert_eq!(records.len(), 1, "unexpected records: {lines:?}");
    assert_eq!(records[0].as_bytes(), &suite[..suite.len() - 2]);
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
    // CPU 1's fifth byte, checked before CPU 2 skipped it, landed inside CPU 2's
    // line, including its prefix. Removing exactly that one byte must recover
    // the complete original line; unrelated corruption is not evidence.
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
fn a_bounded_record_and_kernel_line_stay_intact_when_either_owner_stalls() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut payload = vec![b'x'; 4096];
    payload[4095] = b'\n';
    let kernel = b"[signal] Process 42 (client) terminated by signal 15 (SIGTERM)\n";
    for fresh in [false, true] {
        for (first, second) in [(payload.as_slice(), kernel.as_slice()), (kernel.as_slice(), payload.as_slice())] {
            WIRE.lock().unwrap().clear();
            OPEN.store(0, Ordering::Relaxed);
            // This stall ends independently of the waiting writer. 60 ms exceeds
            // the port's scaled 10 ms takeover bound, including between the byte's
            // ownership check and its hardware write.
            WAKE_AFTER.store(0, Ordering::Relaxed);
            HOLD_AT.store(0, Ordering::Relaxed);
            let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
            thread::scope(|scope| {
                let one = scope.spawn(|| {
                    STALL_AT.with(|at| at.set(5));
                    STALL.with(|stall| stall.set(Duration::from_millis(60)));
                    STALLED.with(|flag| flag.set(Some(stalled)));
                    uart_owner::write_atomic::<Test, false>(1, first, fresh, &mut Ownership::acquire());
                });
                wait_for(|| stalled.load(Ordering::Acquire));
                let two = scope.spawn(|| uart_owner::write_atomic::<Test, false>(2, second, fresh, &mut Ownership::acquire()));
                one.join().unwrap();
                two.join().unwrap();
            });
            assert_eq!(&*WIRE.lock().unwrap(), &[first, second].concat());
        }
    }
}

#[test]
fn a_bounded_record_waits_for_a_skipped_breadcrumbs_in_flight_byte() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    WAKE_AFTER.store(0, Ordering::Relaxed);
    let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let one = thread::spawn(move || {
        STALL_AT.with(|at| at.set(5));
        STALL.with(|stall| stall.set(Duration::from_millis(60)));
        STALLED.with(|flag| flag.set(Some(stalled)));
        record(1, b"breadcrumb\n", Kind::Plain);
    });
    wait_for(|| stalled.load(Ordering::Acquire));
    let two = thread::spawn(|| {
        uart_owner::write_atomic::<Test, false>(2, b"SUITE record intact\n", false, &mut Ownership::acquire());
    });
    one.join().unwrap();
    two.join().unwrap();
    assert_eq!(&*WIRE.lock().unwrap(), b"bread\r\nSUITE record intact\ncrumb\n");
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

#[test]
fn a_capture_only_record_excludes_a_logger_even_after_its_ticket_is_skipped() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    CAPTURED.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    struct Record;
    impl std::fmt::Display for Record {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            out.write_str("census ")?;
            // Reentry must not wait on its own capture or append nested bytes.
            assert!(!uart_owner::capture_only::<Test>(format_args!("nested")));
            // The logger skips this stalled ticket while formatting is paused.
            // It reaches the UART while capture excludes its bytes.
            thread::spawn(|| {
                uart_owner::write::<Test, true>(
                    1,
                    b"logger\n",
                    Kind::Plain,
                    &mut Ownership::acquire(),
                );
            })
            .join()
            .unwrap();
            out.write_str("done\n")
        }
    }
    assert!(uart_owner::capture_only::<Test>(format_args!("{Record}")));
    assert_eq!(&*WIRE.lock().unwrap(), b"logger\n");
    assert_eq!(&*CAPTURED.lock().unwrap(), b"census done\n");
    uart_owner::write::<Test, true>(
        1,
        b"after\n",
        Kind::Plain,
        &mut Ownership::acquire(),
    );
    assert_eq!(&*CAPTURED.lock().unwrap(), b"census done\nafter\n");
}

#[test]
fn fatal_output_does_not_wait_for_another_cpu() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    WAKE_AFTER.store(0, Ordering::Relaxed);
    let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let gate: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let holder = thread::spawn(move || {
        STALL_AT.with(|at| at.set(5));
        STALLED.with(|flag| flag.set(Some(stalled)));
        STALL_GATE.with(|flag| flag.set(Some(gate)));
        uart_owner::write_atomic::<Test, false>(1, b"record paused at the UART\n", false, &mut Ownership::acquire());
    });
    wait_for(|| stalled.load(Ordering::Acquire));
    uart_owner::fatal::<Test>(b"fatal\n");
    assert_eq!(&*WIRE.lock().unwrap(), b"recofatal\n");
    gate.store(true, Ordering::Release);
    holder.join().unwrap();
}

#[test]
fn a_signal_logged_mid_record_returns_before_the_record_resumes_and_follows_it_whole() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    WAKE_AFTER.store(0, Ordering::Relaxed);
    let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let gate: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let payload = b"SERIAL_RECORD payload held mid-record\n";
    let signal = b"[signal] Process 42 (client) terminated by signal 15 (SIGTERM)\n";
    let holder = thread::spawn(move || {
        STALL_AT.with(|at| at.set(5));
        STALLED.with(|flag| flag.set(Some(stalled)));
        STALL_GATE.with(|flag| flag.set(Some(gate)));
        uart_owner::write_atomic::<Test, false>(1, payload, false, &mut Ownership::acquire());
    });
    wait_for(|| stalled.load(Ordering::Acquire));
    {
        let _deferred = Ownership::<Test>::acquire();
        assert!(!IRQ_MASKED.with(Cell::get), "deferred ownership left IRQs masked");
    }
    assert!(uart_owner::capture_only::<Test>(format_args!("capture during record\n")));
    assert!(!IRQ_MASKED.with(Cell::get), "capture-only caller retained the IRQ mask");
    uart_owner::write_atomic::<Test, true>(2, signal, false, &mut Ownership::acquire());
    assert_eq!(&*WIRE.lock().unwrap(), &payload[..4], "signal writer waited for the record or interrupted it");
    gate.store(true, Ordering::Release);
    holder.join().unwrap();
    assert_eq!(&*WIRE.lock().unwrap(), &[payload.as_slice(), signal.as_slice()].concat());
}

#[test]
fn deferred_ring_overflow_is_counted_and_reported_after_retained_lines() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    WIRE.lock().unwrap().clear();
    OPEN.store(0, Ordering::Relaxed);
    WAKE_AFTER.store(0, Ordering::Relaxed);
    let stalled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let gate: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let payload = b"record paused at UART\n";
    let line = b"[signal] deferred line\n";
    let holder = thread::spawn(move || {
        STALL_AT.with(|at| at.set(5));
        STALLED.with(|flag| flag.set(Some(stalled)));
        STALL_GATE.with(|flag| flag.set(Some(gate)));
        uart_owner::write_atomic::<Test, false>(1, payload, false, &mut Ownership::acquire());
    });
    wait_for(|| stalled.load(Ordering::Acquire));
    for _ in 0..65 {
        uart_owner::write_atomic::<Test, true>(2, line, false, &mut Ownership::acquire());
    }
    gate.store(true, Ordering::Release);
    holder.join().unwrap();
    let mut expected = payload.to_vec();
    for _ in 0..64 { expected.extend_from_slice(line); }
    expected.extend_from_slice(b"[serial] deferred record overflow: 1\n");
    assert_eq!(&*WIRE.lock().unwrap(), &expected);
}
