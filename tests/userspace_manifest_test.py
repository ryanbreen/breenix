#!/usr/bin/env python3
"""Prebuilt userspace is accepted only when it matches the sources that built it."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ManifestTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        (self.repo / 'scripts').mkdir()
        shutil.copy(ROOT / 'scripts/userspace-manifest.py', self.repo / 'scripts')
        self.source = self.repo / 'userspace/programs/src/hello.rs'
        self.source.parent.mkdir(parents=True)
        self.source.write_text('fn main() {}\n')
        self.elfs = self.repo / 'userspace/programs'
        (self.elfs / 'hello.elf').write_bytes(b'hello')
        (self.elfs / 'busybox.elf').write_bytes(b'pinned')
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True)
        self.assertEqual(self.run_tool('write').returncode, 0)

    def run_tool(self, *args):
        return subprocess.run(['python3', str(self.repo / 'scripts/userspace-manifest.py'), *args,
                               '--arch', 'x86_64'], capture_output=True, text=True)

    def assert_refused(self, reason):
        result = self.run_tool('check')
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(reason, result.stderr)

    def test_matching_userspace_is_accepted(self):
        self.assertEqual(self.run_tool('check').returncode, 0)

    def test_missing_manifest_is_refused(self):
        (self.elfs / '.userspace-manifest.json').unlink()
        self.assert_refused('no userspace manifest')

    def test_changed_source_is_refused(self):
        self.source.write_text('fn main() { loop {} }\n')
        self.assert_refused('sources changed')

    def test_archive_cleaning_script_is_a_build_input(self):
        cleaner = self.repo / 'scripts/libc-clean-archive.py'
        cleaner.write_text('clean archive version 1')
        self.assert_refused('sources changed')
        self.assertEqual(self.run_tool('write').returncode, 0)
        cleaner.write_text('clean archive version 2')
        self.assert_refused('sources changed')

    def test_stale_extra_and_missing_elfs_are_refused(self):
        (self.elfs / 'hello.elf').write_bytes(b'older build')
        self.assert_refused('hello.elf')
        (self.elfs / 'hello.elf').write_bytes(b'hello')
        (self.elfs / 'renamed.elf').write_bytes(b'left over')
        self.assert_refused('renamed.elf')
        (self.elfs / 'renamed.elf').unlink()
        (self.elfs / 'hello.elf').unlink()
        self.assert_refused('hello.elf')

    def test_foreign_busybox_is_not_part_of_the_build(self):
        (self.elfs / 'busybox.elf').write_bytes(b'repinned')
        self.assertEqual(self.run_tool('check').returncode, 0)

    def test_disk_packed_before_the_last_userspace_change_is_refused(self):
        disk = self.repo / 'ext2.img'
        disk.write_bytes(b'disk')
        manifest = self.elfs / '.userspace-manifest.json'
        stamp = manifest.stat().st_mtime
        os.utime(disk, (stamp - 60, stamp - 60))
        result = self.run_tool('check', '--disk', str(disk))
        self.assertEqual(result.returncode, 1)
        self.assertIn('packed before', result.stderr)
        os.utime(disk, (stamp + 60, stamp + 60))
        self.assertEqual(self.run_tool('check', '--disk', str(disk)).returncode, 0)

    def test_unchanged_rewrite_keeps_the_manifest_time(self):
        manifest = self.elfs / '.userspace-manifest.json'
        os.utime(manifest, (1, 1))
        self.assertEqual(self.run_tool('write').returncode, 0)
        self.assertEqual(manifest.stat().st_mtime, 1)


if __name__ == '__main__':
    unittest.main()
