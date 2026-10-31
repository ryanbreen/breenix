"""Execute the terminal transport against deterministic register/VRAM devices."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class TerminalTransport(unittest.TestCase):
    def test_engine_receipts_deadlines_and_binding(self):
        source = (ROOT / 'kernel/src/gpu/gk208/emergency.rs').read_text()
        # Preparation uses the existing compiler and mapping owner. This harness
        # executes the unmodified post-drain transport, with register devices.
        body = source[source.index('struct Terminal('):source.index('/// Preparation only: map immutable')]
        start = body.index('    /// Preparation only: share')
        end = body.index('    pub(crate) fn packet(', start)
        body = body[:start] + body[end:]
        start = body.index('    pub(crate) fn prepare(')
        end = body.index('    pub(crate) fn bound(', start)
        body = body[:start] + body[end:]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'test.rs').write_text((ROOT / 'tests/fixtures/gt710_panic_transport.rs').read_text().replace('// TRANSPORT', body))
            subprocess.run(['rustc', '--edition=2021', '--test', '--cfg', 'feature="capture_panic_oracle"', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
