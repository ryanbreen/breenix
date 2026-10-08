#!/usr/bin/env python3
"""Boundary scoring and real ext2 boot-target sequence readback, without a VM."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('sequence', ROOT / 'scripts/x86-suite-boot.py')
sequence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sequence)


class SequenceTests(unittest.TestCase):
    def test_later_panic_cannot_fail_finished_suites_and_unstarted_is_not_run(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, out = root / 'repo', root / 'out'
            out.mkdir()
            (repo / 'docs/suites').mkdir(parents=True)
            for suite in ('first', 'second', 'third', 'fourth'):
                (repo / 'docs/suites' / (suite + '.json')).write_text(json.dumps(dict(id=suite, categories=[dict(id='case', cases=[dict(id='one')])])) )
            def output(suite):
                user = out / 'serial_user.log'
                with user.open('ab') as handle:
                    handle.write(f'SUITE {suite} START cases=1\n'.encode())
                    if suite != 'third':
                        handle.write(f'SUITE {suite} CASE case/one PASS ms=1\nSUITE {suite} DONE passed=1 failed=0 skipped=0 total=1\nSUITE_SEQUENCE READY {suite}\n'.encode())
                with (out / 'serial_kernel.log').open('ab') as handle:
                    handle.write(b'KERNEL PANIC: third suite\n' if suite == 'third' else b'healthy\n')
            class Child:
                stopped = False
                def __init__(self, command): output('first')
                def poll(self): return 1 if self.stopped else None
                def terminate(self): self.stopped = True
                def wait(self, timeout=None): return 0
            class QMP:
                def __init__(self, path): self.index = 0
                def command(self, name, args=None):
                    if name == 'cont':
                        self.index += 1
                        output(('first', 'second', 'third')[self.index])
                        if self.index == 2: child.stopped = True
                def close(self): pass
            child = Child([])
            with patch.object(sequence.subprocess, 'Popen', return_value=child), patch.object(sequence, 'QMP', QMP), \
                 patch.object(sequence.time, 'sleep'), patch.dict(os.environ, BREENIX_QMP_SOCKET='/fake'), \
                 patch.object(sys, 'argv', ['boot', str(repo), str(out), 'first,second,third,fourth', '300', 'qemu']):
                self.assertEqual(sequence.main(), 1)
            rows = json.loads((out / 'suite-results.json').read_text())
            self.assertEqual([row['verdict'] for row in rows], ['PASS', 'PASS', 'FAIL', 'NOT-RUN'])
            self.assertIn('third suite', rows[2]['reason'])
            self.assertNotIn('PANIC', (out / 'suite-first/serial_kernel.log').read_text())

    def test_write_target_validates_every_binary_and_removes_old_sequence(self):
        bindir = Path('/opt/homebrew/opt/e2fsprogs/sbin')
        environment = dict(os.environ, PATH=str(bindir) + ':' + os.environ['PATH'])
        mkfs = shutil.which('mke2fs', path=environment['PATH'])
        debugfs = shutil.which('debugfs', path=environment['PATH'])
        if not mkfs or not debugfs:
            self.skipTest('requires e2fsprogs')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            image = root / 'disk.img'
            image.write_bytes(bytes(8 * 1024 * 1024))
            subprocess.run([mkfs, '-t', 'ext2', '-F', str(image)], check=True, capture_output=True)
            for suite in ('first', 'second'):
                (root / ('suite-' + suite + '.elf')).write_bytes(b'ELF-' + suite.encode())
            commands = root / 'commands'
            commands.write_text(f'mkdir /etc\nmkdir /sbin\nwrite {root}/suite-first.elf /sbin/suite-first\nwrite {root}/suite-second.elf /sbin/suite-second\n')
            subprocess.run([debugfs, '-w', '-f', str(commands), str(image)], check=True, capture_output=True)
            script = str(ROOT / 'scripts/write-boot-target.sh')
            def run(ids):
                return subprocess.run([script, str(image), 'first', str(root / 'suite-first.elf'), ids], env=environment, capture_output=True)
            self.assertEqual(run('first,second').returncode, 0)
            (root / 'suite-second.elf').write_bytes(b'wrong ELF')
            self.assertNotEqual(run('first,second').returncode, 0)
            self.assertEqual(run('first').returncode, 0)
            result = subprocess.run([debugfs, '-R', 'cat /etc/breenix/suite-sequence', str(image)], capture_output=True)
            self.assertEqual(result.stdout, b'')


if __name__ == '__main__':
    unittest.main()
