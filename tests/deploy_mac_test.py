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
                        REMOTE_HOME=str(self.remote), CALL_LOG=str(self.log), HOME=str(self.root))
        self.run_git('init', '-q')
        self.run_git('config', 'user.email', 'test@example.invalid')
        self.run_git('config', 'user.name', 'Test')
        shutil.copy(ROOT / 'scripts/deploy-mac.sh', self.root / 'scripts')
        shutil.copy(ROOT / 'scripts/check-host-renderer.py', self.root / 'scripts')
        (self.root / 'scripts/boot-interactive.sh').write_text(
            r'''#!/usr/bin/env python3
import json, os, pathlib, sys
with open(os.environ['CALL_LOG'], 'a') as f: f.write(' '.join(sys.argv[1:]) + '\n')
if '--build-only' in sys.argv: sys.exit(0)
p = pathlib.Path(sys.argv[sys.argv.index('--serial-log') + 1]).expanduser()
p.parent.mkdir(parents=True, exist_ok=True)
commit = pathlib.Path('.deployed-commit').read_text().strip()
if os.environ.get('SCENARIO') == 'mismatch': commit = '0' * 40
p.write_text('==> Commit: ' + commit + ' deployed=1\n')
renderer = 'llvmpipe' if os.environ.get('SCENARIO') == 'refused' else 'ANGLE Metal Renderer: Apple M1'
records = [dict(event=e, ctx_id=1, gl_vendor='Apple', gl_renderer=renderer, gl_version='4') for e in ('context-create', 'first-submit-3d')]
(p.parent / 'host-renderer.json').write_text(json.dumps(dict(format=1, records=records)))
sys.exit(1 if os.environ.get('SCENARIO') == 'remote-failure' else 0)
''' )
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
        (self.root / '.gitignore').write_text('target/\ncache/\nLibrary/\n')
        self.run_git('add', 'scripts', '.gitignore')
        self.run_git('commit', '-qm', 'fixture')
        self.shim('ssh', '''#!/usr/bin/env python3
import os, subprocess, sys
with open(os.environ['CALL_LOG'], 'a') as f: f.write('ssh ' + ' '.join(sys.argv[1:]) + '\\n')
env = dict(os.environ, HOME=os.environ['REMOTE_HOME'])
r = subprocess.run(sys.argv[-1], shell=True, env=env, stdin=sys.stdin.buffer)
sys.exit(r.returncode)
''')
        self.shim('rsync', '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
with open(os.environ['CALL_LOG'], 'a') as f: f.write('rsync ' + ' '.join(sys.argv[1:]) + '\\n')
source, target = sys.argv[-2:]
if ':' in source:
    src = pathlib.Path(os.environ['REMOTE_HOME']) / source.split(':', 1)[1].removeprefix('~/')
    dst = pathlib.Path(target)
else:
    src = pathlib.Path(source.rstrip('/'))
    dst = pathlib.Path(os.environ['REMOTE_HOME']) / target.split(':', 1)[1].removeprefix('~/')
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

    def deploy(self, *args, **env):
        return subprocess.run([str(self.root / 'scripts/deploy-mac.sh'), *args, 'test-host'], cwd=self.root,
                              env=dict(self.env, **env), text=True, capture_output=True)

    def test_transfer_and_verify(self):
        result = self.deploy('--push-only')
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
        self.log.unlink()
        result = self.deploy('--push-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        transfers = [line for line in self.log.read_text().splitlines() if line.startswith('rsync ')]
        self.assertEqual(len(transfers), 2)
        self.assertFalse(any('QEMU-GL.app' in call for call in transfers))

    def test_boot_and_local_judgment(self):
        for scenario in ('pass', 'refused', 'mismatch', 'remote-failure'):
            with self.subTest(scenario=scenario):
                result = self.deploy(SCENARIO=scenario)
                self.assertEqual(result.returncode == 0, scenario == 'pass', result.stdout + result.stderr)
                evidence = sorted((self.root / 'Library/Caches/breenix/deploy-mac/test-host').glob('*/serial.txt'))
                self.assertTrue(evidence, result.stdout + result.stderr)
                self.assertIn(str(evidence[-1].parent), result.stdout + result.stderr)
                self.assertTrue((evidence[-1].parent / 'host-renderer.json').exists())
                self.assertIn('--mode desktop --gpu-gl --no-build --no-display --until-desktop-ready --serial-log', self.log.read_text())

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
