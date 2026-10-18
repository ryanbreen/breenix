#!/usr/bin/env python3
"""Exercise linker freshness and stale installation with retained Cargo outputs."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class IncrementalTests(unittest.TestCase):
    def test_linker_only_edit_reruns_real_build_script_with_retained_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            project = root / 'userspace/programs'
            (project / 'src').mkdir(parents=True)
            (root / 'scripts').mkdir()
            verify = root / 'scripts/rust-fork.sh'
            verify.write_text('#!/bin/sh\nexit 0\n')
            verify.chmod(0o755)
            (root / 'rust-fork.toml').write_text('')
            (project / 'Cargo.toml').write_text('[package]\nname="freshness-fixture"\nversion="0.1.0"\nedition="2021"\n')
            (project / 'src/main.rs').write_text('fn main() {}\n')
            shutil.copyfile(ROOT / 'userspace/programs/build.rs', project / 'build.rs')
            linker = project / 'linker.ld'
            linker.write_text('/* initial script */\n')
            # Supply the separately built archive the real build script tracks.
            archive = root / 'libs/libbreenix-libc/target/aarch64-breenix/release/libc.a'
            host = subprocess.check_output(['rustc', '-vV'], text=True)
            arch = next(line.split(': ')[1].split('-')[0] for line in host.splitlines() if line.startswith('host:'))
            archive = Path(str(archive).replace('aarch64-breenix', arch + '-breenix'))
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b'!<arch>\n')
            env = dict(os.environ, CARGO_BUILD_JOBS='4')
            for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR'):
                env.pop(key, None)
            def build():
                result = subprocess.run(['cargo', 'build', '--offline'], cwd=project, env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn('warning:', result.stderr)
            build()
            executable = project / 'target/debug/freshness-fixture'
            first = executable.stat().st_mtime_ns
            build()
            self.assertEqual(executable.stat().st_mtime_ns, first)
            linker.write_text('/* changed linker input only */\n')
            # Keep the edit newer even on coarse-timestamp filesystems.
            os.utime(linker, ns=(first + 2_000_000_000, first + 2_000_000_000))
            build()
            self.assertNotEqual(executable.stat().st_mtime_ns, first)

    def test_install_validation_rejects_removed_target_despite_stale_executable(self):
        source = (ROOT / 'userspace/programs/build.sh').read_text()
        validation = source[source.index('CURRENT_TARGETS='):source.index('# Check producer names')]
        with tempfile.TemporaryDirectory() as tmp:
            project = Path(tmp)
            stale = project / 'target/x86_64-breenix/release/removed'
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b'old executable')
            metadata = {'packages': [{'manifest_path': str(project / 'Cargo.toml'),
                                     'targets': [{'name': 'current', 'kind': ['bin']}]}]}
            cargo = project / 'cargo'
            cargo.write_text('#!/bin/sh\ncat <<\'JSON\'\n' + json.dumps(metadata) + '\nJSON\n')
            cargo.chmod(0o755)
            env = dict(os.environ, SCRIPT_DIR=str(project), PATH=str(project) + ':' + os.environ['PATH'])
            for name, status in [('current:installed', 0), ('removed', 1), ('renamed', 1)]:
                result = subprocess.run(['bash', '-ec', 'STD_BINARIES=("' + name + '")\n' + validation],
                                        env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, status, result.stderr)
            self.assertTrue(stale.exists())


if __name__ == '__main__':
    unittest.main()
