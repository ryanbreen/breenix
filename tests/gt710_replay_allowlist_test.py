"""Retained replay admits the host gallery exporter and refuses guest changes."""
from pathlib import Path
import runpy
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
HOST_FILES = ['tools/screen-gallery/export.py', 'tests/screen_gallery_export_test.py']


class ReplayAllowlist(unittest.TestCase):
    def changed(self, files):
        names = ''.join(name + '\0' for name in files).encode()
        with patch('subprocess.run'), patch('subprocess.check_output', return_value=names):
            return API['retained_scorer_changes']('a' * 40, 'b' * 40)

    def test_host_gallery_export_and_its_tests_are_admitted(self):
        self.assertEqual(self.changed(HOST_FILES), HOST_FILES)

    def test_guest_change_alongside_export_is_refused(self):
        with self.assertRaisesRegex(ValueError, 'runtime files: .*userspace/programs/src/suite.rs'):
            self.changed(HOST_FILES + ['userspace/programs/src/suite.rs'])


if __name__ == '__main__':
    unittest.main()
