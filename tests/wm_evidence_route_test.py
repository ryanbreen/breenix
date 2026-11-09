"""Stage the neutral route in ext2 and submit its two slots to the real receiver."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit

from gt710_final_upload_test import Handler, upload
from metal_tools_test import StateTest, load_script
import metal_disk
import metal_root


class WmEvidenceRoute(StateTest):
    def setUp(self):
        super().setUp()
        self.mke2fs, self.debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        self.boot = upload.m.STATE / 'boots/sample'
        self.inputs = self.boot / 'inputs'
        self.inputs.mkdir(parents=True)
        tree = self.inputs / 'tree'
        (tree / 'etc/breenix').mkdir(parents=True)
        (tree / 'sbin').mkdir()
        for suite in ('window-manager', 'console', 'gpu', 'breengel', 'gui-apps', 'time'):
            (tree / ('sbin/suite-' + suite)).write_bytes(b'\x7fELFfixture')
        self.image = self.inputs / 'ext2.img'
        subprocess.run([self.mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F',
                        str(self.image), '4096'], check=True, capture_output=True)
        self.site = {'BOOT_URL': 'http://fixture.invalid/boot',
                     'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345',
                     'CONSOLE_IDENTITY_KEY': 'ab' * 32}
        self.build = {'ext2': {'sha256': hashlib.sha256(self.image.read_bytes()).hexdigest()}}
        upload.m.write_json(upload.m.STATE / 'disk.json',
                            dict(installedBy='prior', bytes=4096, sha256AfterRecord='a' * 64))

    def read(self, name):
        return subprocess.run([self.debugfs, '-R', 'cat /etc/breenix/' + name,
                               str(self.image)], check=True, capture_output=True).stdout.decode()

    def stage(self, suite):
        # Delivery starts from an ext2 image whose record sector is unused.
        with self.image.open('r+b') as image:
            image.write(bytes(512))
        # Each iteration represents a fresh boot's upload slots.
        for artifact in self.boot.iterdir():
            if artifact.is_file() and artifact.name.startswith(('gt710-', 'native-')):
                artifact.unlink()
        metal_disk.stage(self.boot, self.inputs, 'sample', 'a' * 40, self.site,
                         self.build, mode='suite', suite=suite)

    def test_staged_route_reaches_both_digest_verified_slots(self):
        for suites in ('window-manager', 'console,window-manager', 'gui-apps', 'console,gui-apps'):
            with self.subTest(suites=suites):
                self.stage(suites)
                route = self.read('evidence-route')
                self.assertRegex(route, r'^http://fixture.invalid/gt710-upload/[a-f0-9]{64}\n$')
                self.assertEqual(self.read('gt710-upload'), route)
                stat = subprocess.run([self.debugfs, '-R', 'stat /etc/breenix/evidence-route',
                                       str(self.image)], check=True, capture_output=True).stdout.decode()
                self.assertIn('Mode:  0400', stat)
                self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
                path = urlsplit(route.strip()).path
                slot = self.boot
                if ',' in suites:
                    selected = suites.split(',')[-1]
                    path += '/suites/' + selected
                    slot /= 'native-suites/' + selected
                with patch.object(upload.m, 'read_lease', return_value=dict(id='sample', state='powered')):
                    bad = Handler(path + '/wrong', b'wrong')
                    upload.receive(bad)
                    self.assertEqual(bad.code, 404)
                    for suffix, body, filename, receipt in (
                            ('', b'startup pixels', 'gt710-pixels.rle', 'native-upload.json'),
                            ('/final', b'final panel pixels', 'gt710-final.rle', 'native-final-upload.json')):
                        rejected = Handler(path + suffix, body, '0' * 64)
                        upload.receive(rejected)
                        self.assertEqual(rejected.code, 400)
                        self.assertFalse((slot / filename).exists())
                        handler = Handler(path + suffix, body)
                        upload.receive(handler)
                        self.assertEqual(handler.code, 200)
                        self.assertEqual((slot / filename).read_bytes(), body)
                        facts = json.loads((slot / receipt).read_text())
                        self.assertTrue(facts['complete'])
                        self.assertEqual(facts['sha256'], hashlib.sha256(body).hexdigest())

    def test_large_single_request_refused_and_ordered_parts_accepted(self):
        for suite in ('window-manager', 'gui-apps'):
            self.stage(suite)
            path = urlsplit(self.read('evidence-route').strip()).path
            body = b'x' * 2_400_000
            digest = hashlib.sha256(body).hexdigest()
            manifest = f'SUITE_PARTS v=1 first=0 parts=2 bytes={len(body)} sha256={digest}\n'.encode()
            with patch.object(upload.m, 'read_lease', return_value=dict(id='sample', state='powered')):
                oversized = Handler(path, body)
                upload.receive(oversized)
                self.assertEqual(oversized.code, 400)
                self.assertFalse((self.boot / 'gt710-pixels.rle').exists())
                start = Handler(path, manifest)
                upload.receive(start)
                self.assertEqual(start.code, 200)
                wrong = Handler(path + '/pictures/1', body[2 << 20:])
                upload.receive(wrong)
                self.assertEqual(wrong.code, 404)
                for index, part in enumerate((body[:2 << 20], body[2 << 20:])):
                    handler = Handler(path + f'/pictures/{index}', part)
                    upload.receive(handler)
                    self.assertEqual(handler.code, 200)
                whole = b''.join((self.boot / f'gt710-breengel-{i:04}.rle').read_bytes() for i in range(2))
                self.assertEqual(hashlib.sha256(whole).hexdigest(), digest)
                final = Handler(path + '/final', b'final')
                upload.receive(final)
                self.assertEqual(final.code, 200)
                late = Handler(path + '/pictures/2', b'late')
                upload.receive(late)
                self.assertEqual(late.code, 404)

    def test_no_route_and_reused_root_disarm_the_shared_facility(self):
        self.stage('window-manager')
        metal_root.stage_root(self.image, 'suite', 'window-manager', self.site)
        self.assertFalse(self.read('evidence-route').strip())
        for suite in ('console', 'gpu', 'breengel', 'time'):
            with self.subTest(suite=suite):
                self.stage(suite)
                self.assertFalse(self.read('evidence-route').strip())
                self.assertEqual(bool(self.read('gt710-upload').strip()), suite != 'time')

    def test_launcher_starts_receiver_before_launching_window_manager(self):
        from types import SimpleNamespace
        run = load_script('metal-run')
        args = SimpleNamespace(mode='suite', suite='window-manager', payload='image',
                               inject=None, input=None, disk='root', sha='a' * 40,
                               branch='metal-wm/fixture', no_wait=True, nmi=False,
                               serve_as=None, content_type=None, ramdisk=False)
        stopped = SimpleNamespace(returncode=1, stdout='fixture stopped before launch', stderr='')
        with patch.object(run, 'host_env', return_value={}), \
             patch.object(run, 'verify_target'), \
             patch.object(run, 'resolve_suites', return_value='window-manager'), \
             patch.object(run, 'in_metal', return_value=stopped) as commands:
            self.assertEqual(run.boot(args), 1)
        self.assertEqual(commands.call_args_list[0].args[1:4],
                         ('systemctl', 'start', 'breenix-metal-machine.service'))
        self.assertIn('launch', commands.call_args_list[1].args)


if __name__ == '__main__':
    unittest.main()
