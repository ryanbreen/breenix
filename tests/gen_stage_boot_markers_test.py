#!/usr/bin/env python3
"""The ribbon generator must check every architecture's committed table."""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = Path('scripts/gen-stage-boot-markers.py')
X86 = Path('userspace/programs/src/suite_breengel/stage_boot_markers_x86_64.in')


class GeneratorCheckTest(unittest.TestCase):
    def test_missing_and_stale_x86_table_fail_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for path in (SCRIPT, Path('docs/boot-path.json'), Path('docs/metal/boot-path.json'),
                         Path('tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json'),
                         Path('tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'),
                         Path('userspace/programs/src/suite_breengel/stage_boot_markers.in')):
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / path, root / path)
            def check():
                return subprocess.run(['python3', str(root / SCRIPT), '--check'], capture_output=True, text=True)
            self.assertNotEqual(check().returncode, 0, 'missing x86 table accepted')
            (root / X86).write_text('stale\n')
            self.assertNotEqual(check().returncode, 0, 'stale x86 table accepted')
            shutil.copy2(ROOT / X86, root / X86)
            valid = check()
            self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)


if __name__ == '__main__':
    unittest.main()
