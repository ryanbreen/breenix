"""Force CRC entry between native acceptance and adapter latch retirement."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing

ROOT = Path(__file__).resolve().parents[1]


class CrcOrdering(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_crc_waits_for_adapter_retirement_of_latch_and_ready_receipt(self):
        process = (ROOT / 'kernel/src/gpu/gk208/ce/process.rs').read_text()
        functions = '\n'.join(item(process, marker) for marker in [
            'pub(in crate::gk208) fn present(',
            'pub(in crate::gk208) fn present_complete(',
            'pub(in crate::gk208) fn capture_crc(',
        ]).replace('pub(in crate::gk208)', 'pub(crate)').replace('super::super::', '')
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        methods = '\n'.join(item(display, marker) for marker in [
            '    fn acquire(', '    fn observe(', 'pub(super) fn for_window_observer(',
            'pub(super) fn for_observation(',
        ])
        if 'pub(super) fn for_crc(' in display:
            methods += item(display, 'pub(super) fn for_crc(')
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        readback = item(core, 'fn read_back_impl(')
        start = readback.index('Some(if window {')
        selection = readback[start:readback.index('\n    } else {\n        None', start)]
        # Execute the CRC entry's production closure and its readback transition.
        entry = item(core, 'pub fn observe_screen_crc(')
        start = entry.index('let result = (||')
        entry = entry[start:entry.index('})();', start) + len('})();')] + '\nresult'
        adapter = (ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text()
        start = adapter.index('        if let Some((sent, sample)) = self.flipping.as_ref()')
        reap = adapter[start:adapter.index('        let Some(front)', start)]
        fixture = (ROOT / 'tests/fixtures/gt710_crc_ordering.rs').read_text()
        self.run_fixture(fixture.replace('// SESSION_FUNCTIONS', functions)
                         .replace('// TRANSITION_METHODS', methods.replace('pub(super)', 'pub(crate)'))
                         .replace('// SELECTION', selection).replace('// ENTRY', entry)
                         .replace('// REAP', reap))


if __name__ == '__main__':
    unittest.main()
