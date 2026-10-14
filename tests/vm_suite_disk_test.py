"""Run VM cleanup against real raw/VMDK images and verify the recorded verdict."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VmSuiteDiskTest(unittest.TestCase):
    def test_stopped_written_disk_controls_exit_and_vigil_status(self):
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
        definitions = '\n'.join(f'{name}() {{\n{functions[name]}\n}}' for name in
                                ('bounded_vm_command', 'score_vm_suite_disk', 'finish_vm_boot', 'install_vm_boot_traps'))
        mke2fs = shutil.which('mke2fs') or '/opt/homebrew/opt/e2fsprogs/sbin/mke2fs'
        for platform in ('parallels', 'vmware'):
            for outcome in ('pass', 'wrong-bytes', 'stop-failed', 'convert-failed'):
                if platform == 'parallels' and outcome == 'convert-failed':
                    continue
                with self.subTest(platform=platform, outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    (root / 'scripts').mkdir()
                    (root / 'docs/suites').mkdir(parents=True)
                    (root / 'scripts/suite-verdict.py').symlink_to(ROOT / 'scripts/suite-verdict.py')
                    (root / 'scripts/bounded-vm-command.py').symlink_to(ROOT / 'scripts/bounded-vm-command.py')
                    recorder = root / 'scripts/vigil-record.sh'
                    recorder.write_text('#!/bin/sh\nprintf "%s" "$3" > "$RECORD"\n')
                    recorder.chmod(0o755)
                    manifest = {'id': 'disk', 'categories': [{'id': 'writes', 'cases': [{'id': 'sync'}]}],
                                'diskChecks': [{'path': '/written', 'byte': 65, 'length': 64}]}
                    (root / 'docs/suites/disk.json').write_text(json.dumps(manifest))
                    serial = root / 'serial'
                    serial.write_text('SUITE disk START cases=1\nSUITE disk CASE writes/sync PASS ms=1\n'
                                      'SUITE disk DONE passed=1 failed=0 skipped=0 total=1\n')
                    files = root / 'files'
                    files.mkdir()
                    (files / 'written').write_bytes((b'B' if outcome == 'wrong-bytes' else b'A') * 64)
                    hdd = root / 'data.hdd'
                    hdd.mkdir()
                    disk = hdd / 'written.hds'
                    with disk.open('wb') as stream:
                        stream.truncate(4 * 1024 * 1024)
                    subprocess.run([mke2fs, '-q', '-F', '-t', 'ext2', '-d', str(files), str(disk)],
                                   check=True, capture_output=True)
                    vmdk = root / 'written.vmdk'
                    # Keep the unused platform's disk invalid so pass cases
                    # detect a scorer reading the wrong disk.
                    (files / 'written').write_bytes((b'B' if platform == 'parallels' or outcome == 'wrong-bytes' else b'A') * 64)
                    vmware_raw = root / 'vmware.raw'
                    with vmware_raw.open('wb') as stream:
                        stream.truncate(4 * 1024 * 1024)
                    subprocess.run([mke2fs, '-q', '-F', '-t', 'ext2', '-d', str(files), str(vmware_raw)],
                                   check=True, capture_output=True)
                    if platform == 'vmware':
                        (files / 'written').write_bytes(b'B' * 64)
                        subprocess.run([mke2fs, '-q', '-F', '-t', 'ext2', '-d', str(files), str(disk)],
                                       check=True, capture_output=True)
                    subprocess.run(['qemu-img', 'convert', '-f', 'raw', '-O', 'vmdk', str(vmware_raw), str(vmdk)],
                                   check=True, capture_output=True)
                    if outcome == 'convert-failed':
                        vmdk.write_text('invalid VMDK')
                    # Both controllers mark the disk stopped before the scorer runs.
                    controller = root / 'stop'
                    controller.write_text('#!/bin/sh\n[ "$STOP_FAIL" = 0 ] || exit 1\ntouch "$STOPPED"\n')
                    controller.chmod(0o755)
                    env = dict(os.environ, BREENIX_ROOT=str(root), BOOT_MODE='suite', SUITE='disk',
                               SERIAL_LOG=str(serial), PARALLELS='true' if platform == 'parallels' else 'false',
                               PARALLELS_VM='test', EXT2_HDD_DIR=str(hdd), EXT2_VMDK_ARG=str(vmdk),
                               VMRUN=str(controller), VMX_FILE='test.vmx', VM_BUNDLE=str(root),
                               STOPPED=str(root / 'stopped'), STOP_FAIL='1' if outcome == 'stop-failed' else '0',
                               RECORD=str(root / 'record'), CONTROLLER=str(controller))
                    script = definitions + '''
prlctl() { "$CONTROLLER" "$@"; }
host_slot_header() { :; }
vmware_check_network_log() { :; }
vmware_network_summary() { :; }
host_slot_release() { :; }
delete_vmware_bundle() { :; }
install_vm_boot_traps
exit 0
'''
                    result = subprocess.run(['/bin/bash', '-c', script], env=env, capture_output=True,
                                            text=True, timeout=15)
                    expected = 0 if outcome == 'pass' else 1
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                    self.assertEqual((root / 'record').read_text(), str(expected))
                    self.assertEqual((root / 'stopped').exists(), outcome != 'stop-failed')
                    if outcome == 'pass':
                        self.assertIn('raw disk checks passed=1', result.stdout)
                    elif outcome == 'wrong-bytes':
                        self.assertIn('raw disk bytes differ: /written', result.stdout)
                    else:
                        self.assertNotIn('raw disk checks passed=', result.stdout)


if __name__ == '__main__':
    unittest.main()
