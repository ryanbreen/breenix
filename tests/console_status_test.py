"""Pure host checks for the console status parser and the B36 panel model."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / 'kernel/src/console/status.rs'
assert MODEL.is_file(), 'console status parser and geometry model are not implemented'
priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
with tempfile.TemporaryDirectory(prefix='console-status-') as tmp:
    binary = Path(tmp) / 'check'
    subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                              str(ROOT / 'tests/console_status_test.rs'),
                              '-o', str(binary)], check=True,
                   env=dict(os.environ, CARGO_BUILD_JOBS='4'))
    subprocess.run([str(binary)], check=True)
