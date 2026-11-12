"""Run production recovery and focus waits with a deterministic host clock."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ElapsedReporting(unittest.TestCase):
    def test_wait_results_report_elapsed_after_wait_with_bounded_output(self):
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        source = (ROOT / 'tests/fixtures/wm_elapsed.rs').read_text()
        for token, text, signatures in [
            ('REPORT', desktop, ['pub fn elapsed(']),
            ('UNTIL', desktop, ['pub fn until(', 'pub fn until_from(']),
            ('SUPERVISOR', cases, ['pub fn supervisor_restarts(']),
            ('FOCUS', desktop, ['pub fn click(', 'pub fn focus(']),
            ('AWAIT', desktop, ['pub fn await_event(']),
            ('STOP', desktop, ['pub fn stop(']),
            ('DIAGNOSTIC', desktop, ['pub fn failure_diagnostic(']),
        ]:
            source = source.replace(token, '\n'.join(item(text, s) for s in signatures if s in text))
        source = source.replace('std::fs::', 'crate::guest_fs::')
        run_rust(self, source)


if __name__ == '__main__':
    unittest.main()
