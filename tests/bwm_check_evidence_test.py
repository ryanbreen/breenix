"""Check that bwm validation measurements survive redirected stdout."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class CheckEvidence(unittest.TestCase):
    def test_measurement_reaches_the_evidence_descriptor(self):
        source = (ROOT / 'userspace/programs/src/bwm/chrome.rs').read_text()
        measure = item(source, 'pub(super) fn measure_check')
        with tempfile.TemporaryDirectory(prefix='bwm-check-evidence-') as directory:
            temp = Path(directory)
            evidence = temp / 'evidence'
            evidence.touch()
            # Substitute the device path only; the production write is retained.
            measure = measure.replace('"/dev/screen-evidence"', f'"{evidence}"')
            program = 'mod chrome { ' + measure + ' }\nfn main() { assert_eq!(chrome::measure_check(|| 42), 42); }'
            (temp / 'check.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021']
            subprocess.run(compiler + [str(temp / 'check.rs'), '-o', str(temp / 'check')], check=True)
            with (temp / 'stdout').open('w') as output:
                subprocess.run([str(temp / 'check')], stdout=output, check=True)
            self.assertRegex(evidence.read_text(), r'^VALUE bwm-check-us [0-9]+\n$')
            self.assertEqual(evidence.read_text(), (temp / 'stdout').read_text())


if __name__ == '__main__':
    unittest.main()
