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
        commands = re.findall(r'^\s*(printf .*) > "?' + re.escape(destination) + r'/(hosts|resolv.conf)"?$', source, re.M)
        self.assertEqual(len(commands), 2)
        etc = directory / 'etc'
        etc.mkdir()
        script = '\n'.join(command + ' > ' + shlex.quote(str(etc / filename)) for command, filename in commands)
        subprocess.run(['/bin/bash', '-e', '-c', script], check=True)
        self.assertEqual((etc / 'hosts').read_bytes(), b'127.0.0.1 localhost\n')
        self.assertEqual((etc / 'resolv.conf').read_bytes(), b'nameserver 10.0.2.3\n')

    def test_native_path_seeds(self):
        with tempfile.TemporaryDirectory() as temporary:
            self.check_seeds(SCRIPT.read_text(), '$MOUNT_DIR/etc', Path(temporary))

    def test_docker_receives_one_complete_script_and_seeds(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for directory in ['scripts', 'userspace/programs/aarch64', 'fonts', 'target', 'stubs']:
                (repo / directory).mkdir(parents=True, exist_ok=True)
            shutil.copy2(SCRIPT, repo / 'scripts/create_ext2_disk.sh')
            (repo / 'scripts/install-busybox.py').write_text('# fixture: no binaries are installed\n')
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
