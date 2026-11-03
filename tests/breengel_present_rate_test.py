"""Run the present-rate measurement source against a periodic compositor (G32).

The virtual clock and counter use integer nanoseconds; no wall-clock sleeps or
hardware are involved. Frame production keeps its own cadence, independent of
scanout, so the start phase covers arbitrary counter/clock boundaries.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class BreenGLPresentRate(unittest.TestCase):
    def test_periodic_compositor_across_start_phases(self):
        case = item((ROOT / 'userspace/programs/src/suite_breengel/window_cases/composition.rs').read_text(),
                    'fn composition_present_rate(')
        measurement = case[case.index('    let mut frame_times'):].rstrip()[:-1]
        fixture = r'''
use std::cell::RefCell;
type CaseResult = Result<(), breenix_suite::suite::CaseError>;
struct Model { ns: i128, centihz: i128, reported: Option<(i64, String)>, frames: u32, idle_tail: bool, idle_ns: i128 }
thread_local! {
    static MODEL: RefCell<Model> = const { RefCell::new(Model {
        ns: 0, centihz: 6002, reported: None, frames: 0, idle_tail: false, idle_ns: 0,
    }) };
}
fn count(ns: i128, centihz: i128) -> u64 {
    (ns * centihz / 100_000_000_000) as u64
}
fn counter(name: &str) -> Result<u64, String> {
    assert_eq!(name, "composites");
    MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if m.idle_tail && m.frames == 300 { m.ns += 230_000_000; m.idle_ns += 230_000_000; m.idle_tail = false; }
        m.ns += 10_000; // Polling and procfs reads take time, even between frames.
        Ok(count(m.ns - m.idle_ns, m.centihz))
    })
}
mod time {
    pub struct Instant(i128);
    impl Instant { pub fn as_nanos(&self) -> i128 { self.0 } }
    pub fn now_monotonic() -> Result<Instant, String> {
        Ok(Instant(super::MODEL.with(|m| m.borrow().ns)))
    }
}
fn now_ms() -> u64 { (time::now_monotonic().unwrap().as_nanos() / 1_000_000) as u64 }
fn since(start: u64) -> u64 { now_ms() - start }
trait Cite<T> { fn cite(self, section: &str) -> Result<T, String>; }
impl<T> Cite<T> for Result<T, String> {
    fn cite(self, _: &str) -> Result<T, String> { self }
}
struct Window;
impl Window {
    fn frame(&self, _: u32) -> Result<(), String> {
        MODEL.with(|m| { let mut m = m.borrow_mut(); m.ns += 13_000_000; m.frames += 1; });
        Ok(())
    }
}
mod breenix_suite { pub mod suite {
    pub type CaseError = String;
    pub fn timing(samples: &[u32], bound: u32) -> Result<(), String> {
        assert_eq!(samples.len(), 64);
        assert_eq!(bound, 16_667);
        Ok(())
    }
} }
fn ensure(ok: bool, _: &str, message: impl Into<String>) -> CaseResult {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn value(name: &str, rate: i64, unit: &str, bounds: Option<(i64, i64)>) {
    assert_eq!(name, "composites-per-second");
    assert_eq!(bounds.unwrap().1, i64::MAX);
    MODEL.with(|m| m.borrow_mut().reported = Some((rate, unit.to_owned())));
}
fn measure() -> CaseResult {
    let w = Window;
MEASUREMENT
}
// The original arbitrary-endpoint, millisecond, integer-floor calculation.
fn legacy(centihz: i128, phase: i128) -> bool {
    MODEL.with(|m| *m.borrow_mut() = Model { ns: phase, centihz, reported: None, frames: 0, idle_tail: false, idle_ns: 0 });
    let start = now_ms();
    let composites0 = counter("composites").unwrap();
    MODEL.with(|m| m.borrow_mut().ns += 270 * 13_000_000);
    let elapsed = since(start).max(1);
    let composed = counter("composites").unwrap() - composites0;
    let rate = composed * 1_000 / elapsed;
    rate >= 60
}
#[test]
fn legacy_flips_on_both_rates() {
    for centihz in [6002, 5994] {
        let mut passed = 0;
        for phase in 0..512 {
            passed += usize::from(legacy(centihz, phase * 100_000_000_000 / centihz / 512));
        }
        println!("legacy {centihz}/100 Hz: {passed}/512 phases pass");
        assert!(passed > 0 && passed < 512, "old formula must flip on both rates");
    }
}
#[test]
fn stopped_client_does_not_extend_window() {
    MODEL.with(|m| *m.borrow_mut() = Model {
        ns: 0, centihz: 6002, reported: None, frames: 0, idle_tail: true, idle_ns: 0,
    });
    assert!(measure().is_ok(), "idle after the final commit must not enter the measurement");
    MODEL.with(|m| assert!((m.borrow().reported.as_ref().unwrap().0 - 60_020).abs() <= 1));
}
#[test]
fn whole_periods_classify_every_phase() {
    let mut results = Vec::new();
    for centihz in [6002, 5994] {
        let mut passed = 0;
        let mut reports_exact = true;
        for phase in 0..512 {
            MODEL.with(|m| *m.borrow_mut() = Model {
                ns: phase * 100_000_000_000 / centihz / 512,
                centihz, reported: None, frames: 0, idle_tail: false, idle_ns: 0,
            });
            let result = measure();
            passed += usize::from(result.is_ok());
            MODEL.with(|m| {
                let m = m.borrow();
                let (rate, unit) = m.reported.as_ref().unwrap();
                // 10 us poll jitter over 3.5 s changes the report by at most 1 mHz.
                reports_exact &= unit == "mHz" && (*rate - centihz as i64 * 10).abs() <= 1;
            });
        }
        println!("production {centihz}/100 Hz: {passed}/512 phases pass");
        results.push((centihz, passed, reports_exact));
    }
    for (centihz, passed, reports_exact) in results {
        assert_eq!(passed, if centihz >= 6000 { 512 } else { 0 },
                   "G32: every start phase must classify {centihz}/100 Hz correctly");
        assert!(reports_exact, "VALUE must report the measured millihertz rate");
    }
}
'''.replace('MEASUREMENT', measurement)
        with tempfile.TemporaryDirectory(prefix='breengel-present-rate-') as tmp:
            source = Path(tmp) / 'check.rs'
            source.write_text(fixture)
            binary = Path(tmp) / 'check'
            target = 'x86_64-apple-darwin' if sys.platform == 'darwin' else 'x86_64-unknown-linux-gnu'
            args = ['rustc', '--edition=2021', '--target', target, '--test', '-Dwarnings',
                    str(source), '-o', str(binary)]
            if sys.platform == 'darwin':
                args = ['taskpolicy', '-b', *args]
            build = subprocess.run(args, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary), '--nocapture', '--test-threads=1'],
                                 capture_output=True, text=True)
            print(run.stdout, end='')
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()
