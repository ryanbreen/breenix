"""Exercise the shared policy and real launchers without building or booting guests.

Build breenix-runs first; override its location with BREENIX_RUNS_BIN if needed.
"""
import importlib.util
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
GUARD = ROOT / 'scripts/effort-target-guard.py'
SPEC = importlib.util.spec_from_file_location('guard', GUARD)
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


class EffortTargetGuardTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.origin = self.base / 'origin'
        self.repo = self.base / 'checkout'
        subprocess.run(['git', 'init', '--bare', '-q', str(self.origin)], check=True)
        self.repo.mkdir()
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Guard Test')
        self.git('config', 'user.email', 'guard@example.invalid')
        self.git('config', 'commit.gpgsign', 'false')
        self.git('config', 'core.hooksPath', '/dev/null')
        (self.repo / 'docs/efforts').mkdir(parents=True)
        shutil.copy(ROOT / 'docs/efforts/path.json', self.repo / 'docs/efforts/path.json')
        self.main = self.commit('main')
        self.git('switch', '-qc', 'breengel/suite')
        self.effort = self.commit('effort')
        self.git('switch', '-qc', 'breengel/window')
        self.descendant = self.commit('window')
        self.git('switch', '-qc', 'fix/regression', 'main')
        self.fix = self.commit('fix')
        self.git('switch', '-qc', 'scratch/probe', 'main')
        self.scratch = self.commit('scratch')
        self.git('remote', 'add', 'origin', str(self.origin))
        self.git('push', '-q', 'origin', '--all')
        self.git('fetch', '-q', 'origin')
        # Runtime inputs for the actual launchers, all outside the tested Git graph.
        shutil.copy(ROOT / 'run.sh', self.repo / 'run.sh')
        (self.repo / 'scripts').symlink_to(ROOT / 'scripts', target_is_directory=True)
        (self.repo / 'docker').symlink_to(ROOT / 'docker', target_is_directory=True)
        (self.repo / 'tools').symlink_to(ROOT / 'tools', target_is_directory=True)
        shutil.copy(ROOT / 'docs/boot-path.json', self.repo / 'docs/boot-path.json')
        self.binary = Path(os.environ.get('BREENIX_RUNS_BIN',
            ROOT / 'tools/breenix-runs/.build/debug/breenix-runs'))
        self.env = dict(os.environ, BREENIX_RUNS_STORE=str(self.base / 'store'))
        for key in ('BREENIX_CATCHUP', 'BREENIX_SLOT_SESSION'):
            self.env.pop(key, None)

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.repo), *args], check=True,
                              text=True, capture_output=True).stdout.strip()

    def commit(self, text):
        (self.repo / 'file').write_text(text)
        self.git('add', '.')
        self.git('commit', '-qm', text)
        return self.git('rev-parse', 'HEAD')

    def invoke(self, *args, env=None):
        return subprocess.run(args, cwd=self.repo, env=env or self.env, text=True,
                              capture_output=True, timeout=20)

    def policy(self, sha, target='x86', *extra):
        return self.invoke('python3', str(GUARD), '--repo', str(self.repo), '--commit', sha,
                           '--target', target, *extra)

    def test_containment_and_exemptions(self):
        for sha in (self.main, self.fix, self.scratch):
            self.assertEqual(self.policy(sha).returncode, 0)
        # The requested SHA need not be HEAD or the effort branch tip.
        self.git('branch', '-D', 'breengel/suite')
        self.git('push', '-q', 'origin', '--delete', 'breengel/suite')
        result = self.policy(self.effort)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('origin/breengel/window', result.stderr)
        self.assertIn('ARM64 QEMU until its catch-up', result.stderr)
        self.assertEqual(self.policy(self.effort, 'x86', '--catchup').returncode, 0)
        self.git('branch', 'fix/shared', self.effort)
        self.git('push', '-q', 'origin', 'fix/shared')
        self.assertEqual(self.policy(self.effort).returncode, 0)
        self.git('push', '-q', 'origin', '--delete', 'fix/shared')
        self.git('switch', '-q', 'main')
        self.git('merge', '-q', '--ff-only', 'breengel/window')
        self.git('push', '-q', 'origin', 'main')
        self.assertEqual(self.policy(self.effort).returncode, 0)

    def test_unknown_prefix_is_not_an_effort(self):
        self.git('switch', '-qc', 'not-an-effort/lane', 'main')
        sha = self.commit('unknown')
        self.git('push', '-q', 'origin', 'not-an-effort/lane')
        self.assertEqual(self.policy(sha).returncode, 0)

    def test_offline_uses_tracking_refs_only(self):
        self.git('remote', 'set-url', 'origin', str(self.base / 'unavailable'))
        result = self.policy(self.effort)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Cannot read origin', result.stderr)
        self.assertIn('origin/breengel/suite', result.stderr)
        self.assertEqual(self.policy(self.fix).returncode, 0)
        for branch in ('breengel/suite', 'breengel/window'):
            self.git('update-ref', '-d', 'refs/remotes/origin/' + branch)
        self.assertEqual(self.policy(self.effort).returncode, 0)

    def test_arm64_never_reads_origin_or_refuses(self):
        with patch.object(guard, 'refresh_origin', side_effect=AssertionError('network on ARM64')):
            self.assertEqual(guard.check(self.repo, self.effort, 'arm64-qemu', False), self.effort)

    def test_vm_run_metadata_persists_catchup(self):
        body = re.search(r'(?ms)^record_launch_metadata\(\) \{\n(.*?)^\}',
                         (ROOT / 'run.sh').read_text()).group(1)
        for target in ('parallels', 'vmware'):
            for catchup in ('0', '1'):
                serial = self.base / ('serial-' + target + catchup)
                env = dict(self.env, SERIAL_LOG=str(serial), TARGET=target, BREENIX_CATCHUP=catchup)
                result = self.invoke('/bin/bash', '-c', body, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(Path(str(serial) + '.launch.json').read_text()),
                                 {'target': target, 'catchup': catchup == '1', 'commit': self.scratch})

    def test_real_launcher_dry_invocations(self):
        self.assertTrue(self.binary.is_file(), 'Build breenix-runs before running these host tests')
        for target in ('x86', 'parallels', 'vmware', 'arm64-qemu'):
            for name, sha, catchup, accepted in (
                ('effort', self.effort, False, target == 'arm64-qemu'),
                ('main', self.main, False, True), ('fix', self.fix, False, True),
                ('scratch', self.scratch, False, True), ('catchup', self.effort, True, True)):
                with self.subTest(target=target, name=name):
                    env = self.env.copy()
                    if target == 'x86':
                        args = [str(self.binary), 'run', 'x86', '--sha', sha, '--dry-run']
                        if catchup:
                            args.append('--catchup')
                    else:
                        self.git('checkout', '-q', '--detach', sha)
                        args = ['/bin/bash', str(self.repo / 'run.sh'), '--dry-run']
                        if target != 'arm64-qemu':
                            args.append('--' + target)
                        if catchup:
                            env['BREENIX_CATCHUP'] = '1'
                    result = self.invoke(*args, env=env)
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)
                    self.assertIn('Breenix target=', result.stdout.splitlines()[0])
                    if catchup:
                        self.assertIn('catchup=true' if target == 'x86' else 'BREENIX_CATCHUP=1',
                                      result.stdout.splitlines()[0])
                    if not accepted:
                        self.assertIn('origin/breengel/', result.stderr)
                        self.assertNotIn('prepareClone:', result.stdout)
                    print(f'dry {target} {name}: {"accepted" if accepted else "refused"}', flush=True)
        self.assertFalse((self.repo / 'target').exists(), 'dry runs must not build guests')
        runs = self.base / 'store/runs'
        self.assertFalse(runs.exists() and any(runs.iterdir()), 'dry runs must not launch runs')


if __name__ == '__main__':
    unittest.main()
