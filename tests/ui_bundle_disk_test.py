"""A no-route UI run needs the exact retained bytes from its stopped disk."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


class BundleDisk(unittest.TestCase):
    def test_stopped_disk_fallback_and_refusals(self):
        with tempfile.TemporaryDirectory(prefix='ui-bundle-disk-') as tmp:
            root = Path(tmp)
            tree = root / 'tree'
            body = b'UTART001\nretained first-failure fixture'
            case = 'first/one'
            path = '/var/uitest/ui-testing/' + case
            destination = tree / path.lstrip('/')
            destination.mkdir(parents=True)
            (destination / 'retention.bundle').write_bytes(body)
            disk = root / 'disk.img'
            mke2fs = next((p for p in ('mke2fs', '/usr/sbin/mke2fs',
                          '/opt/homebrew/opt/e2fsprogs/sbin/mke2fs',
                          '/usr/local/opt/e2fsprogs/sbin/mke2fs') if shutil.which(p)), None)
            self.assertIsNotNone(mke2fs, 'the disk fallback regression requires e2fsprogs')
            made = subprocess.run([mke2fs, '-q', '-F', '-t', 'ext2', '-d', str(tree),
                                   str(disk), '8192'], capture_output=True, text=True)
            self.assertEqual(made.returncode, 0, made.stderr)
            manifest = json.loads((ROOT / 'tests/fixtures/suite-verdict/manifest.json').read_text())
            manifest['id'] = 'ui-testing'
            definition = root / 'manifest.json'
            definition.write_text(json.dumps(manifest))
            text = (ROOT / 'tests/fixtures/suite-verdict/all-pass.txt').read_text().replace(
                'SUITE fixture ', 'SUITE ui-testing ')
            label = 'uitest-' + hashlib.sha256(case.encode()).hexdigest()[:16]
            record = (f'SUITE ui-testing EVIDENCE bundle {case} label={label} bytes={len(body)} '
                      f'sha256={hashlib.sha256(body).hexdigest()} result=disk-only:no-route path={path}\n')
            log = root / 'serial.txt'

            def score(anchor=record, image=disk, failed=False):
                log.write_text(text.replace('SUITE ui-testing CASE first/one ',
                                            anchor + 'SUITE ui-testing CASE first/one '))
                if failed:
                    log.write_text(log.read_text().replace('CASE first/one PASS ms=1',
                        'CASE first/one FAIL ms=1 msg=real-failure').replace(
                        'DONE passed=3 failed=0', 'DONE passed=2 failed=1'))
                command = [sys.executable, str(ROOT / 'scripts/suite-verdict.py'), str(definition), str(log)]
                if image is not None:
                    command += ['--disk', str(image)]
                result = subprocess.run(command, capture_output=True, text=True)
                return result.returncode, result.stdout + result.stderr

            code, output = score()
            self.assertEqual(code, 0, output)
            suite_manifest = root / 'docs/suites/ui-testing.json'
            suite_manifest.parent.mkdir(parents=True)
            suite_manifest.write_text(json.dumps(manifest))
            scorer = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))
            sequence = runpy.run_path(str(ROOT / 'scripts/suite-sequence.py'))
            row = sequence['score_one'](scorer, root, 'ui-testing', False,
                                        log.read_text().splitlines(), log, [log], disk)
            self.assertEqual(row[1], 'PASS', row)
            # The QEMU watcher has already stopped its owned process before scoring.
            watcher = runpy.run_path(str(ROOT / 'scripts/watch-qemu.py'))
            clock = SimpleNamespace(verdict=lambda: (True, 'fixture clock'))
            with patch.dict(watcher['main'].__globals__, {
                    'ROOT': root, 'WallClockCheck': lambda _: clock,
                    'watch': lambda *args: 'done', 'stop': lambda _: None,
                    'subprocess': SimpleNamespace(Popen=lambda _: SimpleNamespace(returncode=0))}), \
                    patch.object(sys, 'argv', ['watch-qemu.py', '--serial', str(log), '--mode', 'suite',
                                               '--suite', 'ui-testing', '--disk', str(disk), '--', 'fixture']):
                self.assertEqual(watcher['main'](), 0)
            waiter = runpy.run_path(str(ROOT / 'scripts/wait-boot-done.py'))
            clock.observe = lambda _: None
            output = io.StringIO()
            with patch.dict(waiter['main'].__globals__, {'WallClockCheck': lambda *args: clock}), \
                    patch.object(sys, 'argv', ['wait-boot-done.py', str(log), 'suite', '--suite',
                                               'ui-testing', '--post-stop-ui-verdict']), \
                    contextlib.redirect_stdout(output):
                self.assertEqual(waiter['main'](), 0)
            self.assertIn('DONE: SUITE ui-testing DONE', output.getvalue())
            self.assertNotIn('PASS:', output.getvalue())
            # VM cleanup must score UI bundles even without manifest diskChecks.
            (root / 'scripts').mkdir()
            (root / 'scripts/suite-verdict.py').symlink_to(ROOT / 'scripts/suite-verdict.py')
            hdd = root / 'data.hdd'
            hdd.mkdir()
            os.link(disk, hdd / 'written.hds')
            functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
            shell = 'score_vm_suite_disk() {\n' + functions['score_vm_suite_disk'] + '\n}\nscore_vm_suite_disk'
            checked = subprocess.run(['/bin/bash', '-c', shell], env=dict(os.environ,
                BREENIX_ROOT=str(root), BOOT_MODE='suite', SUITE='ui-testing', ONLY='',
                SERIAL_LOG=str(log), PARALLELS='true', EXT2_HDD_DIR=str(hdd)),
                capture_output=True, text=True)
            self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)
            self.assertIn('PASS: SUITE ui-testing DONE', checked.stdout)
            # Slot exhaustion and a refused transfer also preserve disk custody.
            for status in ('slots', 'transfer', 'deadline'):
                code, output = score(record.replace('no-route', status))
                self.assertEqual(code, 0, output)
            for anchor, image, failed in (
                    (record, None, False), (record, root / 'missing.img', False),
                    (record.replace(hashlib.sha256(body).hexdigest(), '0' * 64), disk, False),
                    (record.replace(f'bytes={len(body)}', f'bytes={len(body)+1}'), disk, False),
                    (record.replace(path, path + '/missing'), disk, False),
                    (record + record, disk, False), (record, disk, True)):
                with self.subTest(anchor=anchor, image=image, failed=failed):
                    self.assertNotEqual(score(anchor, image, failed)[0], 0)

    def test_disk_only_records_do_not_consume_the_sixteen_upload_slots(self):
        scorer = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))
        names = [f'failures/case-{index}' for index in range(17)]
        manifest = dict(id='ui-testing', categories=[dict(id='failures',
                        cases=[dict(id=f'case-{index}') for index in range(17)])])
        records = [('START', None, '')]
        for case in names:
            label = 'uitest-' + hashlib.sha256(case.encode()).hexdigest()[:16]
            line = (f'SUITE ui-testing EVIDENCE bundle {case} label={label} bytes=9 '
                    f'sha256={"a" * 64} result=disk-only:slots path=/var/uitest/ui-testing/{case}')
            match = re.fullmatch(scorer['bundle_shape']('ui-testing'), line)
            self.assertIsNotNone(match)
            records.append(('EVIDENCE', match, line))
            passed = re.fullmatch(r'(.*)', case)
            records.append(('PASS', passed, ''))
        self.assertIsNone(scorer['result_problem'](manifest, records, names))
        uploaded = [(kind, re.fullmatch(scorer['bundle_shape']('ui-testing'),
                     line.replace('disk-only:slots', 'ok')), line) if kind == 'EVIDENCE'
                    else (kind, match, line) for kind, match, line in records]
        self.assertIsNotNone(scorer['result_problem'](manifest, uploaded, names))


if __name__ == '__main__':
    unittest.main()
