#!/usr/bin/env python3
"""Cache correctness and bounded eviction without a VM or Rust build."""
import importlib.util
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

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

    def build(self, repo, logs, label):
        for name, data in {'userspace/programs/example.elf': b'ELF bytes',
                           'target/test_binaries.img': b'test bytes',
                           'testdata/ext2.img': b'ext2 bytes'}.items():
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (repo / 'userspace/programs/target').mkdir(exist_ok=True)
        (repo / 'userspace/programs/target/stale').write_text('not a clean build')

    def call(self):
        with patch.object(artifacts, 'checked'), patch.object(artifacts, 'key', return_value='a' * 64), patch.object(artifacts.sys, 'argv',
                ['gate-artifacts.py', str(self.repo), str(self.logs)]):
            artifacts.main()

    def test_clean_verified_once_then_hit_restores_bytes_and_removes_obsolete_elf(self):
        with patch.object(artifacts, 'build', side_effect=self.build) as build:
            self.call()
            self.assertEqual([call.args[2] for call in build.call_args_list], ['candidate', 'clean-verification'])
            elf = self.repo / 'userspace/programs/example.elf'
            elf.write_bytes(b'stale')
            (elf.parent / 'obsolete.elf').write_bytes(b'obsolete')
            self.call()
            self.assertEqual(build.call_count, 2)
            self.assertEqual(elf.read_bytes(), b'ELF bytes')
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
        names = ['Cargo.lock', 'userspace/programs/Cargo.lock', 'userspace/source.rs', 'scripts/create_ext2_disk.sh', 'vendor/busybox/manifest.json', 'rust-toolchain.toml']
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
                return b'\0'.join(name.encode() for name in names)
            if command[:2] == ['rustup', 'which']:
                return str(compiler)
            if '--print' in command:
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

    def test_hard_floor_checks_whole_filesystem_even_on_hit(self):
        usage = __import__('collections').namedtuple('Usage', 'total used free')(100, 99, 1)
        with patch.dict(os.environ, BREENIX_GATE_FREE_GB='15'), patch.object(trees.shutil, 'disk_usage', return_value=usage):
            with self.assertRaisesRegex(RuntimeError, 'below safe floor'):
                trees.prune(self.cache, protected=(self.repo,))
            trees.prune(self.cache, protected=(self.repo,), required=False)

    def test_fresh_without_verified_entry_fails_explicitly(self):
        with patch.object(artifacts, 'build', side_effect=self.build), patch.dict(os.environ, BREENIX_GATE_FRESH='1'):
            with self.assertRaisesRegex(RuntimeError, 'requires a verified cache entry'):
                self.call()



if __name__ == '__main__':
    unittest.main()
