"""Check clear's exact stdout bytes using a native build."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ClearCommandTest(unittest.TestCase):
    def test_writes_home_display_and_scrollback_sequences(self):
        source = ROOT / 'userspace/programs/src/clear.rs'
        self.assertTrue(source.is_file(), 'clear command is missing')
        with tempfile.TemporaryDirectory() as staging:
            command = Path(staging) / 'clear'
            subprocess.run(['rustc', '--edition=2021', '-Dwarnings', source, '-o', command],
                           cwd=ROOT, check=True, timeout=30)
            result = subprocess.run([command], capture_output=True, check=True, timeout=5)
        self.assertEqual(result.stdout, b'\x1b[H\x1b[2J\x1b[3J')


if __name__ == '__main__':
    unittest.main()
