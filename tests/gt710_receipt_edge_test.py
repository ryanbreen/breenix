"""Exercise receipt timing when the raster edge and raw counter edge differ."""
from gt710_gr_live_unwind_test import item
from gt710_receipt_timing_test import ReceiptTiming, source
import unittest


class ReceiptEdge(unittest.TestCase):
    run_fixture = ReceiptTiming.run_fixture

    def test_presentation_armed_before_edge_cannot_reuse_the_previous_frame(self):
        native = source('kernel/src/gpu/gk208/native.rs')
        clock = source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        functions = item(native, 'fn presentation_sample(')
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
const WIDTH: u64 = 1688;
const HEIGHT: u64 = 1066;
const PIXELS: u64 = WIDTH * HEIGHT;
const HZ: u64 = 108_000_000;
const EPOCH: u64 = 2_000_000_000;
fn nanos(pixels: u64) -> u64 { (u128::from(pixels) * 1_000_000_000 / u128::from(HZ)) as u64 }
#[test]
fn latch_edge_is_independent_of_raw_counter_transition() {
    // The retained GT710_MODE raster; offset is a model input, not a
    // measurement of this card. rnndb documents separate VLINE/count fields.
    for start in [0u16, 65535] {
        for edge in [0u16, 1047, 1065] {
            for counter_delay_lines in [1u64, 2, 19] {
                let mut evo = native::Evo { head: 0,
                    clock: Some(vblank_clock::VblankClock::new(start, EPOCH, PIXELS, HZ)),
                    frame_pixels: PIXELS, pixel_hz: HZ, vblank_line: edge,
                    last_presentation: None };
                // Seed a completed frame, then arm a distinct presentation
                // immediately before the next physical latch.
                NOW.with(|v| v.set(EPOCH + nanos(100)));
                let mut regs = native::Registers { count: start,
                    line: u32::from(edge), pixel: 100 };
                evo.record_presentation(&regs, 7).unwrap();
                assert_eq!(evo.last_presentation, Some((7, EPOCH)));
                for frame in [1u64, 2, 4, 65536] {
                    let latch = EPOCH + nanos(frame * PIXELS);
                    let armed = latch - 1_000;
                    let mut first = None;
                    // Sample before and after the delayed raw-count edge.
                    for phase in [100, counter_delay_lines * WIDTH + 100, 500 * WIDTH] {
                        regs.count = start.wrapping_add(frame as u16)
                            .wrapping_sub(u16::from(phase < counter_delay_lines * WIDTH));
                        regs.line = ((u64::from(edge) + phase / WIDTH) % HEIGHT) as u32;
                        regs.pixel = (phase % WIDTH) as u32;
                        let sampled = latch + nanos(phase);
                        NOW.with(|v| v.set(sampled));
                        evo.record_presentation(&regs, 9).unwrap();
                        let (resource, receipt) = evo.last_presentation.unwrap();
                        assert_eq!(resource, 9);
                        assert!(receipt >= armed,
                            "receipt {receipt} predates armed {armed}, count={}, line={}",
                            regs.count, regs.line);
                        assert!(receipt <= sampled);
                        assert_eq!(receipt, latch, "receipt must name the latch, not the poll");
                        assert_eq!(*first.get_or_insert(receipt), receipt,
                            "raw count advancing within the frame changed its time");
                    }
                }
            }
        }
    }
}
'''.replace('CLOCK', clock).replace('FUNCTIONS', functions).replace('RECORD', record)
        self.run_fixture(fixture.replace('pub(in crate::gk208)', 'pub(crate)'))


if __name__ == '__main__':
    unittest.main()
