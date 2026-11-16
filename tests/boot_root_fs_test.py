#!/usr/bin/env python3
"""Check the boot root choice without starting a VM."""
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class BootRootFsTest(unittest.TestCase):
    def test_default_and_breenfs_drive_selection(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = r'''
set -eu
ROOT=$1
WRITABLE=$2/root.img
MODE=suite
SUITE=directories,filesystem,files-io
run_build() { printf 'build:%s\n' "$*"; }
source "$ROOT/scripts/lib/root-fs.sh"
ROOT_FS=ext2
root_fs_prepare
printf 'ext2:%s\n' "$ROOT_DRIVE"
ROOT_FS=breenfs
root_fs_prepare
printf 'breenfs:%s\n' "$ROOT_DRIVE"
root_fs_scratch
printf 'scratch:%s\n' "${BREENFS_SCRATCH_ARGS[*]}"
'''
            result = subprocess.run(['bash', '-c', script, 'test', str(ROOT), tmp],
                                    text=True, capture_output=True, check=True).stdout
            self.assertIn(f'ext2:if=none,id=ext2,format=raw,file={tmp}/root.img,cache=unsafe,aio=threads', result)
            self.assertIn('scripts/create-breenfs-root.sh', result)
            self.assertIn('breenfs-mkfs --size 67108864', result)
            self.assertIn('id=breenfs-scratch,format=raw', result)
            boot = (ROOT / 'scripts/boot-interactive.sh').read_text()
            self.assertLess(boot.index('${BREENFS_SCRATCH_ARGS[@]'), boot.index('${BREENFS_READ_ARGS[@]'))
            self.assertIn('--root-fs', boot)


if __name__ == '__main__':
    unittest.main()
