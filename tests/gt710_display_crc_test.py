"""Model the production CRC collector and core methods without a GPU boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DisplayCrc(unittest.TestCase):
    def test_core_sequence_collection_and_record(self):
        driver = ROOT / 'kernel/src/gpu/gk208'
        self.assertTrue((driver / 'display_crc.rs').is_file(),
                        'the backend has no display CRC producer')
        fixture = (ROOT / 'tests/fixtures/gt710_display_crc.rs').read_text()
        fixture = fixture.replace('DRIVER', str(driver)).replace('ABI', str(ROOT / 'libs/gpu-abi/src'))
        with tempfile.TemporaryDirectory(prefix='display-crc-') as directory:
            source = Path(directory) / 'check.rs'
            binary = Path(directory) / 'check'
            source.write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test',
                                      '-Dwarnings', str(source), '-o', str(binary)],
                           cwd=ROOT, check=True)
            subprocess.run([str(binary), '--nocapture'], check=True, timeout=30)


if __name__ == '__main__':
    unittest.main()
