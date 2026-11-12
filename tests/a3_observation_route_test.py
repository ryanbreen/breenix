"""Run the latency case with an absent launcher route and deterministic completions."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ObservationRoute(unittest.TestCase):
    def test_observation_does_not_replace_latency_statistics_or_verdict(self):
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        source = (ROOT / 'tests/fixtures/a3_observation_route.rs').read_text()
        source = source.replace('LATENCY_BODY', item(cases, 'fn input_latency('))
        source = source.replace('OBSERVATION_BODY', item(cases, 'fn input_observation('))
        source = source.replace('DIAGNOSTIC_BODY', item(desktop, 'pub fn failure_diagnostic('))
        source = source.replace('UPLOAD_BODY', item(suite, 'pub fn upload_diagnostic('))
        source = source.replace('ROUTE_BODY', item(suite, 'fn evidence_route('))
        # Substitute only guest filesystem boundaries, keeping the production bodies.
        source = source.replace('std::fs::', 'crate::guest_fs::')
        run_rust(self, source)


if __name__ == '__main__':
    unittest.main()
