"""Ruling 91: stage diagnostics are judged against an exact allowlist."""
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))
import cxx_runtime


class StageDiagnosticsTest(unittest.TestCase):
    def test_allowlisted_diagnostic_is_not_first_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = cxx_runtime.run_stage('stage', ('sh', '-c', 'echo "tool: error: probe absent"'),
                                           root, root, allowlist={'tool: error: probe absent': 1})
            self.assertIsNone(result['first_error'])

    def test_unlisted_diagnostic_is_first_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = cxx_runtime.run_stage('stage', ('sh', '-c', 'echo "tool: error: unexpected"'),
                                           root, root, allowlist={'tool: error: probe absent': 1})
            self.assertEqual(result['first_error'], 'tool: error: unexpected')
