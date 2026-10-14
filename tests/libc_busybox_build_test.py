"""Exercise per-applet selection from the builder's actual probe-processing code."""
import json
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class AppletSelection(unittest.TestCase):
    def select(self, core_links):
        record = {'core': {'links': core_links, 'missing': [] if core_links else ['startup']},
                  'compile_errors': [], 'objects_failed': [], 'applets': {
                      'cat': {'config': 'CAT', 'compiles': True, 'links': True, 'missing': []},
                      'optional': {'config': 'OPTIONAL', 'compiles': True, 'links': False, 'missing': ['missing_api']},
                      'dependent': {'config': 'DEPENDENT', 'compiles': True, 'links': True, 'missing': []},
                      'alias': {'config': 'OPTIONAL', 'compiles': True, 'links': True, 'missing': []}}}
        script = (ROOT / 'scripts/build-libc-busybox.sh').read_text()
        def code(marker):
            return re.search(r"<<'" + marker + r"'\n(.*?)\n" + marker + r"\n", script, re.S).group(1)
        with tempfile.TemporaryDirectory(prefix='breenix-applet-selection-') as temporary:
            directory = Path(temporary)
            probe, omit, links = [directory / name for name in ['applets.json', 'omit.config', 'links']]
            probe.write_text(json.dumps(record))
            config = directory / '.config'
            config.write_text('CONFIG_CAT=y\n# CONFIG_OPTIONAL is not set\n# CONFIG_DEPENDENT is not set\n')
            result = subprocess.run(['python3', '-c', code('PYSELECT'), str(probe), str(omit)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(probe.read_text()), record)
            self.assertEqual(omit.read_text(), 'CONFIG_OPTIONAL=n\n')
            subprocess.run(['python3', '-c', code('PYLINKS'), str(probe), str(links), str(config)], check=True)
            self.assertEqual(links.read_text(), 'cat\n')
            self.assertIn('unavailable applet optional', result.stderr)
            return result.stderr

    def test_optional_failure_is_recorded_without_a_false_core_diagnostic(self):
        self.assertNotIn('unresolved startup', self.select(True))

    def test_core_diagnostic_names_the_actual_startup_failure(self):
        self.assertIn('unresolved startup symbols: startup', self.select(False))


if __name__ == '__main__':
    unittest.main()
