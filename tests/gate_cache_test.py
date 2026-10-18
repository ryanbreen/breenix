#!/usr/bin/env python3
"""Cache correctness and bounded eviction without a VM or Rust build."""
import importlib.util
from contextlib import redirect_stdout
import io
import json
import os
import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('artifacts', ROOT / 'scripts/gate-artifacts.py')
artifacts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifacts)
trees = artifacts.tree_cache


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cache = self.root / 'cache'
        self.repo = self.cache / 'trees' / 'lane'
        self.logs = self.root / 'logs'
        self.logs.mkdir()
        self.repo.mkdir(parents=True)
        self.cache.mkdir(exist_ok=True)
        # These cache-control tests model the Linux execution host; their
        # builders and tool probes are mocked so they also run on macOS.
        platform = patch.object(artifacts.sys, 'platform', 'linux')
        platform.start()
        self.addCleanup(platform.stop)
        self.env = patch.dict(os.environ, BREENIX_GATE_CACHE_DIR=str(self.cache),
                             BREENIX_GATE_FREE_GB='0', BREENIX_GATE_CACHE_GB='12',
                             CARGO_HOME=str(self.root / 'cargo'), BREENIX_GATE_FRESH='0')
        self.env.start()
        self.addCleanup(self.env.stop)
        # The key follows a caller's library override; these tests model the
        # tree's own rust-fork, so the override is cleared (patch.dict restores it).
        os.environ.pop('BREENIX_RUST_FORK_LIBRARY', None)

    def build(self, repo, logs, label, targets=None):
        for name, data in {'userspace/programs/example.elf': b'ELF bytes',
                           'userspace/programs/libc-busybox.links': b'cat\nsh\n',
                           'target/test_binaries.img': b'test bytes',
                           'testdata/ext2.img': b'ext2 bytes'}.items():
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (repo / 'userspace/programs/target').mkdir(exist_ok=True)
        (repo / 'userspace/programs/target/stale').write_text('not a clean build')

    def call(self, *flags):
        with patch.object(artifacts, 'checked'), patch.object(artifacts, 'key', return_value='a' * 64), \
                patch.object(artifacts, 'metal_toolchain_key', return_value='b' * 64), \
                patch.object(artifacts, 'metal_source_times'), patch.object(artifacts.sys, 'argv',
                ['gate-artifacts.py', *flags, str(self.repo), str(self.logs)]):
            artifacts.main()

    def test_clean_verified_once_then_hit_restores_bytes_and_removes_obsolete_elf(self):
        with patch.object(artifacts, 'build', side_effect=self.build) as build:
            self.call()
            self.assertEqual([call.args[2] for call in build.call_args_list], ['candidate', 'clean-verification'])
            elf = self.repo / 'userspace/programs/example.elf'
            elf.write_bytes(b'stale')
            links = elf.parent / 'libc-busybox.links'
            links.unlink()
            (elf.parent / 'obsolete.elf').write_bytes(b'obsolete')
            self.call()
            self.assertEqual(build.call_count, 2)
            self.assertEqual(elf.read_bytes(), b'ELF bytes')
            self.assertEqual(links.read_bytes(), b'cat\nsh\n')
            self.assertFalse((elf.parent / 'obsolete.elf').exists())

    def test_fresh_checkout_compares_against_existing_verified_entry(self):
        with patch.object(artifacts, 'build', side_effect=self.build) as build:
            self.call()
            fresh = self.root / 'fresh'
            fresh.mkdir()
            self.repo = fresh
            with patch.dict(os.environ, BREENIX_GATE_FRESH='1'):
                self.call()
            self.assertEqual(build.call_count, 3)
            self.assertEqual((fresh / 'userspace/programs/example.elf').read_bytes(), b'ELF bytes')

    def test_fresh_checkout_mismatch_is_rejected(self):
        with patch.object(artifacts, 'build', side_effect=self.build):
            self.call()
        fresh = self.root / 'fresh'
        fresh.mkdir()
        self.repo = fresh
        def wrong(repo, logs, label):
            self.build(repo, logs, label)
            (repo / 'userspace/programs/example.elf').write_bytes(b'path-dependent')
        with patch.object(artifacts, 'build', side_effect=wrong), patch.dict(os.environ, BREENIX_GATE_FRESH='1'):
            with self.assertRaisesRegex(RuntimeError, 'fresh checkout artifacts differ'):
                self.call()

    def test_failed_build_emits_phase_duration_and_failure_status(self):
        output = io.StringIO()
        with patch.object(artifacts, 'checked', side_effect=RuntimeError('compiler error')), redirect_stdout(output):
            with self.assertRaisesRegex(RuntimeError, 'compiler error'):
                artifacts.build(self.repo, self.logs, 'candidate')
        lines = output.getvalue().splitlines()
        self.assertTrue(any('phase=userspace-build' in line and 'ended=' in line and 'seconds=' in line and 'status=1' in line for line in lines))

    def test_mismatch_never_publishes_verified_key(self):
        def unequal(repo, logs, label):
            self.build(repo, logs, label)
            if label == 'clean-verification':
                (repo / 'userspace/programs/example.elf').write_bytes(b'different')
        with patch.object(artifacts, 'build', side_effect=unequal):
            with self.assertRaisesRegex(RuntimeError, 'differs from clean'):
                self.call()
        self.assertFalse((self.cache / 'artifacts' / ('a' * 64) / 'verified.json').exists())

    def test_corrupt_cached_disk_fails_instead_of_booting_or_rebuilding(self):
        with patch.object(artifacts, 'build', side_effect=self.build) as build:
            self.call()
            (self.cache / 'artifacts' / ('a' * 64) / 'testdata/ext2.img').write_bytes(b'corrupt')
            with self.assertRaisesRegex(RuntimeError, 'corrupt artifact cache'):
                self.call()
            self.assertEqual(build.call_count, 2)

    def test_content_key_tracks_source_script_busybox_toolchain_and_external_library(self):
        names = ['Cargo.lock', 'userspace/programs/Cargo.lock', 'userspace/source.rs', 'scripts/create_ext2_disk.sh', 'scripts/install-libc-busybox.sh', 'scripts/libc-clean-archive.py', 'vendor/busybox/manifest.json', 'rust-toolchain.toml']
        for name in names:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        library = self.repo / 'rust-fork/library'
        library.mkdir(parents=True)
        (library / 'std.rs').write_text('std')
        version = [b'tool v1']
        compiler = self.root / 'compiler'
        compiler.write_bytes(b'compiler bytes')
        def output(command, **kwargs):
            if 'ls-files' in command:
                return b'\0'.join(name.encode() for name in names if any(name == selected or name.startswith(selected + '/') for selected in command[command.index('--') + 1:]))
            if command[:2] == ['rustup', 'which']:
                return str(compiler)
            if '--print' in command or '--tool-dir' in command:
                return str(self.root)
            return version[0]
        with patch.object(artifacts.subprocess, 'check_output', side_effect=output):
            previous = artifacts.key(self.repo)
            for name in names + ['rust-fork/library/std.rs']:
                with (self.repo / name).open('a') as handle:
                    handle.write('changed')
                current = artifacts.key(self.repo)
                self.assertNotEqual(previous, current, name)
                previous = current
            version[0] = b'tool v2'
            self.assertNotEqual(previous, artifacts.key(self.repo))
            previous = artifacts.key(self.repo)
            stock = self.root / 'lib/rustlib/src/rust/library/core/src/lib.rs'
            stock.parent.mkdir(parents=True)
            stock.write_text('stock core source')
            self.assertNotEqual(previous, artifacts.key(self.repo))
            previous = artifacts.key(self.repo)
            alias = self.root / 'library-alias'
            alias.symlink_to(library, target_is_directory=True)
            with patch.dict(os.environ, BREENIX_RUST_FORK_LIBRARY=str(alias)):
                self.assertNotEqual(previous, artifacts.key(self.repo))

    def test_repack_uses_breenix_userspace_without_foreign_busybox_installer(self):
        disk = self.repo / 'testdata/ext2.img'
        disk.parent.mkdir()
        disk.write_bytes(b'disk')
        (self.repo / 'target').mkdir()
        with patch.dict(os.environ, CARGO_HOME=str(self.root / 'cargo')), \
                patch.object(artifacts, 'checked') as checked, \
                patch.object(artifacts, 'canonical_ext2'):
            artifacts.build(self.repo, self.logs, 'candidate')
        commands = [call.args[0] for call in checked.call_args_list]
        self.assertEqual(len(commands), 3)
        self.assertIn('build.sh', ' '.join(commands[0]))
        self.assertEqual(commands[1][-1], 'create-test-disk')
        self.assertEqual(commands[2], [str(self.repo / 'scripts/create_ext2_disk.sh')])

    def test_eviction_skips_leased_entries_and_removes_old_idle_entry(self):
        os.utime(self.repo, (1, 1))
        old = self.cache / 'artifacts/old'
        old.mkdir(parents=True)
        (old / 'data').write_bytes(b'a')
        os.utime(old, (2, 2))
        active = self.repo
        with trees.lease(self.cache, 'trees-lane'), patch.dict(os.environ, BREENIX_GATE_CACHE_GB='1'), \
             patch.object(trees, 'size', return_value=700 * 1024**2):
            trees.prune(self.cache)
        self.assertFalse(old.exists())
        self.assertTrue(active.exists())

    def test_soft_budget_retains_active_entry(self):
        with trees.lease(self.cache, 'trees-lane'), patch.dict(os.environ, BREENIX_GATE_CACHE_GB='0'), \
             patch.object(trees, 'size', return_value=1):
            trees.prune(self.cache)
            self.assertTrue(self.repo.exists())

    def test_abandoned_clone_cleanup_uses_process_ownership_not_directory_mtime(self):
        canonical = self.root / 'breenix'
        live = self.root / 'breenix-20200101T000000Z-x86_64-gate-abcd'
        abandoned = self.root / 'breenix-20200101T000000Z-x86_64-gate-beef'
        for clone in (live, abandoned):
            (clone / '.git').mkdir(parents=True)
        os.utime(live, (1, 1))
        proc = self.root / 'proc'
        process = proc / '123'
        (process / 'fd').mkdir(parents=True)
        (process / 'cmdline').write_bytes(b'kernel builder')
        (process / 'cwd').symlink_to(live, target_is_directory=True)
        trees.reclaim_legacy(str(canonical), processes=proc)
        self.assertTrue(live.exists())
        self.assertFalse(abandoned.exists())

    def test_hard_floor_checks_whole_filesystem_even_on_hit(self):
        usage = __import__('collections').namedtuple('Usage', 'total used free')(100, 99, 1)
        with patch.dict(os.environ, BREENIX_GATE_FREE_GB='15'), patch.object(trees.shutil, 'disk_usage', return_value=usage):
            with self.assertRaisesRegex(RuntimeError, 'below safe floor'):
                trees.prune(self.cache, protected=(self.repo,))
            trees.prune(self.cache, protected=(self.repo,), required=False)

    def test_metal_builds_once_and_its_entry_is_never_verified_for_the_x86_gate(self):
        helper = patch.dict(os.environ, HOST_SLOTS_HELPER='/nonexistent', BREENIX_SLOT_SESSION='/nonexistent')
        with helper, patch.object(artifacts, 'build', side_effect=self.build) as build, \
                patch.object(artifacts.subprocess, 'run') as slot:
            self.call('--metal')
            self.assertEqual([call.args[2] for call in build.call_args_list], ['metal'])
            slot.assert_not_called()  # the metal caller holds its metal-build lease
            entry = self.cache / 'metal-artifacts' / artifacts.metal_cache_key('a' * 64)
            self.assertTrue((entry / 'built-once.json').exists())
            self.assertFalse((self.cache / 'artifacts' / ('a' * 64)).exists())
            (self.repo / 'userspace/programs/example.elf').write_bytes(b'stale')
            self.call('--metal')
            self.assertEqual(build.call_count, 1)
            self.assertEqual((self.repo / 'userspace/programs/example.elf').read_bytes(), b'ELF bytes')
            # The same inputs at the x86 gate still build twice and verify.
            self.call()
            self.assertEqual([call.args[2] for call in build.call_args_list[1:]], ['candidate', 'clean-verification'])

    def test_redeployed_metal_builder_cannot_reuse_previous_artifacts(self):
        helper = self.root / 'deployed-gate-artifacts.py'
        helper.write_text('builder version one')
        with patch.object(artifacts, '__file__', str(helper)), \
                patch.object(artifacts, 'build', side_effect=self.build) as build:
            self.call('--metal')
            helper.write_text('builder version two')
            self.call('--metal')
            self.assertEqual(build.call_count, 2)
            self.assertEqual(len(list((self.cache / 'metal-artifacts').glob('*/built-once.json'))), 2)

    def test_metal_input_miss_reuses_targets_under_exclusive_lease_and_failure_releases_it(self):
        targets = self.cache / 'metal-targets' / ('b' * 64)
        lock = 'metal-targets-' + targets.name
        def build(repo, logs, label, *, targets):
            self.assertIsNone(trees.lease(self.cache, lock, blocking=False))
            marker = targets / 'programs/compiled'
            if marker.exists():
                self.assertEqual(marker.read_text(), 'previous build')
                raise RuntimeError('compiler rejected changed source')
            marker.write_text('previous build')
            self.build(repo, logs, label)
        with patch.object(artifacts, 'build', side_effect=build):
            self.call('--metal')
            # A different exact input key misses, but retains the same compiler pool.
            with patch.object(artifacts, 'metal_cache_key', return_value='c' * 64):
                with self.assertRaisesRegex(RuntimeError, 'compiler rejected'):
                    self.call('--metal')
        self.assertFalse((self.cache / 'metal-artifacts' / ('c' * 64) / 'built-once.json').exists())
        with trees.lease(self.cache, lock, blocking=False) as lease:
            self.assertIsNotNone(lease)

    def test_metal_source_times_compare_bytes_and_changed_inputs_are_newer_than_waiting_clone(self):
        subprocess.run(['git', 'init', '--quiet', str(self.repo)], check=True)
        source = self.repo / 'userspace/programs/src/main.rs'
        source.parent.mkdir(parents=True)
        source.write_text('first version')
        subprocess.run(['git', '-C', str(self.repo), 'add', '.'], check=True)
        fork = self.repo / 'gate-rust-fork/library/std/src/lib.rs'
        fork.parent.mkdir(parents=True)
        fork.write_text('std input')
        (self.repo / 'rust-fork').symlink_to('gate-rust-fork')
        targets = self.cache / 'metal-targets/compiler'
        targets.mkdir(parents=True)
        with patch.object(artifacts.time, 'time_ns', return_value=10000000000):
            artifacts.metal_source_times(self.repo, targets)
        os.utime(source, ns=(1, 1))  # a new clone's timestamps are irrelevant
        with patch.object(artifacts.time, 'time_ns', return_value=20000000000):
            artifacts.metal_source_times(self.repo, targets)
        self.assertEqual(source.stat().st_mtime_ns, 10000000000)
        self.assertEqual(fork.stat().st_mtime_ns, 10000000000)
        source.write_text('other version')  # same length, older checkout timestamp
        os.utime(source, ns=(1, 1))
        with patch.object(artifacts.time, 'time_ns', return_value=30000000000):
            artifacts.metal_source_times(self.repo, targets)
        self.assertEqual(source.stat().st_mtime_ns, 30000000000)
        self.assertEqual(fork.stat().st_mtime_ns, 10000000000)

    def test_metal_targets_are_evicted_after_failure_when_over_budget(self):
        def failed(repo, logs, label, *, targets):
            (targets / 'programs/partial').write_bytes(b'partial build')
            raise RuntimeError('compile failed')
        with patch.dict(os.environ, BREENIX_GATE_CACHE_GB='0'), \
                patch.object(artifacts, 'build', side_effect=failed):
            with self.assertRaisesRegex(RuntimeError, 'compile failed'):
                self.call('--metal')
        self.assertFalse((self.cache / 'metal-targets' / ('b' * 64)).exists())

    def test_fresh_without_verified_entry_fails_explicitly(self):
        with patch.object(artifacts, 'build', side_effect=self.build), patch.dict(os.environ, BREENIX_GATE_FRESH='1'):
            with self.assertRaisesRegex(RuntimeError, 'requires a verified cache entry'):
                self.call()

    def test_gate_refuses_missing_container_configuration_or_builder(self):
        lane = 'a' * 64
        tree = self.cache / 'trees' / lane
        (tree / 'scripts').mkdir(parents=True)
        host = self.root / 'build-host.env'
        builder = tree / 'scripts/x86-gate-build.py'
        for missing in ('host', 'builder'):
            with self.subTest(missing=missing):
                host.write_text('configured')
                builder.touch()
                (host if missing == 'host' else builder).unlink()
                with patch.object(trees, 'BUILD_HOST', host), \
                        patch.object(trees, 'reclaim_legacy'), \
                        patch.object(trees, 'checkout'), \
                        patch.object(trees.subprocess, 'Popen') as child, \
                        patch.object(trees.sys, 'argv', ['gate-tree.py', 'repo', lane, 'b' * 40,
                                                       str(self.logs), '1', 'full']):
                    with self.assertRaisesRegex(RuntimeError, 'refusing to build in the gate VM'):
                        trees.main()
                    child.assert_not_called()

    def test_gate_runs_container_then_boot_only(self):
        lane = 'a' * 64
        tree = self.cache / 'trees' / lane
        (tree / 'scripts').mkdir(parents=True)
        (tree / 'scripts/x86-gate-build.py').touch()
        host = self.root / 'build-host.env'
        host.touch()
        commands = []

        def launch(command, **kwargs):
            commands.append((command, dict(kwargs['env'])))
            if len(commands) == 1:
                (self.logs / 'x86-build.json').write_text(json.dumps({'build_seconds': 42}))
            child = Mock(returncode=0)
            child.poll.return_value = 0
            return child

        with patch.object(trees, 'BUILD_HOST', host), \
                patch.object(trees, 'reclaim_legacy'), patch.object(trees, 'checkout'), \
                patch.object(trees.signal, 'signal'), \
                patch.object(trees.subprocess, 'Popen', side_effect=launch), \
                patch.object(trees.sys, 'argv', ['gate-tree.py', 'repo', lane, 'b' * 40,
                                               str(self.logs), '1', 'full']):
            self.assertEqual(trees.main(), 0)
        self.assertEqual(len(commands), 2)
        self.assertEqual(commands[0][0][1:3], [str(tree / 'scripts/x86-gate-build.py'), 'remote'])
        self.assertEqual(commands[1][0], [str(tree / 'docker/qemu/run-x86-gate.sh'), '1', 'full'])
        self.assertEqual(commands[1][1]['BREENIX_GATE_PHASE'], 'boot')
        self.assertEqual(commands[1][1]['BREENIX_GATE_BUILD_SECS'], '42')



if __name__ == '__main__':
    unittest.main()
