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


if __name__ == '__main__':
    unittest.main()
