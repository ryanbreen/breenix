"""Run the native receipt producer with independent scanout/monotonic clocks.

BREENIX_RECEIPT_SOURCE_REV selects an old tree without changing the checkout.
The raster and monotonic clock are deterministic; no GPU or boot is involved.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    revision = os.environ.get('BREENIX_RECEIPT_SOURCE_REV')
    if revision:
        return subprocess.run(['git', 'show', f'{revision}:{path}'], cwd=ROOT,
                              check=True, capture_output=True, text=True).stdout
    return (ROOT / path).read_text()


class ReceiptTiming(unittest.TestCase):
    def run_fixture(self, fixture):
        with tempfile.TemporaryDirectory(prefix='receipt-timing-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings',
                       str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(path / 'check')], capture_output=True, text=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_completed_receipts_stay_in_the_monotonic_clock_domain(self):
        native = source('kernel/src/gpu/gk208/native.rs')
        clock = source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        functions = item(native, 'pub(in crate::gk208) fn vblank_counter(')
        if 'fn presentation_sample(' in native:
            functions += '\n' + item(native, 'fn presentation_sample(')
        record = item(native, 'pub(in crate::gk208) fn record_presentation(')
        fixture = r'''
use std::cell::Cell;
thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }
mod signal { pub fn monotonic_nanos() -> u64 { super::NOW.with(Cell::get) }
    use std::cell::Cell; }
mod vblank_clock { CLOCK }
mod native {
    pub struct Registers { pub count: u16, pub line: u32, pub pixel: u32 }
    impl Registers {
        pub fn read(&self, register: usize) -> u32 {
            match register {
                0x616340 => u32::from(self.count) << 16 | self.line,
                0x616344 => self.pixel,
                0x640414 => 1066 << 16 | 1688,
                _ => panic!("unexpected register {register:x}"),
            }
        }
    }
    pub struct Evo {
        pub head: u32,
        pub clock: Option<super::vblank_clock::VblankClock>,
        pub frame_pixels: u64,
        pub pixel_hz: u64,
        pub vblank_line: u16,
        pub last_presentation: Option<(u32, u64)>,
    }
    FUNCTIONS
    impl Evo { RECORD }
}
const PIXELS: u64 = 1688 * 1066;
const HZ: u64 = 108_000_000;
const EPOCH: u64 = 2_000_000_000;
fn exercise(drift_ppm: i64) {
    let start = 65530u16;
    let mut evo = native::Evo {
        head: 0, clock: Some(vblank_clock::VblankClock::new(start, EPOCH, PIXELS, HZ)),
        frame_pixels: PIXELS, pixel_hz: HZ, vblank_line: 1047, last_presentation: None,
    };
    assert_eq!((evo.frame_pixels, evo.pixel_hz, evo.vblank_line), (PIXELS, HZ, 1047));
    // Approximately ten minutes after the initial edge. A 100 ppm difference
    // puts the old projection about 60 ms on either side of the actual latch.
    for frame in [35_998u64, 35_999, 36_000, 36_002] {
        let elapsed = (u128::from(frame) * u128::from(PIXELS) * 1_000_000_000
            / u128::from(HZ)) as u64;
        let latch = (EPOCH as i64 + elapsed as i64
            + elapsed as i64 * drift_ppm / 1_000_000) as u64;
        let commit = latch - 2_000_000;
        let regs = native::Registers { count: start.wrapping_add(frame as u16),
            line: (1047 + 320) % 1066, pixel: 500 };
        let nominal_phase = (320 * 1688 + 500) * 1_000_000_000u64 / HZ;
        let actual_phase = (nominal_phase as i64
            + nominal_phase as i64 * drift_ppm / 1_000_000) as u64;
        let now = latch + actual_phase;
        NOW.with(|v| v.set(now));
        // The existing counter helper is also exercised, without changing its ABI.
        assert_eq!(native::vblank_counter(&regs, 0).unwrap(), (now, regs.count));
        evo.record_presentation(&regs, 7).unwrap();
        let (resource, receipt) = evo.last_presentation.unwrap();
        assert_eq!(resource, 7);
        assert!(receipt <= now, "receipt {receipt} is later than now {now}");
        assert!(receipt >= commit, "receipt {receipt} predates commit {commit}");
        assert!(receipt.abs_diff(latch) <= 1_000,
            "receipt {receipt} lost the hardware latch {latch}");
        // Same frame, later bookkeeping: the published presentation stays fixed.
        NOW.with(|v| v.set(now + 1_000_000));
        evo.record_presentation(&regs, 7).unwrap();
        assert_eq!(evo.last_presentation, Some((7, receipt)));
    }
}
#[test] fn faster_scanout_does_not_publish_future_receipts() { exercise(-100); }
#[test] fn slower_scanout_does_not_publish_receipts_before_the_commit() { exercise(100); }
'''.replace('CLOCK', clock).replace('FUNCTIONS', functions).replace('RECORD', record)
        fixture = fixture.replace('pub(in crate::gk208)', 'pub(crate)')
        self.run_fixture(fixture)

    def test_edge_calibration_rejects_a_preempted_poll_and_handles_raster_wrap(self):
        native = source('kernel/src/gpu/gk208/native.rs')
        clock = source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        functions = '\n'.join(item(native, marker) for marker in [
            'pub(in crate::gk208) fn vblank_counter(', 'fn presentation_sample('])
        methods = '\n'.join(item(native, marker) for marker in [
            'fn presentation_attempts(', 'fn start_clock(', 'pub(in crate::gk208) fn record_presentation('])
        fixture = r'''
use std::cell::Cell;
const EPOCH: u64 = 2_000_000_000;
const PIXELS: u64 = 1688 * 1066;
const HZ: u64 = 108_000_000;
thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }
mod signal { pub fn monotonic_nanos() -> u64 { super::NOW.with(Cell::get) }
    use std::cell::Cell; }
mod vblank_clock { CLOCK }
mod native {
    use super::*;
    pub struct Registers { pub preempt: bool, pub delayed: Cell<bool> }
    impl Registers {
        pub fn read(&self, register: usize) -> u32 {
            let mut now = NOW.with(Cell::get);
            if self.preempt && !self.delayed.get()
                && now >= EPOCH + PIXELS * 1_000_000_000 / HZ {
                self.delayed.set(true);
                now += 40_000; // skip more than two lines at the first edge
            }
            NOW.with(|v| v.set(now + 500));
            let pixels = u128::from(now - EPOCH) * u128::from(HZ) / 1_000_000_000;
            let frame = (pixels / u128::from(PIXELS)) as u16;
            let phase = (pixels % u128::from(PIXELS)) as u32;
            match register {
                0x616340 => u32::from(65535u16.wrapping_add(frame)) << 16
                    | (1047 + phase / 1688) % 1066,
                0x616344 => phase % 1688,
                0x640414 => 1066 << 16 | 1688,
                _ => panic!("unexpected register {register:x}"),
            }
        }
    }
    pub struct Evo {
        pub head: u32,
        pub clock: Option<super::vblank_clock::VblankClock>,
        pub frame_pixels: u64,
        pub pixel_hz: u64,
        pub vblank_line: u16,
        pub last_presentation: Option<(u32, u64)>,
    }
    FUNCTIONS
    impl Evo { METHODS }
    pub fn exercise(preempt: bool) {
        NOW.with(|v| v.set(EPOCH + (PIXELS - 1000) * 1_000_000_000 / HZ));
        let regs = Registers { preempt, delayed: Cell::new(false) };
        let mut evo = Evo { head: 0, clock: None, frame_pixels: PIXELS,
            pixel_hz: HZ, vblank_line: 0, last_presentation: None };
        evo.start_clock(&regs).unwrap();
        assert_eq!(evo.vblank_line, 1047, "a delayed poll chose a later line");
        let count = vblank_counter(&regs, 0).unwrap().1;
        assert_eq!(count, if preempt { 1 } else { 0 });
        evo.record_presentation(&regs, 9).unwrap();
        let expected = EPOCH + (if preempt { 2 } else { 1 }) * PIXELS * 1_000_000_000 / HZ;
        assert!(evo.last_presentation.unwrap().1.abs_diff(expected) < 1_000);
        // Impossible raster samples fail closed, without replacing the receipt.
        let receipt = evo.last_presentation;
        evo.frame_pixels -= 1;
        assert!(evo.record_presentation(&regs, 9).is_err());
        assert_eq!(evo.last_presentation, receipt);
    }
}
#[test] fn calibrates_nonzero_edge_line_across_counter_wrap() { native::exercise(false); }
#[test] fn ignores_preempted_edge_observation() { native::exercise(true); }
'''.replace('CLOCK', clock).replace('FUNCTIONS', functions).replace('METHODS', methods)
        self.run_fixture(fixture.replace('pub(in crate::gk208)', 'pub(crate)'))


if __name__ == '__main__':
    unittest.main()
