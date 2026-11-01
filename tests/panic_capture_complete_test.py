"""Run the panic trigger and BXCAP emitter across scheduler-lock contention."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PanicCaptureComplete(unittest.TestCase):
    def test_scheduler_contention_at_injection_and_after_the_panic_banner(self):
        fixture = (ROOT / 'tests/fixtures/panic_capture_complete.rs').read_text()
        fixture = fixture.replace('ORACLE_PATH', str(ROOT / 'kernel/src/capture_oracle.rs'))
        fixture = fixture.replace('CAPTURE_PATH', str(ROOT / 'kernel/src/capture/mod.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--cfg', 'feature="capture_panic_oracle"', str(path / 'check.rs'),
                '-o', str(path / 'check')], check=True)
            failures = []
            for scenario in ('busy', 'banner-race', 'peer', 'deadline', 'real-panic'):
                result = subprocess.run([str(path / 'check'), scenario],
                                        capture_output=True, text=True)
                if result.returncode:
                    failures.append(f'{scenario}: {result.stdout}{result.stderr}')
            self.assertEqual(failures, [], '\n'.join(failures))


if __name__ == '__main__':
    unittest.main()
