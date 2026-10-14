"""Exercise fork materialization and refusal paths without building or booting."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/rust-fork.sh'

class ForkPinTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / 'breenix'
        self.root.mkdir()
        (self.root / 'scripts').mkdir()
        (self.root / 'libs/libc').mkdir(parents=True)
        shutil.copy(SCRIPT, self.root / 'scripts/rust-fork.sh')
        self.source = Path(self.temp.name) / 'source'
        self.source.mkdir()
        self.git('init', '-b', 'breenix-target')
        self.git('config', 'user.email', 'test@example.com')
        self.git('config', 'user.name', 'Test')
        (self.source / 'library/std').mkdir(parents=True)
        (self.source / 'library/std/Cargo.toml').write_text('[package]\nname="std"\n')
        (self.source / 'library/Cargo.toml').write_text('[patch.crates-io]\nlibc={path="../../libs/libc"}\n')
        (self.source / 'src').mkdir()
        (self.source / 'src/version').write_text('1.90.0\n')
        self.git('add', '.')
        self.git('commit', '-m', 'Initial library')
        self.sha = self.git('rev-parse', 'HEAD')
        self.git('remote', 'add', 'origin', str(self.source))
        # No submodules in the fixture; sync's targeted paths need empty gitlinks.
        self.env = dict(os.environ, BREENIX_RUST_FORK_SOURCE=str(self.source))
        for name in ('BREENIX_RUST_FORK_LIBRARY', 'BREENIX_RUST_FORK_DEV'):
            self.env.pop(name, None)
        self.pin()
        subprocess.run(['git', '-C', str(self.source), 'worktree', 'add', '-b', 'rust-std/thread', str(self.root / 'rust-fork'), self.sha], check=True, capture_output=True)

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args, where=None):
        return subprocess.check_output(['git', '-C', str(where or self.source), *args], text=True, stderr=subprocess.DEVNULL).strip()

    def pin(self, repo=None, branch='breenix-target'):
        (self.root / 'rust-fork.toml').write_text(f'repo="{repo or self.source}"\nbranch="{branch}"\ncommit="{self.sha}"\nrust="1.90.0"\n')

    def snapshot(self):
        destination = self.root / 'gate-rust-fork'
        return subprocess.run([str(self.root / 'scripts/rust-fork.sh'), 'snapshot', str(self.source), str(destination)], env=self.env, text=True, capture_output=True)

    def publish_lane_commit(self):
        # A commit published only on a lane branch of a remote the canonical clone has not fetched.
        remote = Path(self.temp.name) / 'remote'
        subprocess.run(['git', 'clone', '--quiet', '--branch', 'breenix-target', str(self.source), str(remote)], check=True, capture_output=True)
        self.git('switch', '--quiet', '-c', 'rust-std/lane', where=remote)
        (remote / 'library/lane.rs').write_text('lane')
        self.git('add', '.', where=remote)
        self.git('commit', '-m', 'Lane change', where=remote)
        self.sha = self.git('rev-parse', 'HEAD', where=remote)
        return remote

    def verify(self, command='verify'):
        return subprocess.run([str(self.root / 'scripts/rust-fork.sh'), command], env=self.env, text=True, capture_output=True)

    def test_sync_replaces_shared_link_with_lane_worktree(self):
        fork = self.root / 'rust-fork'
        self.git('worktree', 'remove', str(fork))
        self.git('branch', '-D', 'rust-std/thread')
        fork.symlink_to(self.source)
        result = subprocess.run([str(self.root / 'scripts/rust-fork.sh'), 'sync', '--branch', 'rust-std/thread'], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(fork.is_symlink())
        self.assertEqual(self.git('branch', '--show-current', where=fork), 'rust-std/thread')
        self.assertEqual(self.verify().returncode, 0)

    def test_snapshot_uses_committed_pin_and_rejects_edits(self):
        fork = self.root / 'rust-fork'
        (fork / 'library/std/Cargo.toml').write_text('dirty input must not be copied')
        destination = self.root / 'gate-rust-fork'
        result = subprocess.run([str(self.root / 'scripts/rust-fork.sh'), 'snapshot', str(self.source), str(destination)], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('[package]', (destination / 'library/std/Cargo.toml').read_text())
        self.env['BREENIX_RUST_FORK_LIBRARY'] = str(destination / 'library')
        self.assertEqual(self.verify().returncode, 0)
        (destination / 'library/std/Cargo.toml').write_text('changed')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_clean_pin(self):
        self.assertEqual(self.verify().returncode, 0)

    def test_dirty_library_is_rejected(self):
        (self.root / 'rust-fork/library/std/Cargo.toml').write_text('changed')
        self.assertIn('dirty', self.verify().stderr)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_untracked_input_is_rejected(self):
        (self.root / 'rust-fork/library/input.rs').write_text('changed')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_descendant_needs_explicit_development_override(self):
        fork = self.root / 'rust-fork'
        (fork / 'library/new.rs').write_text('new')
        self.git('add', '.', where=fork)
        self.git('commit', '-m', 'Lane change', where=fork)
        self.assertNotEqual(self.verify().returncode, 0)
        self.env['BREENIX_RUST_FORK_DEV'] = '1'
        result = self.verify()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('UNPINNED', result.stderr)

    def test_shared_libc_binding_is_rejected(self):
        self.env['BREENIX_RUST_FORK_LIBRARY'] = str(self.source / 'library')
        result = self.verify()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('this Breenix checkout', result.stderr)

    def test_release_pin_requires_remote_reachability(self):
        fork = self.root / 'rust-fork'
        self.assertEqual(self.verify('check-reachable').returncode, 0)
        self.assertEqual(self.verify('check-release').returncode, 0)
        (fork / 'library/new.rs').write_text('new')
        self.git('add', '.', where=fork)
        self.git('commit', '-m', 'Unpublished lane', where=fork)
        self.sha = self.git('rev-parse', 'HEAD', where=fork)
        self.pin()
        result = self.verify('check-release')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not reachable from breenix-target', result.stderr)
        self.assertEqual(self.git('for-each-ref', 'refs/rust-fork-check'), '')

    def test_snapshot_fetches_a_pin_only_on_a_lane_branch(self):
        remote = self.publish_lane_commit()
        self.pin(repo=remote, branch='rust-std/lane')
        result = self.snapshot()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / 'gate-rust-fork/library/lane.rs').read_text(), 'lane')
        self.assertEqual(self.verify('check-reachable').returncode, 0)
        result = self.verify('check-release')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('main may only pin a commit on breenix-target', result.stderr)

    def test_present_pin_needs_no_fetch(self):
        self.pin(repo=Path(self.temp.name) / 'unreachable')
        result = self.snapshot()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_pin_is_reported(self):
        self.publish_lane_commit()
        self.pin()
        result = self.snapshot()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('fetch', result.stderr)

if __name__ == '__main__':
    unittest.main()
