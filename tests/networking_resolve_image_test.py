"""Check both image resolver seeds, including the real Docker shell argument."""
import json
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/create_ext2_disk.sh'


class ResolverImageTests(unittest.TestCase):
    def check_seeds(self, source, destination, directory):
        commands = re.findall(r'^\s*(printf .*) > "?' + re.escape(destination) + r'/(hosts|resolv.conf|services)"?$', source, re.M)
        self.assertEqual(len(commands), 3)
        etc = directory / 'etc'
        etc.mkdir()
        script = '\n'.join(command + ' > ' + shlex.quote(str(etc / filename)) for command, filename in commands)
        subprocess.run(['/bin/bash', '-e', '-c', script], check=True, env=dict(os.environ, DNS_SERVER='10.211.55.1'))
        self.assertEqual((etc / 'hosts').read_bytes(), b'127.0.0.1 localhost\n')
        self.assertEqual((etc / 'resolv.conf').read_bytes(), b'nameserver 10.211.55.1\n')

        self.assertIn(b'http 80/tcp www\n', (etc / 'services').read_bytes())

    def test_private_target_disk_nameservers_read_back(self):
        debugfs = '/opt/homebrew/opt/e2fsprogs/sbin/debugfs'
        mke2fs = '/opt/homebrew/opt/e2fsprogs/sbin/mke2fs'
        if not Path(debugfs).exists():
            debugfs = shutil.which('debugfs')
            mke2fs = shutil.which('mke2fs')
        self.assertTrue(debugfs and mke2fs, 'e2fsprogs required for disk verification')
        with tempfile.TemporaryDirectory() as temporary:
            disk = Path(temporary) / 'target.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', str(disk), '8192'], check=True)
            subprocess.run([debugfs, '-w', '-R', 'mkdir /etc', str(disk)], check=True, capture_output=True)
            for server in ['10.0.2.3', '10.211.55.1', 'dhcp', '10.0.2.3']:
                subprocess.run(['/bin/bash', str(ROOT / 'scripts/write-resolv-conf.sh'), str(disk), server], check=True)
                result = subprocess.run([debugfs, '-R', 'stat /etc/resolv.conf', str(disk)], check=True, capture_output=True)
                if server == 'dhcp':
                    self.assertIn(b'Type: symlink', result.stdout)
                    self.assertIn(b'Fast link dest: "/proc/net/pnp"\n', result.stdout)
                else:
                    self.assertIn(b'Type: regular', result.stdout)
                    result = subprocess.run([debugfs, '-R', 'cat /etc/resolv.conf', str(disk)], check=True, capture_output=True)
                    self.assertEqual(result.stdout, f'nameserver {server}\n'.encode())

    def test_retired_vmware_nat_nameserver_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            disk = Path(temporary) / 'target.img'
            disk.touch()
            result = subprocess.run(['/bin/bash', str(ROOT / 'scripts/write-resolv-conf.sh'), str(disk), '172.16.45.2'],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('Unsupported target DNS server', result.stderr)

    def test_native_path_seeds(self):
        with tempfile.TemporaryDirectory() as temporary:
            self.check_seeds(SCRIPT.read_text(), '$MOUNT_DIR/etc', Path(temporary))

    def test_docker_receives_one_complete_script_and_seeds(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for directory in ['scripts', 'userspace/programs/aarch64', 'fonts', 'target', 'stubs']:
                (repo / directory).mkdir(parents=True, exist_ok=True)
            shutil.copy2(SCRIPT, repo / 'scripts/create_ext2_disk.sh')
            (repo / 'scripts/userspace-manifest.py').write_text('# fixture: manifest verified\n')
            shutil.copy2(ROOT / 'scripts/install-libc-busybox.sh', repo / 'scripts')
            (repo / 'vendor/busybox').mkdir(parents=True)
            shutil.copy2(ROOT / 'vendor/busybox/required-applets', repo / 'vendor/busybox')
            binaries = repo / 'userspace/programs/aarch64'
            (binaries / 'libc-busybox.elf').write_bytes(b'fixture ELF')
            shutil.copy2(ROOT / 'vendor/busybox/required-applets', binaries / 'libc-busybox.links')
            capture = repo / 'docker-args.json'
            docker = repo / 'stubs/docker'
            docker.write_text('#!/usr/bin/env python3\nimport json,os,pathlib,sys\n'
                              'if sys.argv[1] == "run":\n'
                              ' pathlib.Path(os.environ["CAPTURE"]).write_text(json.dumps(sys.argv[1:]))\n'
                              ' pathlib.Path(os.environ["IMAGE"]).touch()\n')
            docker.chmod(0o755)
            uname = repo / 'stubs/uname'
            uname.write_text('#!/bin/sh\necho Darwin\n')
            uname.chmod(0o755)
            env = dict(os.environ, PATH=str(repo / 'stubs') + ':' + os.environ['PATH'],
                       CAPTURE=str(capture), IMAGE=str(repo / 'target/ext2-aarch64.img'))
            result = subprocess.run(['/bin/bash', str(repo / 'scripts/create_ext2_disk.sh'), '--arch', 'aarch64'],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            args = json.loads(capture.read_text())
            self.assertEqual(args[-3:-1], ['sh', '-c'])
            self.assertIn('umount /mnt/ext2', args[-1])
            self.check_seeds(args[-1], '/mnt/ext2/etc', repo)


if __name__ == '__main__':
    unittest.main()
