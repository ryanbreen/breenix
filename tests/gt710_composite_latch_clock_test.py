"""G33a: composite clocks follow native latching frames, not bookkeeping."""
from pathlib import Path
import unittest

import window_composite_clock_test as provider_test

ROOT = Path(__file__).resolve().parents[1]


class LatchClock(unittest.TestCase):
    def test_frame_periods_preserve_misses_slow_modes_and_wrap(self):
        path = ROOT / 'kernel/src/gpu/gk208/vblank_clock.rs'
        self.assertTrue(path.exists(), 'G33a native latching-vblank clock is absent')
        fixture = r'''
mod clock { SOURCE }
fn passes(frames: u64, elapsed: u64) -> bool {
    u128::from(frames) * 1_000_000_000 >= 60 * u128::from(elapsed)
}
fn main() {
    // The R240's EDID timing, and a deliberately slower raster.
    for (pixels, hz, expected) in [(1688, 108_000_000, true), (1690, 108_000_000, false)] {
        let pixels = pixels * 1066;
        for start in [0, 137, 65530] {
            let mut c = clock::VblankClock::new(start, 2_000_000_000, pixels, hz);
            let first = c.at(start);
            for n in 1..=1000u16 {
                let at = c.at(start.wrapping_add(n));
                assert_eq!(at - first, u64::from(n) * pixels * 1_000_000_000 / hz);
                assert_eq!(passes(u64::from(n), at - first), expected);
            }
        }
    }
    let mut c = clock::VblankClock::new(65530, 2_000_000_000, 1688*1066, 108_000_000);
    let first = c.at(65530);
    let last = c.at(147); // 153 vblanks, but only 152 completed composites.
    assert!(!passes(152, last-first), "one missed vblank must still fail");
    assert_eq!(c.at(147), last, "same frame cannot advance the clock");
}
'''.replace('SOURCE', path.read_text().replace('//!', '//'))
        provider_test.CompositeClock.run_rust(self, fixture)

    def test_provider_publishes_the_frames_time_despite_late_finish(self):
        provider = (ROOT / 'kernel/src/tracing/providers/window.rs').read_text()
        self.assertIn('presented_ns: u64', provider,
                      'count_composite still samples scene::finish bookkeeping time')
        # Use the production provider and counter stubs from its concurrency test,
        # with publication delayed by 5.7 ms (the failed capture discrepancy).
        source = (ROOT / 'tests/window_composite_clock_test.py').read_text()
        fixture = source.split("fixture = r'''", 1)[1].split("'''.replace", 1)[0]
        fixture = fixture[:fixture.index('fn main()')] + r'''
fn main() {
    window::init();
    per_cpu::preempt_disable(); per_cpu::preempt_enable();
    window::count_damage_rects(0); window::count_buffer_alloc();
    window::record_irq_masked(1); window::reset_irq_masked();
    CLOCK.store(5_700_000, Ordering::Relaxed);
    let presented = signal::monotonic_nanos();
    window::count_composite(7, presented);
    assert_eq!(window::composite_clock(), (1, presented));
    CLOCK.store(999_999_999, Ordering::Relaxed);
    window::count_composite(7, presented + PERIOD);
    assert_eq!(window::composite_clock(), (2, presented + PERIOD));
}
'''
        provider_test.CompositeClock.run_rust(self, fixture.replace('PROVIDER', provider.replace('//!', '//')))


if __name__ == '__main__':
    unittest.main()
