#!/usr/bin/env python3
"""The hosted startup probe is selected only for the rust-host suite."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
STAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGE)


class StartupProbeTest(unittest.TestCase):
    def test_selection_and_hosted_layout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            probe = root / 'startup-probe'
            probe.write_bytes(b'hosted fixture')
            for selection, expected in [('time', False), ('rust-host', True), ('time,rust-host', True)]:
                self.assertEqual(STAGE.probe_for_suite(selection, probe), probe if expected else None)
        build = (ROOT / 'userspace/programs/build.sh').read_text()
        self.assertIn('HOSTED_BIN="rust-host-startup-probe"', build)
        self.assertIn('linker-hosted.ld', build)
        self.assertIn('--eh-frame-hdr', build)


if __name__ == '__main__':
    unittest.main()
