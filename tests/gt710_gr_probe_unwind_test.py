"""Inject startup failures into the production GR probe and execute its unwind.

Firmware operations are substituted; reset and state restoration use ctxctl.rs.
The context-generation tail is replaced by an error after successful startup.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index(name)
    end = source.index('\n}\n', start) + 3
    return source[start:end]


class ProbeUnwind(unittest.TestCase):
    def test_startup_failures_restore_state_and_release_scratch_after_stop(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        probe = function(source, 'pub(super) fn probe(')
        helpers = ''
        if 'fn probe_inner(' in source:
            inner = function(source, 'fn probe_inner(')
            helpers = source[source.index('struct ProbeCleanup'):source.index('/// Capture startup')]
        else:
            inner, probe = probe, ''
        inner = inner[:inner.index('    let context = super::lifetime::Context::new(')]
        inner += '    let _ = (device, console, ready, withheld, stale);\n    Err("late failure")\n}\n'
        harness = (ROOT / 'tests/fixtures/gt710_gr_probe_unwind.rs').read_text()
        harness = harness.replace('PRODUCTION', helpers + probe + inner)
        harness = harness.replace('ROOT', str(ROOT))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'probe.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(path / 'probe.rs'), '-o', str(path / 'probe')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'probe'), 'unwind', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
