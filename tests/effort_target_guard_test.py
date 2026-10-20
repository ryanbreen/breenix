"""Exercise the shared policy and real launchers without building or booting guests.

Prerequisites: run scripts/build-host-test-tools.sh; BREENIX_RUNS_BIN overrides the CLI.
"""
import base64
import binascii
import importlib.util
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
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
        subprocess.run(['git', 'init', '--bare', '-q', '-b', 'main', str(self.origin)], check=True)
        self.repo.mkdir()
        self.git('init', '-q', '-b', 'main')
        self.configure(self.repo)
        (self.repo / 'docs/efforts').mkdir(parents=True)
        shutil.copy(ROOT / 'docs/efforts/path.json', self.repo / 'docs/efforts/path.json')
        shutil.copy(ROOT / 'docs/efforts/metal.json', self.repo / 'docs/efforts/metal.json')
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
        self.env = dict(os.environ, BREENIX_ALLOW_X86_QEMU="1", BREENIX_RUNS_STORE=str(self.base / 'store'))
        for key in ('BREENIX_CATCHUP', 'BREENIX_SLOT_SESSION', 'BREENIX_TARGET_POLICY_CHECKED',
                    'BREENIX_HOST_ENTRY'):
            self.env.pop(key, None)

    @staticmethod
    def configure(repo):
        for key, value in (('user.name', 'Guard Test'), ('user.email', 'guard@example.invalid'),
                           ('commit.gpgsign', 'false'), ('core.hooksPath', '/dev/null')):
            subprocess.run(['git', '-C', str(repo), 'config', key, value], check=True)

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.repo), *args], check=True,
                              text=True, capture_output=True).stdout.strip()

    def commit(self, text):
        (self.repo / 'file').write_text(text)
        self.git('add', 'file')
        self.git('commit', '-qm', text)
        return self.git('rev-parse', 'HEAD')

    def invoke(self, *args, env=None):
        return subprocess.run(args, cwd=self.repo, env=env or self.env, text=True,
                              capture_output=True, timeout=60)

    def policy(self, sha, target='x86', *extra, env=None):
        return self.invoke('python3', str(GUARD), '--repo', str(self.repo), '--commit', sha,
                           '--target', target, *extra, env=env)

    @staticmethod
    def start_gate(stdout):
        # The dry-run plan carries the remote gate script base64-encoded.
        line = next(line for line in stdout.splitlines() if 'startGate:' in line)
        decoded = []
        for token in re.findall(r'[A-Za-z0-9+/]{100,}={0,2}', line):
            try:
                decoded.append(base64.b64decode(token).decode(errors='replace'))
            except binascii.Error:
                pass
        return '\n'.join(decoded)

    def assert_refused(self, result, lane='breengel/'):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('effort lane ' + lane, result.stderr)
        self.assertIn('ARM64 QEMU until its catch-up', result.stderr)

    def test_main_fix_and_scratch_on_main_are_accepted(self):
        for target in ('x86', 'parallels', 'vmware'):
            for sha in (self.main, self.fix, self.scratch):
                result = self.policy(sha, target)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), sha)

    def test_effort_history_is_refused_until_catchup(self):
        for target in ('x86', 'parallels', 'vmware'):
            for sha in (self.effort, self.descendant):
                self.assert_refused(self.policy(sha, target))
                self.assertEqual(self.policy(sha, target, '--catchup').returncode, 0)
        for value in ('1', 'true', 'YES'):
            self.assertEqual(self.policy(self.effort, env=dict(self.env, BREENIX_CATCHUP=value)).returncode, 0)
        for value in ('0', 'false', 'no', ''):
            self.assert_refused(self.policy(self.effort, env=dict(self.env, BREENIX_CATCHUP=value)))
        result = self.policy(self.main, env=dict(self.env, BREENIX_CATCHUP='maybe'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('BREENIX_CATCHUP must be', result.stderr)

    def metal_lane(self, name='metal-bring-up/first-light', base='main'):
        self.git('switch', '-qc', name, base)
        sha = self.commit(name)
        self.git('push', '-q', 'origin', name)
        return sha

    def merge(self, branch):
        # The ours strategy preserves the current tree while adding the other ancestry.
        self.git('merge', '-q', '--no-ff', '-s', 'ours', branch, '-m', 'merge test history')
        return self.git('rev-parse', 'HEAD')

    def test_guard_table_for_both_streams(self):
        metal = self.metal_lane()
        for target in guard.TARGETS:
            for catchup in (False, True):
                for sha in (self.main, self.fix, self.scratch, self.effort, metal):
                    with self.subTest(target=target, catchup=catchup, sha=sha):
                        expected = (sha != self.effort or target == 'arm64-qemu' or
                                    (catchup and target != 'x86-metal'))
                        if target == 'x86-metal' and sha in (self.fix, self.scratch):
                            expected = False  # Positive ancestry rule also applies to unlabelled work.
                        result = self.policy(sha, target, *(['--catchup'] if catchup else []))
                        self.assertEqual(result.returncode == 0, expected, result.stdout + result.stderr)
                        if expected:
                            self.assertEqual(result.stdout.strip(), sha)

    def test_metal_catchup_refuses_effort_from_fresh_clone(self):
        fresh = self.base / 'fresh'
        subprocess.run(['git', 'clone', '-q', '--single-branch', '-b', 'main',
                        str(self.origin), str(fresh)], check=True)
        (fresh / 'docs/efforts').mkdir(parents=True)
        for name in ('path.json', 'metal.json'):
            shutil.copy(ROOT / 'docs/efforts' / name, fresh / 'docs/efforts' / name)
        result = self.invoke('python3', str(GUARD), '--repo', str(fresh),
                             '--commit', self.effort, '--target', 'x86-metal', '--catchup')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("main-path effort work cannot run on metal", result.stderr)

    def test_metal_requires_published_first_parent_history(self):
        self.git('switch', '-qc', 'metal-stream/definition', 'main')
        local = self.commit('definition')
        for extra in ([], ['--catchup']):
            result = self.policy(local, 'x86-metal', *extra)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('not first-parent reachable', result.stderr)
        self.git('push', '-q', 'origin', 'metal-stream/definition')
        self.assertEqual(self.policy(local, 'x86-metal').returncode, 0)
        self.git('switch', '-qc', 'scratch/renamed-metal')
        self.git('branch', '-D', 'metal-stream/definition')
        self.assertEqual(self.policy(local, 'x86-metal').returncode, 0)
        self.git('push', '-q', 'origin', 'HEAD:refs/heads/scratch/renamed-metal')
        self.git('push', '-q', 'origin', '--delete', 'metal-stream/definition')
        self.assertIn('not first-parent reachable', self.policy(local, 'x86-metal').stderr)

    def test_deleted_or_renamed_second_parent_is_refused(self):
        for rename in (False, True):
            with self.subTest(rename=rename):
                name = 'metal-bring-up/merge-' + str(rename)
                self.metal_lane(name)
                merged = self.merge('scratch/probe')
                self.git('push', '-q', 'origin', name)
                if rename:
                    self.git('branch', '-m', 'scratch/probe', 'fix/renamed')
                    self.git('push', '-q', 'origin', 'fix/renamed')
                else:
                    self.git('branch', '-D', 'scratch/probe')
                self.git('push', '-q', 'origin', '--delete', 'scratch/probe')
                for extra in ([], ['--catchup']):
                    result = self.policy(merged, 'x86-metal', *extra)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('not first-parent reachable', result.stderr)
                if not rename:
                    self.git('branch', 'scratch/probe', self.scratch)
                    self.git('push', '-q', 'origin', 'scratch/probe')

    def test_metal_and_main_second_parents_are_accepted(self):
        other = self.metal_lane('metal-userspace/ramdisk')
        name = 'metal-network/integration'
        self.metal_lane(name)
        merged = self.merge(other)
        self.git('push', '-q', 'origin', name)
        self.assertEqual(self.policy(merged, 'x86-metal').returncode, 0)
        self.git('switch', '-q', 'main')
        main = self.commit('new main')
        self.git('push', '-q', 'origin', 'main')
        self.git('switch', '-q', name)
        merged = self.merge(main)
        self.git('push', '-q', 'origin', name)
        self.assertEqual(self.policy(merged, 'x86-metal').returncode, 0)

    def test_main_path_lane_cannot_hide_on_metal_first_parent(self):
        name = 'metal-bring-up/contaminated'
        self.metal_lane(name, 'breengel/window')
        for extra in ([], ['--catchup']):
            result = self.policy('HEAD', 'x86-metal', *extra)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('main-path effort work cannot run on metal', result.stderr)

    def test_deleted_main_effort_second_parent_still_refuses_metal(self):
        name = 'metal-bring-up/deleted-parent'
        self.metal_lane(name)
        merged = self.merge('breengel/window')
        self.git('push', '-q', 'origin', name)
        self.git('branch', '-D', 'breengel/suite', 'breengel/window')
        self.git('push', '-q', 'origin', '--delete', 'breengel/suite', 'breengel/window')
        for extra in ([], ['--catchup']):
            result = self.policy(merged, 'x86-metal', *extra)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('not first-parent reachable', result.stderr)

    def test_local_main_path_lane_and_unavailable_origin_refuse_metal(self):
        self.git('switch', '-qc', 'breengel/local-metal', 'main')
        local = self.commit('local main effort')
        self.git('push', '-q', 'origin', 'HEAD:refs/heads/metal-bring-up/shared')
        self.assertIn('effort lane breengel/local-metal', self.policy(local, 'x86-metal').stderr)
        self.git('remote', 'set-url', 'origin', str(self.base / 'unavailable'))
        for extra in ([], ['--catchup']):
            self.assertIn('cannot read origin', self.policy(self.main, 'x86-metal', *extra).stderr)

    def test_requested_sha_need_not_be_a_lane_tip(self):
        self.git('branch', '-D', 'breengel/suite')
        self.git('push', '-q', 'origin', '--delete', 'breengel/suite')
        self.assert_refused(self.policy(self.effort), 'breengel/window')

    def test_fix_and_scratch_names_do_not_exempt_effort_history(self):
        self.git('push', '-q', 'origin', self.effort + ':refs/heads/fix/shared')
        self.assert_refused(self.policy(self.effort))
        self.git('switch', '-qc', 'scratch/from-lane', 'breengel/window')
        probe = self.commit('probe from lane')
        self.git('push', '-q', 'origin', 'scratch/from-lane')
        self.assert_refused(self.policy(probe))
        # A main-based fix stays accepted while those branches exist.
        self.assertEqual(self.policy(self.fix).returncode, 0)

    def test_merged_effort_is_accepted(self):
        self.git('switch', '-q', 'main')
        self.git('merge', '-q', '--ff-only', 'breengel/window')
        self.git('push', '-q', 'origin', 'main')
        for sha in (self.effort, self.descendant):
            self.assertEqual(self.policy(sha).returncode, 0)

    def test_gate_tree_reads_the_projects_origin_not_the_canonical_cache(self):
        # The gate VM's canonical clone keeps a stale local main and an old lane branch;
        # its gate trees are shared clones of it, and their boots run this guard.
        canonical = self.base / 'canonical'
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(canonical)], check=True)
        subprocess.run(['git', '-C', str(canonical), 'branch', '-q', 'breengel/suite', 'origin/breengel/suite'],
                       check=True)
        self.git('switch', '-q', 'main')
        merged = self.merge('breengel/suite')
        self.git('push', '-q', 'origin', 'main')
        spec = importlib.util.spec_from_file_location('trees', ROOT / 'scripts/gate-tree.py')
        trees = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(trees)
        tree = self.base / 'cache/trees' / ('a' * 64)
        for sha in (merged, self.descendant):
            trees.checkout(str(canonical), tree, sha, 'a' * 64)
            shutil.copytree(self.repo / 'docs/efforts', tree / 'docs/efforts', dirs_exist_ok=True)
            result = self.invoke('python3', str(GUARD), '--repo', str(tree), '--target', 'x86')
            if sha == merged:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), merged)
            else:
                self.assert_refused(result, 'origin/breengel/window')

    def test_unpushed_local_lane_commit_is_refused(self):
        self.git('switch', '-qc', 'breengel/local', 'main')
        local = self.commit('unpushed lane work')
        self.assert_refused(self.policy(local, 'parallels'), 'breengel/local')
        result = self.invoke('/bin/bash', str(self.repo / 'run.sh'), '--dry-run', '--parallels')
        self.assert_refused(result, 'breengel/local')

    def test_lane_commit_pushed_under_another_name_is_refused(self):
        self.assertTrue(self.binary.is_file(), 'Missing breenix-runs; run scripts/build-host-test-tools.sh from the repository root')
        self.git('switch', '-qc', 'breengel/local', 'main')
        local = self.commit('lane work pushed elsewhere')
        self.git('push', '-q', 'origin', 'HEAD:refs/heads/wrb/tmp')
        result = self.invoke(str(self.binary), 'run', 'x86', '--sha', local, '--dry-run')
        self.assert_refused(result, 'breengel/local')

    def test_unknown_prefix_is_not_an_effort(self):
        self.git('switch', '-qc', 'not-an-effort/lane', 'main')
        sha = self.commit('unknown')
        self.git('push', '-q', 'origin', 'not-an-effort/lane')
        self.assertEqual(self.policy(sha).returncode, 0)

    def test_unreadable_origin_refuses_every_vm_target(self):
        self.git('remote', 'set-url', 'origin', str(self.base / 'unavailable'))
        for target in ('x86', 'parallels', 'vmware'):
            result = self.policy(self.main, target)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('cannot read origin', result.stderr)
        self.assertEqual(self.policy(self.effort, 'arm64-qemu').returncode, 0)

    def test_slow_origin_is_bounded_and_its_process_group_killed(self):
        pid_file = self.base / 'upload-pack.pid'
        slow = self.base / 'slow-upload-pack'
        # Ignores SIGTERM, so the guard must escalate to SIGKILL.
        slow.write_text('#!/bin/sh\ntrap "" TERM\necho $$ > ' + str(pid_file) + '\nexec sleep 60\n')
        slow.chmod(0o755)
        self.git('config', 'remote.origin.uploadpack', str(slow))
        started = time.monotonic()
        with patch.object(guard, 'TIMEOUT_SECONDS', 1), patch.object(guard, 'TERM_GRACE_SECONDS', 1):
            with self.assertRaisesRegex(ValueError, 'cannot read origin .*longer than 1 s'):
                guard.check(self.repo, self.main, 'x86', False)
        self.assertLess(time.monotonic() - started, 10)
        pid = int(pid_file.read_text())
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.1)
        else:
            self.fail('the timed-out upload-pack survived')
        locks = [str(path) for path in (self.repo / '.git').rglob('*.lock')]
        self.assertEqual(locks, [])

    def test_guard_writes_no_refs(self):
        # Change origin behind this checkout's back; a fetch would prune or add tracking refs.
        other = self.base / 'other'
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(other)], check=True)
        subprocess.run(['git', '-C', str(other), 'push', '-q', 'origin', '--delete', 'scratch/probe'], check=True)
        subprocess.run(['git', '-C', str(other), 'push', '-q', 'origin', self.main + ':refs/heads/fix/new'], check=True)
        before = self.git('for-each-ref')
        self.assertEqual(self.policy(self.main).returncode, 0)
        self.assertEqual(self.git('for-each-ref'), before)

    def test_unfetched_commit_is_fetched_and_unknown_commit_is_explained(self):
        other = self.base / 'other'
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(other)], check=True)
        self.configure(other)
        (other / 'file').write_text('pushed elsewhere')
        subprocess.run(['git', '-C', str(other), 'commit', '-qam', 'elsewhere'], check=True)
        subprocess.run(['git', '-C', str(other), 'push', '-q', 'origin', 'HEAD:refs/heads/fix/elsewhere'], check=True)
        sha = subprocess.run(['git', '-C', str(other), 'rev-parse', 'HEAD'], check=True,
                             text=True, capture_output=True).stdout.strip()
        result = self.policy(sha)
        self.assertEqual((result.returncode, result.stdout.strip()), (0, sha), result.stderr)
        result = self.policy('0123456789abcdef0123456789abcdef01234567')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Cannot resolve commit 0123456789abcdef', result.stderr)
        self.assertIn('git fetch failed', result.stderr)

    def test_checked_stamp_skips_only_the_same_target_and_commit(self):
        self.git('remote', 'set-url', 'origin', str(self.base / 'unavailable'))
        stamp = dict(self.env, BREENIX_TARGET_POLICY_CHECKED='x86:' + self.effort)
        self.assertEqual(self.policy(self.effort, env=stamp).returncode, 0)
        self.assertIn('cannot read origin', self.policy(self.effort, 'parallels', env=stamp).stderr)
        self.assertIn('cannot read origin', self.policy(self.descendant, env=stamp).stderr)

    def test_arm64_never_reads_origin_or_refuses(self):
        with patch.object(guard, 'origin', side_effect=AssertionError('network on ARM64')):
            self.assertEqual(guard.check(self.repo, self.effort, 'arm64-qemu', False), self.effort)

    def test_x86_qemu_lock_refuses_effort_before_locking(self):
        self.git('checkout', '-q', '--detach', self.effort)
        script = ('source "$1/docker/qemu/lib/qemu-host-lock.sh"; '
                  'qemu_host_lock_acquire qemu-system-x86_64; echo LOCK-ACQUIRED')
        result = self.invoke('/bin/bash', '-c', script, 'lock', str(self.repo))
        self.assertNotIn('LOCK-ACQUIRED', result.stdout)
        self.assert_refused(result)
        self.assertNotIn('QEMU HOST LOCK', result.stderr)

    def test_supervised_reexec_does_not_recheck(self):
        self.git('checkout', '-q', '--detach', self.main)
        result = self.invoke('/bin/bash', str(self.repo / 'run.sh'), '--dry-run', '--parallels',
                             env=dict(self.env, BREENIX_SLOT_SESSION=str(self.base / 'session')))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('Breenix target=', result.stdout)

    def test_vm_run_metadata_persists_catchup(self):
        source = (ROOT / 'run.sh').read_text()
        for platform in ('parallels', 'vmware'):
            self.assertRegex(source, r'\n    install_vm_boot_traps\n(?:    (?:export |\[ )[^\n]*\n)*'
                                     r'    record_launch_metadata\n(?:    #[^\n]*\n)*'
                                     r'    [^\n]*VIGIL_ID=\$\("\$BREENIX_ROOT/scripts/vigil-record\.sh" start '
                                     + platform + ' ')
        body = re.search(r'(?ms)^record_launch_metadata\(\) \{\n(.*?)^\}', source).group(1)
        self.git('checkout', '-q', '--detach', self.scratch)
        for target in ('parallels', 'vmware'):
            for catchup in ('0', '1'):
                serial = self.base / ('serial-' + target + catchup)
                env = dict(self.env, SERIAL_LOG=str(serial), TARGET=target, BREENIX_CATCHUP=catchup)
                result = self.invoke('/bin/bash', '-c', body, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(Path(str(serial) + '.launch.json').read_text()),
                                 {'target': target, 'catchup': catchup == '1', 'commit': self.scratch})

    def test_real_launcher_dry_invocations(self):
        self.assertTrue(self.binary.is_file(), 'Missing breenix-runs; run scripts/build-host-test-tools.sh from the repository root')
        launchers = {
            'breenix-runs x86': ('x86', None),
            'run.sh --x86': ('x86', ['--x86']),
            'run.sh --btrt --x86': ('x86', ['--btrt', '--x86']),
            'run.sh --parallels': ('parallels', ['--parallels']),
            'run.sh --vmware': ('vmware', ['--vmware']),
            'run.sh arm64': ('arm64-qemu', []),
        }
        for launcher, (target, flags) in launchers.items():
            for name, sha, catchup, accepted in (
                ('effort', self.effort, None, target == 'arm64-qemu'),
                ('main', self.main, None, True), ('fix', self.fix, None, True),
                ('scratch', self.scratch, None, True), ('catchup', self.effort, '1', True),
                ('catchup-true', self.effort, 'true', True)):
                with self.subTest(launcher=launcher, name=name):
                    env = self.env.copy()
                    if catchup is not None:
                        env['BREENIX_CATCHUP'] = catchup
                    if flags is None:
                        args = [str(self.binary), 'run', 'x86', '--sha', sha, '--dry-run']
                        if name == 'catchup':
                            args.append('--catchup')
                            env.pop('BREENIX_CATCHUP')
                    else:
                        self.git('checkout', '-q', '--detach', sha)
                        args = ['/bin/bash', str(self.repo / 'run.sh'), '--dry-run', *flags]
                    result = self.invoke(*args, env=env)
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)
                    first = result.stdout.splitlines()[0]
                    self.assertIn('Breenix target=' + target, first)
                    self.assertEqual(result.stdout.count('Breenix target='), 1)
                    if catchup is not None:
                        self.assertIn('catchup=true' if flags is None else 'BREENIX_CATCHUP=1', first)
                    if not accepted:
                        self.assert_refused(result)
                        self.assertNotIn('prepareClone:', result.stdout)
                    elif flags is None:
                        self.assertIn('BREENIX_TARGET_POLICY_CHECKED=x86:' + sha, self.start_gate(result.stdout))
                    print(f'dry {launcher} {name}: {"accepted" if accepted else "refused"}', flush=True)
        self.assertFalse((self.repo / 'target').exists(), 'dry runs must not build guests')
        runs = self.base / 'store/runs'
        self.assertFalse(runs.exists() and any(runs.iterdir()), 'dry runs must not launch runs')


if __name__ == '__main__':
    unittest.main()
