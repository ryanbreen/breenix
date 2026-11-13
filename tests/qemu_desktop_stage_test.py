"""QEMU GUI staging matches the corresponding metal root files."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import metal_smb_test
import metal_root
import vm_site

ROOT = Path(__file__).resolve().parents[1]


class DesktopStageTest(unittest.TestCase):
    def test_gui_selection_matches_metal_without_touching_base(self):
        debugfs, = metal_root.require_e2fsprogs('debugfs')
        site = {key: 'fixture' for key in vm_site.REQUIRED}
        site['SNTP_SERVERS'] = 'clock.example 192.0.2.1'
        for case in (None, 'guskit', 'guskit/smb-save-open', 'bterm/launch'):
            with self.subTest(case=case), metal_smb_test.SmbStagingTest().gui_root() as (base, _, config):
                site.update(config, SMB_TEST_INSTANCE='fixture-instance')
                original = hashlib.sha256(base.read_bytes()).digest()
                qemu = base.with_name('qemu.img')
                metal = base.with_name('metal.img')
                shutil.copyfile(base, qemu)
                shutil.copyfile(base, metal)
                site_file = base.with_name('site')
                site_file.write_text(''.join(f'{key}={value}\n' for key, value in site.items()))
                site_file.chmod(0o600)
                metal_root.stage_root(metal, 'suite', 'gui-apps', site, smb_config=config, case=case)
                result = subprocess.run(['python3', str(ROOT / 'scripts/qemu-desktop-stage.py'), str(qemu),
                                         'gui-apps', case or '', '--forward-port', '23456'], capture_output=True,
                                        env=dict(os.environ, BREENIX_VM_SITE=str(site_file)))
                launcher_log = base.with_name('launcher.log')
                launcher_log.write_bytes(result.stdout + result.stderr)
                self.assertNotIn(config['SMB_TEST_PASSWORD'].encode(), result.stdout + result.stderr)
                self.assertNotIn(config['SMB_TEST_PASSWORD'].encode(), launcher_log.read_bytes())
                self.assertEqual(result.returncode, 0)
                def read(image, command):
                    return subprocess.run([debugfs, '-R', command, str(image)], capture_output=True, check=True)
                for name in ('desktop-suite', 'sntp-servers'):
                    path = '/etc/breenix/' + name
                    if case and case.startswith('bterm/'):
                        self.assertIn(b'File not found', read(qemu, 'stat ' + path).stderr)
                    else:
                        self.assertEqual(read(qemu, 'cat ' + path).stdout, read(metal, 'cat ' + path).stdout)
                        def metadata(image):
                            stat = read(image, 'stat ' + path).stdout.decode()
                            return re.search(r'Mode:\s+(\d+).*User:\s+(\d+)\s+Group:\s+(\d+)', stat, re.S).groups()
                        self.assertEqual(metadata(qemu), metadata(metal))
                for path in ('/etc/breenix/smb-test.cred', '/etc/breenix/smb-test-server',
                             '/etc/shares', '/mnt/guskit-rw', '/mnt/guskit-ro'):
                    if case and case.startswith('bterm/'):
                        self.assertIn(b'File not found', read(qemu, 'stat ' + path).stderr)
                    else:
                        qstat = read(qemu, 'stat ' + path).stdout.decode()
                        mstat = read(metal, 'stat ' + path).stdout.decode()
                        pattern = r'Type:\s+(\w+).*Mode:\s+(\d+).*User:\s+(\d+)\s+Group:\s+(\d+)'
                        self.assertEqual(re.search(pattern, qstat, re.S).groups(),
                                         re.search(pattern, mstat, re.S).groups())
                        if path == '/etc/breenix/smb-test-server':
                            self.assertEqual(read(qemu, 'cat ' + path).stdout, b'10.0.2.2 23456\n')
                        elif path == '/etc/shares':
                            self.assertEqual(read(qemu, 'cat ' + path).stdout, b''.join(
                                f'//10.0.2.2/{share} /mnt/guskit-{share} smb credfile=/etc/breenix/smb-test.cred,port=23456\n'.encode()
                                for share in ('rw', 'ro')))
                        elif not path.startswith('/mnt/'):
                            self.assertEqual(read(qemu, 'cat ' + path).stdout, read(metal, 'cat ' + path).stdout)
                self.assertEqual(hashlib.sha256(base.read_bytes()).digest(), original)


    def test_network_shares_stages_private_endpoint_without_desktop(self):
        debugfs, = metal_root.require_e2fsprogs('debugfs')
        with metal_smb_test.SmbStagingTest().gui_root() as (image, _, config):
            site = {key: 'fixture' for key in vm_site.REQUIRED}
            site.update(config, SMB_TEST_INSTANCE='fixture-instance', SNTP_SERVERS='clock.example')
            site_file = image.with_name('site')
            site_file.write_text(''.join(f'{key}={value}\n' for key, value in site.items()))
            site_file.chmod(0o600)
            result = subprocess.run(['python3', str(ROOT / 'scripts/qemu-desktop-stage.py'), str(image),
                                     'network-shares', '', '--forward-port', '23456'], capture_output=True,
                                    env=dict(os.environ, BREENIX_VM_SITE=str(site_file)))
            self.assertEqual(result.returncode, 0, result.stderr)
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], capture_output=True, check=True)
            self.assertEqual(read('cat /etc/breenix/smb-test-server').stdout, b'10.0.2.2 23456\n')
            self.assertIn(b'username=fixture-user', read('cat /etc/breenix/smb-test.cred').stdout)
            self.assertIn(b'File not found', read('stat /etc/breenix/desktop-suite').stderr)

    def test_missing_smb_configuration_is_refused(self):
        with metal_smb_test.SmbStagingTest().gui_root() as (image, _, _):
            site = {key: 'fixture' for key in vm_site.REQUIRED}
            site_file = image.with_name('site')
            site_file.write_text(''.join(f'{key}={value}\n' for key, value in site.items()))
            site_file.chmod(0o600)
            result = subprocess.run(['python3', str(ROOT / 'scripts/qemu-desktop-stage.py'), str(image),
                                     'gui-apps', 'guskit'], capture_output=True,
                                    env=dict(os.environ, BREENIX_VM_SITE=str(site_file)))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'SMB', result.stderr)

    def test_gui_evidence_route(self):
        debugfs, = metal_root.require_e2fsprogs('debugfs')
        with metal_smb_test.SmbStagingTest().gui_root() as (image, _, _):
            directory = image.parent / 'evidence'
            directory.mkdir()
            (directory / 'route.json').write_text(json.dumps({'port': 12345, 'token': 'a' * 64}))
            subprocess.run(['bash', str(ROOT / 'scripts/write-boot-target.sh'), str(image), 'gui-apps'], check=True)
            subprocess.run(['python3', str(ROOT / 'scripts/qemu-evidence.py'), 'stage', str(image),
                            str(directory), '--suite', 'gui-apps'], check=True)
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True).stdout
            self.assertEqual(read('cat /etc/breenix/evidence-route'),
                             ('http://10.0.2.2:12345/gt710-upload/' + 'a' * 64 + '\n').encode())
            stat = read('stat /etc/breenix/evidence-route').decode()
            self.assertIn('Mode:  0400', stat)
            self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')


if __name__ == '__main__':
    unittest.main()
