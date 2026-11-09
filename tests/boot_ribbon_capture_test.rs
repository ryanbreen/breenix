use graphics::boot_ribbon;
use std::sync::OnceLock;

pub mod time {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NOW_MS: AtomicU64 = AtomicU64::new(0);
    pub fn set_ms(ms: u64) {
        NOW_MS.store(ms, Ordering::Release);
    }
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        let ms = NOW_MS.load(Ordering::Acquire);
        (ms / 1000, ms % 1000 * 1_000_000)
    }
}

pub fn arch_interrupts_enabled() -> bool {
    true
}

const MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
const START: &str = "SUITE breengel START cases=3\n";
const FIRST: &str = "SUITE breengel CASE panel/first PASS ms=7\n";
const SECOND: &str = "SUITE breengel CASE panel/second PASS ms=11\n";

struct Capture {
    boot_prefix: String,
    timed_suite: String,
    start_ms: u64,
    last_ms: u64,
}

// One producer stream, initialized before any assertion; subsequent tests only
// read it. No reset shim or copied producer body is needed.
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let mut clock = 1007;
        let mut raw_len = 0;
        let mut emit = |line: &str| {
            assert!(line.ends_with('\n'));
            clock += 17;
            time::set_ms(clock);
            for byte in line.bytes() {
                boot_ribbon::byte(byte);
            }
            // Model the console consuming concurrently with serial production,
            // rather than manufacturing an undrained scanner overrun.
            boot_ribbon::drain();
            raw_len += line.len();
            clock
        };
        let noise = format!("{}\n", "x".repeat(99));
        let mut boot_prefix = noise.repeat(1500);
        boot_prefix.push_str(
            "[kernel] BOOT_MEMORY_READY\n[kernel] BOOT_STORAGE_READY\n\
             loader will emit BOOT_NEVER_READY\n[kernel] BOOT_USERS_READY\n",
        );
        for line in boot_prefix.split_inclusive('\n') {
            emit(line);
        }
        // Fourteen earlier suites push the raw stream past 1.2 MiB, even
        // without counting the writer's eight bytes per newline (#2824).
        for suite in 0..14 {
            emit(&format!("SUITE filler-{suite} START cases=1\n"));
            for _ in 0..820 {
                emit(&noise);
            }
            emit(&format!("SUITE filler-{suite} CASE work/one PASS ms=1\n"));
            emit(&format!("SUITE filler-{suite} DONE passed=1 failed=0 skipped=0 total=1\n"));
        }
        let start_ms = emit(START);
        let first_ms = emit(FIRST);
        let last_ms = emit(SECOND);
        assert!(raw_len > 1200 * 1024);
        Capture {
            boot_prefix,
            timed_suite: format!("{start_ms}|{START}{first_ms}|{FIRST}{last_ms}|{SECOND}"),
            start_ms,
            last_ms,
        }
    })
}

#[test]
fn serial_preserves_exact_pre_first_start_prefix_for_late_reader() {
    let capture = capture();
    let actual = boot_ribbon::serial().expect("late boot-prefix reader must not overflow");
    assert_eq!(actual, capture.boot_prefix);
    let records: Vec<bool> = boot_ribbon::records().lines().map(|line| {
        line.split_once('|').unwrap().0 == "1"
    }).collect();
    assert_eq!(stage_serial::reached(&actual, MARKERS), records);
}

#[test]
fn reached_guard_matches_independent_boot_prefix_parse() {
    let capture = capture();
    let expected: Vec<bool> = MARKERS.iter().map(|marker| {
        capture.boot_prefix.lines().any(|line| {
            let lower = line.to_ascii_lowercase();
            !["test_exec", "will emit", "->", "marker if"].iter().any(|s| lower.contains(s))
                && marker.split('|').any(|alternative| line.contains(alternative))
        })
    }).collect();
    // Green regression guard on the old sticky store; the other three tests
    // expose its overflow, as coordinator ledger line 596 specifies (#2824).
    assert_eq!(boot_ribbon::reached(), expected);
}

#[test]
fn serial_timed_starts_at_current_suite_with_exact_writer_stamps() {
    let capture = capture();
    let actual = boot_ribbon::serial_timed().expect("late suite reader must not overflow");
    assert_eq!(actual, capture.timed_suite);
}

#[test]
fn suite_status_tracks_two_of_three_breengel_cases_after_long_prefix() {
    let capture = capture();
    let actual = boot_ribbon::suite_status().expect("suite scanner must be available");
    assert_eq!(
        (actual.total(), actual.passed(), actual.failed(), actual.skipped(), actual.pending(),
         actual.current_case(), actual.elapsed(capture.start_ms + 65_000),
         actual.heading(capture.last_ms)),
        (3, 2, 0, 0, 1, Some("panel/second"), (1, 5), Some(String::from(
            "breengel: 2 passed · 0 failed · 1 pending of 3 · 0:00 · case: panel/second"))),
    );
}

#[test]
fn z_suite_status_advances_after_three_megabytes_drained() {
    capture();
    let line = format!("{}\n", "x".repeat(99));
    for _ in 0..(3 * 1024 * 1024 / line.len() + 1) {
        for byte in line.bytes() { boot_ribbon::byte(byte); }
        boot_ribbon::drain();
    }
    for line in ["SUITE latest START cases=1\n", "SUITE latest CASE work/done PASS ms=1\n"] {
        for byte in line.bytes() { boot_ribbon::byte(byte); }
        boot_ribbon::drain();
    }
    let status = boot_ribbon::suite_status().expect("suite scanner must be available");
    assert_eq!((status.total(), status.passed(), status.current_case()), (1, 1, Some("work/done")));
}
