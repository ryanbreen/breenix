"""Bound successful window-manager serial diagnostics using production case bodies.

A populated observer ring models prior ordinary case activity, not a failure.
Only guest I/O, input delivery, and GPU completion are supplied by the host.
"""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]

class PassingExports(unittest.TestCase):
    def test_passing_case_exports_are_bounded(self):
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        source = (ROOT / 'tests/fixtures/a3_passing_exports.rs').read_text()
        for token, text, signature in [
            ('LATENCY_BODY', cases, 'fn input_latency('),
            ('WAIT_STRUCT', desktop, 'struct WaitObservation'),
            ('WAIT_IMPL', desktop, 'impl WaitObservation'),
            ('WAIT_DROP', desktop, 'impl Drop for WaitObservation'),
            ('OBSERVATION_BODY', cases, 'fn input_observation('),
            ('DIAGNOSTIC_BODY', desktop, 'pub fn failure_diagnostic('),
            ('UPLOAD_BODY', suite, 'pub fn upload_diagnostic('),
            ('ROUTE_BODY', suite, 'fn evidence_route('),
        ]:
            source = source.replace(token, item(text, signature) if signature in text else '')
        source = source.replace('std::fs::', 'crate::guest_fs::')
        run_rust(self, source)

if __name__ == '__main__':
    unittest.main()
