"""Exercise the direct gate's policy boundary without building or booting a guest."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DirectHandoffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.canonical = self.base / 'canonical'
        self.repo = self.base / 'private'
        self.home = self.base / 'home'
        config = self.home / '.config/breenix-gate/build-host.env'
        config.parent.mkdir(parents=True)
        config.touch()
        self.canonical.mkdir()
        self.git(self.canonical, 'init', '-q', '-b', 'main')
        self.git(self.canonical, 'config', 'user.name', 'Gate Test')
        self.git(self.canonical, 'config', 'user.email', 'gate@example.invalid')
        self.git(self.canonical, 'config', 'core.hooksPath', '/dev/null')
        self.git(self.canonical, 'config', 'commit.gpgsign', 'false')
        for name in ('docs/efforts/path.json', 'docs/efforts/metal.json',
                     'scripts/effort-target-guard.py', 'scripts/host-slots.sh',
                     'docker/qemu/run-x86-gate.sh', 'docker/qemu/lib/run-inspector-import.sh'):
            dest = self.canonical / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(ROOT / name, dest)
        # Stop at the handoff; the real guard and shell launcher run unchanged.
        (self.canonical / 'scripts/gate-tree.py').write_text(
            'import json, os, sys\n'
            'print(json.dumps(dict(args=sys.argv[1:], stamp=os.environ.get("BREENIX_TARGET_POLICY_CHECKED"))))\n')
        self.git(self.canonical, 'add', '.')
        self.git(self.canonical, 'commit', '-qm', 'fixture')
        self.main = self.git(self.canonical, 'rev-parse', 'HEAD')
        self.git(self.base, 'clone', '-q', str(self.canonical), str(self.repo))
        for key in ('user.name', 'user.email', 'core.hooksPath', 'commit.gpgsign'):
            self.git(self.repo, 'config', key, self.git(self.canonical, 'config', key))
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('BREENIX_')}
        self.env.update(HOME=str(self.home), BREENIX_REPO_DIR=str(self.repo),
                        BREENIX_SLOT_SESSION='handoff-test', BREENIX_GATE_TMP=str(self.base / 'output'))

    @staticmethod
    def git(repo, *args):
        return subprocess.run(['git', '-C', str(repo), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    def invoke(self):
        return subprocess.run(['bash', str(self.repo / 'docker/qemu/run-x86-gate.sh'), '1', 'full'],
                              env=self.env, cwd=self.repo, capture_output=True, text=True, timeout=30)

    def test_private_effort_ref_is_checked_before_handoff(self):
        self.git(self.repo, 'switch', '-qc', 'breengel/private')
        (self.repo / 'change').write_text('private effort')
        self.git(self.repo, 'add', 'change')
        self.git(self.repo, 'commit', '-qm', 'private effort')
        self.git(self.repo, 'push', '-q', 'origin', 'HEAD:refs/heads/fix/regression')
        refused = self.invoke()
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('effort lane breengel/private', refused.stderr)
        self.assertNotIn('"args"', refused.stdout)
        self.env['BREENIX_CATCHUP'] = '1'
        accepted = self.invoke()
        self.assertEqual(accepted.returncode, 0, accepted.stdout + accepted.stderr)
        handoff = json.loads(accepted.stdout.splitlines()[-1])
        self.assertEqual(handoff['stamp'], 'x86:' + self.git(self.repo, 'rev-parse', 'HEAD'))

    def test_default_and_new_output_directories_have_valid_run_identity(self):
        outputs = []
        for base in (None, self.base / 'new output' / 'nested'):
            with self.subTest(base=base):
                if base is None:
                    self.env.pop('BREENIX_GATE_TMP', None)
                else:
                    self.env['BREENIX_GATE_TMP'] = str(base)
                result = self.invoke()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                handoff = json.loads(result.stdout.splitlines()[-1])
                output = Path(handoff['args'][3])
                outputs.append(output)
                self.addCleanup(shutil.rmtree, output.parent)
                self.assertTrue(output.is_dir())
                self.assertEqual(output.name, 'gate-tmp')
                self.assertRegex(output.parent.name, r'^direct-x86-[A-Za-z0-9]+$')
                self.assertEqual(output.parent.parent, base if base else Path('/tmp'))
                self.assertEqual(handoff['stamp'], 'x86:' + self.main)
        self.assertNotEqual(outputs[0].parent.name, outputs[1].parent.name)


if __name__ == '__main__':
    unittest.main()
