"""QEMU GUI entry points resolve metal helpers without inherited PYTHONPATH."""
import os
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DesktopImportTest(unittest.TestCase):
    def test_launcher_entry_points(self):
        environment = dict(os.environ, ROOT=str(ROOT), SUITE='gui-apps', ONLY='bterm/launch')
        environment.pop('PYTHONPATH', None)
        source = (ROOT / 'scripts/boot-interactive.sh').read_text()
        selection = re.search(r"    SMB_INSTANCE=\$\(.*?^PYSMB\n\)", source, re.S | re.M)
        self.assertIsNotNone(selection)
        commands = (
            ['python3', str(ROOT / 'scripts/qemu-desktop-stage.py'), '--help'],
            ['bash', '-c', selection.group(0)],
        )
        for command in commands:
            with self.subTest(entry=command[0]):
                result = subprocess.run(command, env=environment, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
