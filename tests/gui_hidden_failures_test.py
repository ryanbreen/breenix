"""A compositor self-check belongs to the case whose window contains it."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('scorer', ROOT / 'scripts/suite-verdict.py')
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)


class HiddenFailures(unittest.TestCase):
    def test_bwm_check_failure_overrules_the_active_case_pass(self):
        fixtures = ROOT / 'tests/fixtures/suite-verdict'
        manifest = json.loads((fixtures / 'manifest.json').read_text())
        manifest['id'] = 'gui-apps'
        clean = (fixtures / 'all-pass.txt').read_text().replace('SUITE fixture ', 'SUITE gui-apps ')
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial.txt'
            serial.write_text(clean)
            self.assertTrue(scorer.verdict(manifest, serial, [serial])[0])
            for diagnostic in (
                'UI_CHILD pid=1018 name=/usr/bin/bwm | [bwm] client frame check FAILED: window 104: pixel differs',
                '[bwm] client frame check FAILED: window 104: pixel differs',
            ):
                serial.write_text(clean.replace('SUITE gui-apps CASE first/two', diagnostic + '\nSUITE gui-apps CASE first/two'))
                ok, reason = scorer.verdict(manifest, serial, [serial])
                self.assertFalse(ok, reason)
                self.assertIn('CASE first/two', reason)
                self.assertIn('check FAILED', reason)
            # A preceding suite is outside these case windows.
            serial.write_text('UI_CHILD pid=1018 name=/usr/bin/bwm | [bwm] client frame check FAILED: preceding suite\n' + clean)
            self.assertTrue(scorer.verdict(manifest, serial, [serial])[0])


if __name__ == '__main__':
    unittest.main()
