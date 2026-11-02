#!/usr/bin/env python3
"""Host-only candidate staging, including both upgrade fault injections."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
DEBUGFS = shutil.which('debugfs') or '/opt/homebrew/opt/e2fsprogs/sbin/debugfs'
MKE2FS = shutil.which('mke2fs') or '/opt/homebrew/opt/e2fsprogs/sbin/mke2fs'


class CandidateTest(unittest.TestCase):
    def test_candidates(self):
        with tempfile.TemporaryDirectory(prefix='vm-candidate-', dir='/tmp') as temp:
            root = Path(temp)
            (root / 'scripts/metal').mkdir(parents=True)
            (root / 'userspace/programs').mkdir(parents=True)
            (root / 'bin').mkdir()
            for name in ('vm-persistent.sh', 'vm-artifacts.py', 'vm_slots.py', 'vm_site.py'):
                shutil.copy(ROOT / 'scripts' / name, root / 'scripts' / name)
            shutil.copy(ROOT / 'scripts/metal/metal_root.py', root / 'scripts/metal/metal_root.py')
            shutil.copy(ROOT / 'scripts/metal/metal_probe.py', root / 'scripts/metal/metal_probe.py')
            for name, body in {
                'bin/taskpolicy': '#!/bin/sh\nshift\nexec "$@"\n',
                'bin/cargo': '#!/bin/sh\nmkdir -p target/aarch64-breenix-kernel/release\nprintf kernel > target/aarch64-breenix-kernel/release/kernel-aarch64\n',
                'userspace/programs/build.sh': '#!/bin/sh\nexit 0\n',
                'scripts/create_ext2_disk.sh': '#!/bin/sh\nmkdir -p target\ncp fixture.img target/ext2-aarch64.img\n',
            }.items():
                path = root / name
                path.write_text(body)
                path.chmod(0o755)
            tree = root / 'tree'
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'usr/sbin').mkdir(parents=True)
            (tree / 'usr/sbin/bsshd').write_text('daemon')
            image = root / 'fixture.img'
            with image.open('wb') as f:
                f.truncate(16 * 1024 * 1024)
            subprocess.run([MKE2FS, '-q', '-t', 'ext2', '-F', '-d', str(tree), str(image)], check=True)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / '.gitignore').write_text('/target/\n/run/\n/bin/\n/tree/\n/fixture.img\n/site\n')
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture'], check=True)
            run = root / 'run'
            run.mkdir()
            (run / '.breenix-vm-persistent').touch()
            (run / 'system.img').write_bytes(b'persisted system sentinel')
            site = root / 'site'
            site.write_text('OPERATOR_USER=fixture\nOPERATOR_UID=1234\nOPERATOR_PASSWORD=test\nOPERATOR_PASSWORD_HASH=$6$fixture$' + 'A'*86 + '\nOPERATOR_SSH_PUBLIC_KEY=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nCONSOLE_USER=root\nCONSOLE_PASSWORD=test\nROOT_PASSWORD_HASH=$6$fixture$' + 'B'*86 + '\nSNTP_SERVERS=192.0.2.1\n')
            site.chmod(0o600)
            env = dict(os.environ, PATH=str(root / 'bin') + ':' + os.environ['PATH'], BREENIX_VM_SITE=str(site))
            cmd = [str(root / 'scripts/vm-persistent.sh'), 'qemu', 'stage-candidate', '--run-dir', str(run)]
            for extra in (['--inject', 'bad'], ['--mode', 'console'], ['--inject']):
                self.assertNotEqual(subprocess.run(cmd + extra, env=env, capture_output=True).returncode, 0)
            before = hashlib.sha256((run / 'system.img').read_bytes()).hexdigest()
            generations = set()
            for fault in ('', 'upgrade-no-ssh', 'upgrade-service-down'):
                result = subprocess.run(cmd + (['--inject', fault] if fault else []), cwd=root, env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                seal = json.loads((run / 'candidate.json').read_text())
                generation = seal['generation']
                self.assertRegex(generation, r'^[A-Za-z0-9-]{1,64}$')
                self.assertNotIn(generation, generations)
                generations.add(generation)
                self.assertEqual(seal['injectedFault'], fault or None)
                for key, name in (('imageSha256', 'candidate.img'), ('kernelSha256', 'candidate-kernel')):
                    self.assertEqual(seal[key], hashlib.sha256((run / name).read_bytes()).hexdigest())
                listing = subprocess.check_output([DEBUGFS, '-R', 'ls -p /etc/breenix', str(run / 'candidate.img')], stderr=subprocess.DEVNULL, text=True)
                self.assertIn('/operator-account/', listing)
                account = subprocess.check_output([DEBUGFS, '-R', 'cat /etc/breenix/operator-account', str(run / 'candidate.img')], stderr=subprocess.DEVNULL)
                self.assertTrue(account)
                listing = subprocess.check_output([DEBUGFS, '-R', 'ls -p /usr/sbin', str(run / 'candidate.img')], stderr=subprocess.DEVNULL, text=True)
                self.assertEqual('/bsshd/' in listing, fault != 'upgrade-service-down')
                self.assertEqual((run / 'candidate-kernel').read_bytes() == b'kernel', fault != 'upgrade-no-ssh')
                self.assertEqual(hashlib.sha256((run / 'system.img').read_bytes()).hexdigest(), before)


if __name__ == '__main__':
    unittest.main()
