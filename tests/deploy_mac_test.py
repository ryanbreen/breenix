#!/usr/bin/env python3
"""Exercise deployment locally with ssh and rsync stand-ins."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DeployTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'repo'
        self.root.mkdir()
        (self.root / 'scripts').mkdir()
        self.remote = Path(self.tmp.name) / 'remote'
        self.remote.mkdir()
        self.bin = Path(self.tmp.name) / 'bin'
        self.bin.mkdir()
        self.log = Path(self.tmp.name) / 'calls'
        self.env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}',
                        REMOTE_HOME=str(self.remote), CALL_LOG=str(self.log))
        self.run_git('init', '-q')
        self.run_git('config', 'user.email', 'test@example.invalid')
        self.run_git('config', 'user.name', 'Test')
        shutil.copy(ROOT / 'scripts/deploy-mac.sh', self.root / 'scripts')
        (self.root / 'scripts/boot-interactive.sh').write_text(
            '#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALL_LOG"\n')
        (self.root / 'scripts/boot-interactive.sh').chmod(0o755)
        self.items = []
        for name, data in [('target/kernel', b'kernel'), ('target/disk', b'disk'),
                           ('cache/QEMU-GL.app/Contents/bin', b'qemu')]:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        # Use the production hash algorithm, including the bundle digest.
        shutil.copy(ROOT / 'scripts/boot-artifacts.py', self.root / 'scripts/artifact_hash.py')
        import importlib.util
        spec = importlib.util.spec_from_file_location('artifact_hash', self.root / 'scripts/artifact_hash.py')
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        for name, dest in [('target/kernel', 'target/kernel'), ('target/disk', 'target/disk'),
                           ('cache/QEMU-GL.app', 'QEMU-GL.app')]:
            path = self.root / name
            self.items.append(dict(path=str(path), destination=dest,
                                   sha256=mod.bundle_digest(path) if path.is_dir() else mod.file_digest(path)))
        (self.root / 'scripts/boot-artifacts.py').write_text(
            '#!/usr/bin/env python3\nprint(' + repr(json.dumps(self.items)) + ')\n')
        (self.root / '.gitignore').write_text('target/\ncache/\n')
        self.run_git('add', 'scripts', '.gitignore')
        self.run_git('commit', '-qm', 'fixture')
        self.shim('ssh', '''#!/usr/bin/env python3
import os, subprocess, sys
with open(os.environ['CALL_LOG'], 'a') as f: f.write('ssh ' + ' '.join(sys.argv[1:]) + '\\n')
env = dict(os.environ, HOME=os.environ['REMOTE_HOME'])
r = subprocess.run(sys.argv[2], shell=True, env=env, stdin=sys.stdin.buffer)
sys.exit(r.returncode)
''')
        self.shim('rsync', '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
with open(os.environ['CALL_LOG'], 'a') as f: f.write('rsync ' + ' '.join(sys.argv[1:]) + '\\n')
src = pathlib.Path(sys.argv[-2].rstrip('/'))
dst = pathlib.Path(os.environ['REMOTE_HOME']) / sys.argv[-1].split(':', 1)[1].removeprefix('~/')
if src.is_dir(): shutil.copytree(src, dst, dirs_exist_ok=True, symlinks=True)
else:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
if os.environ.get('CORRUPT') and src.name == 'disk': dst.write_bytes(b'corrupt')
''')

    def shim(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def run_git(self, *args):
        subprocess.run(['git', *args], cwd=self.root, check=True, stdout=subprocess.DEVNULL)

    def deploy(self, **env):
        return subprocess.run([str(self.root / 'scripts/deploy-mac.sh'), 'test-host'], cwd=self.root,
                              env=dict(self.env, **env), text=True, capture_output=True)

    def test_transfer_and_verify(self):
        result = self.deploy()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.log.read_text()
        self.assertIn('--mode desktop --gpu-gl --build-only', calls)
        transfers = [line for line in calls.splitlines() if line.startswith('rsync ')]
        self.assertEqual(len(transfers), len(self.items))
        for item in self.items:
            self.assertTrue(any(item['path'] in call for call in transfers))
        self.assertNotIn('cargo', calls)
        self.assertNotIn('build.sh', calls)
        deployed = self.remote / 'breenix'
        self.assertEqual((deployed / '.deployed-commit').read_text().strip(),
                         subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=self.root, text=True).strip())
        self.assertEqual(json.loads((deployed / '.deployed-artifacts.json').read_text()), self.items)
        self.assertTrue((self.remote / 'cache/QEMU-GL.app/Contents/bin').exists())

    def test_dirty_tree_is_refused(self):
        (self.root / 'dirty').write_text('dirty')
        result = self.deploy()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())

    def test_hash_mismatch_prevents_swap(self):
        (self.remote / 'breenix').mkdir()
        (self.remote / 'breenix/old').write_text('old')
        result = self.deploy(CORRUPT='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.remote / 'breenix/old').exists())
        self.assertFalse((self.remote / 'breenix.prev').exists())


if __name__ == '__main__':
    unittest.main()
