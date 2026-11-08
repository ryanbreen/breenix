"""Run console handback lock lifetime with the production trace exporter.

The mode-switch body and its native presentation diagnostic tail are extracted
unchanged; MMIO presentation is modeled. BREENIX_RECEIPT_SOURCE_REV selects an
older source tree for the same assertion.
"""
import unittest

from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class ConsoleTraceLock(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_handback_exports_after_unlock_with_a_detached_snapshot(self):
        process = timing.source('kernel/src/gpu/gk208/ce/process.rs')
        native = item(timing.source('kernel/src/gpu/gk208/native.rs'), 'fn present_inner(')
        tail = native[native.index('    super::display_evidence::record(regs, head, surface, &receipt);'):]
        tail = tail[:tail.index('    Ok(Some(receipt))')]
        mode = item(process, 'pub(crate) fn set_mode(').replace('super::super::', '')
        trace = timing.source('kernel/src/gpu/gk208/presentation_trace.rs').replace('//!', '//')
        # The harness itself belongs to this check, including when old production
        # sources are selected by the revision environment variable.
        from pathlib import Path
        fixture = (Path(__file__).parent / 'fixtures/gt710_console_trace.rs').read_text()
        fixture = fixture.replace('// MODE', mode).replace('// TRACE', trace).replace('// NATIVE_TAIL', tail)
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
