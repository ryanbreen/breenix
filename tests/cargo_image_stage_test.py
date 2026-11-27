"""Read back the Cargo selection on a private ext2 image."""

from contextlib import redirect_stderr, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tests.rust_link_image_stage_test import FIXTURE, RECORDS, ROOT, STAGE, read_json

PREFIX = '/usr/libexec/breenix/rustc'
CORPUS = '/usr/libexec/breenix/cargo/corpus'
HEADROOM = 64 * 1024 * 1024
PROGRAMS = ROOT / 'userspace/programs/target'


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class CargoImageStageTest(unittest.TestCase):
    def test_arm_cargo_image_matches_records_and_size(self):
        self.check_arm_image('cargo')

    def test_arm_rustc_and_cargo_share_hosted_libc(self):
        self.check_arm_image('rustc,cargo')

    def test_arm_cargo_selection_rejects_unported_suites(self):
        for suite in ('d1', 'self-host'):
            with self.subTest(suite=suite), redirect_stderr(io.StringIO()) as stderr:
                argv = ['rust_host_stage.py', '--arch', 'aarch64', '--image', 'unused.img',
                        '--suite', f'cargo,{suite}']
                with patch('sys.argv', argv), self.assertRaises(SystemExit) as error:
                    STAGE.main()
                self.assertEqual(error.exception.code, 2)
                self.assertIn(f'hosted {suite} staging requires x86-64', stderr.getvalue())

    def check_arm_image(self, selection):
        mke2fs, debugfs, dumpe2fs = STAGE.require_e2fsprogs('mke2fs', 'debugfs', 'dumpe2fs')
        k5 = read_json(RECORDS / 'k5/aarch64.json')
        k6 = read_json(RECORDS / 'k6/aarch64.json')
        k7 = read_json(RECORDS / 'k7/aarch64.json')
        kd = Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'kd/aarch64'))
        release = PROGRAMS / 'hosted/aarch64-breenix-hosted/release'
        rust_link = PROGRAMS / 'rust-link/aarch64'
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            image = directory / 'cargo.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-b', '4096', '-F', image, '16M'],
                           check=True, capture_output=True)
            before = image.stat().st_size
            argv = ['rust_host_stage.py', '--arch', 'aarch64', '--image', str(image),
                    '--suite', selection]
            stdout = io.StringIO()
            with patch('sys.argv', argv), redirect_stdout(stdout):
                STAGE.main()

            driver = k5['hosted_link']['driver']
            libc = read_json(RECORDS / 'hosted-libc-aarch64.json')['sha256']['libc.so']
            self.assertEqual(libc, k5['hosted_link']['libc_sha256'])
            self.assertEqual(libc, k7['libc_sha256'])
            expected = {
                '/lib/libc.so': libc,
                '/lib/ld-breenix.so': sha256(kd / 'ld-breenix.so'),
                f'{PREFIX}/bin/cargo': k7['cargo_sha256'],
                f'{PREFIX}/bin/rustc': k5['hosted_rustc_sha256'],
                f'{PREFIX}/lib/{driver}': k5['hosted_driver_sha256'],
                f'{PREFIX}/bin/bld': read_json(rust_link / 'build.json')['files_sha256']['bld'],
            }
            for name in ('libc.a', 'linker.ld'):
                expected[f'{PREFIX}/lib/{name}'] = k6['inputs'][f'lib/tools/{name}']['sha256']
            for name, digest in k5['sysroot']['files_sha256'].items():
                if name.startswith('lib/rustlib/aarch64-breenix/'):
                    expected[f'{PREFIX}/sysroot/{name}'] = digest
            for name, digest in k7['fixture_sha256'].items():
                if name.startswith('corpus/'):
                    expected[f'{CORPUS}/{name.removeprefix("corpus/")}'] = digest
                if name.startswith('targets/'):
                    expected[f'{PREFIX}/{name}'] = digest
            for suite in selection.split(','):
                runner = f'suite-{suite}'
                expected[f'/sbin/{runner}'] = sha256(release / runner)
            if 'rustc' in selection.split(','):
                for name in ('hello.rs', 'type-error.rs', 'uses-std.rs', 'expect.json'):
                    source = 'expect-aarch64.json' if name == 'expect.json' else name
                    expected[f'{PREFIX}/corpus/{name}'] = sha256(RECORDS / 'k5/corpus' / source)
            self.assertEqual(json.loads(stdout.getvalue()), expected)

            dump = directory / 'readback'
            dump.mkdir()
            subprocess.run([debugfs, '-R', f'rdump / {dump}', image], check=True, capture_output=True)
            files = {f'/{path.relative_to(dump)}': path for path in dump.rglob('*') if path.is_file()}
            self.assertEqual({name: sha256(path) for name, path in files.items()}, expected)
            self.assertEqual([name for name in files if name.endswith('/libc.so')], ['/lib/libc.so'])
            self.check_arm_cargo_imports(files[f'{PREFIX}/bin/cargo'], files['/lib/libc.so'], k7)

            summary = subprocess.check_output([dumpe2fs, '-h', image], text=True,
                                              stderr=subprocess.DEVNULL)
            superblock = dict(line.split(':', 1) for line in summary.splitlines() if ':' in line)
            block_size = int(superblock['Block size'])
            payload = sum(path.stat().st_size for path in files.values())
            blocks, partial = divmod(before + payload + HEADROOM, block_size)
            self.assertEqual(int(superblock['Block count']), blocks + bool(partial))
            self.assertEqual(image.stat().st_size, (blocks + bool(partial)) * block_size)
            self.assertGreaterEqual(int(superblock['Free blocks']) * block_size, HEADROOM)

    def check_arm_cargo_imports(self, cargo, libc, k7):
        libdir = subprocess.check_output(['rustc', '--print', 'target-libdir'], cwd=ROOT,
                                         text=True, timeout=10).strip()
        nm = Path(libdir).parent / 'bin/llvm-nm'
        options = ['--dynamic', '--format=posix']
        undefined = subprocess.check_output([nm, *options, '--undefined-only', cargo], text=True)
        defined = subprocess.check_output([nm, *options, '--defined-only', libc], text=True)
        imports = set(re.findall(r'(?m)^(\S+) U\b', undefined))
        exports = set(re.findall(r'(?m)^(\S+) [A-Z]\b', defined))
        self.assertEqual(imports, set(k7['elf']['strong_imports']))
        self.assertEqual(imports - exports, set())

    def test_combined_selection_stages_one_toolchain(self):
        mke2fs, = STAGE.require_e2fsprogs('mke2fs')
        k5 = read_json(RECORDS / 'k5/x86_64.json')
        k7 = read_json(RECORDS / 'k7/x86_64.json')
        k6 = read_json(RECORDS / 'k6/x86_64.json')
        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary) / 'combined.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-b', '4096', '-F', image, '16M'],
                           check=True, capture_output=True)
            stdout = io.StringIO()
            argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image),
                    '--suite', 'rustc,cargo,d1']
            with patch('sys.argv', argv), redirect_stdout(stdout):
                STAGE.main()

            shared = '/usr/libexec/breenix/rustc'
            staged = json.loads(stdout.getvalue())
            for name in ('rustc', 'cargo', 'd1'):
                self.assertIn(f'/sbin/suite-{name}', staged)
            pins = {
                f'{shared}/bin/rustc': k5['hosted_rustc_sha256'],
                f'{shared}/lib/{k5["hosted_link"]["driver"]}': k5['hosted_driver_sha256'],
                f'{shared}/bin/cargo': k7['cargo_sha256'],
                f'{shared}/lib/libc.a': k6['inputs']['lib/tools/libc.a']['sha256'],
                '/lib/libc.so': read_json(RECORDS / 'hosted-libc-x86_64.json')['sha256'],
            }
            for name, digest in k5['sysroot']['files_sha256'].items():
                if name.startswith('lib/rustlib/x86_64-breenix/'):
                    pins[f'{shared}/sysroot/{name}'] = digest
            for path, digest in pins.items():
                with self.subTest(path=path):
                    self.assertEqual([name for name, value in staged.items() if value == digest],
                                     [path])
            self.assertFalse(any('/x86_64-breenix-hosted/' in path for path in staged))
            self.assertEqual(list((image.parent / 'd1').rglob('*')), [],
                             'D1 copied the shared toolchain')

    def test_d1_stages_both_target_specs(self):
        mke2fs, = STAGE.require_e2fsprogs('mke2fs')
        pins = read_json(RECORDS / 'k7/x86_64.json')['fixture_sha256']
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            image = directory / 'root.img'
            debugfs, = STAGE.require_e2fsprogs('debugfs')
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'],
                           check=True, capture_output=True)
            argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image),
                    '--suite', 'd1']
            with patch('sys.argv', argv), redirect_stdout(io.StringIO()):
                STAGE.main()

            # The cross target is explicit, but hosted rustc also resolves its
            # own target through RUST_TARGET_PATH even when Cargo asks for -vV.
            for target in ('x86_64-breenix-hosted', 'x86_64-breenix'):
                name = f'targets/{target}.json'
                with self.subTest(target=target):
                    spec = subprocess.check_output([debugfs, '-R', f'cat {PREFIX}/{name}', image],
                                                   stderr=subprocess.DEVNULL)
                    self.assertEqual(hashlib.sha256(spec).hexdigest(), pins[name])

    def test_rustc_and_cargo_share_hosted_libc(self):
        mke2fs, debugfs = STAGE.require_e2fsprogs('mke2fs', 'debugfs')
        kd = Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'kd'))
        k5 = read_json(RECORDS / 'k5/x86_64.json')
        k7 = read_json(RECORDS / 'k7/x86_64.json')
        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary) / 'rustc-and-cargo.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-b', '4096', '-F', image, '16M'],
                           check=True, capture_output=True)
            stdout = io.StringIO()
            argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image),
                    '--suite', 'rustc,cargo']
            with patch('sys.argv', argv), redirect_stdout(stdout):
                STAGE.main()

            staged = json.loads(stdout.getvalue())
            self.assertIn('/sbin/suite-rustc', staged)
            self.assertIn('/sbin/suite-cargo', staged)
            libc = sha256(kd / 'libc.so')
            self.assertEqual(staged['/lib/libc.so'], libc)
            with subprocess.Popen([debugfs, '-R', 'cat /lib/libc.so', image],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as readback:
                self.assertEqual(hashlib.file_digest(readback.stdout, 'sha256').hexdigest(), libc)
                self.assertEqual(readback.wait(), 0)
            self.assertEqual(staged[f'{PREFIX}/bin/rustc'], k5['hosted_rustc_sha256'])
            driver = k5['hosted_link']['driver']
            self.assertEqual(staged[f'{PREFIX}/lib/{driver}'], k5['hosted_driver_sha256'])
            self.assertEqual(staged[f'{PREFIX}/bin/cargo'], k7['cargo_sha256'])

    def test_cargo_image_matches_records_and_size(self):
        mke2fs, debugfs, dumpe2fs = STAGE.require_e2fsprogs('mke2fs', 'debugfs', 'dumpe2fs')
        cache = Path(os.environ.get('BREENIX_RUSTC_PORT_BUILDS', ROOT / 'build/rustc-port'))
        sysroot = Path(os.environ.get('BREENIX_K5_SYSROOT', cache / 'sysroot'))
        k7_stage = Path(os.environ.get('BREENIX_K7_STAGE', cache / 'k7/stage1'))
        kd = Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'kd'))
        rustc = PROGRAMS / 'rustc/x86_64'
        rust_link = PROGRAMS / 'rust-link/x86_64'
        suite = PROGRAMS / 'hosted/x86_64-breenix-hosted/release/suite-cargo'
        k5 = read_json(RECORDS / 'k5/x86_64.json')
        k6 = read_json(RECORDS / 'k6/x86_64.json')
        k7 = read_json(RECORDS / 'k7/x86_64.json')
        driver = k5['hosted_link']['driver']
        linker = 'lib/tools/linker.ld'
        libc = 'lib/tools/libc.a'
        corpus = RECORDS / 'k7/corpus'
        entries = {
            '/sbin/suite-cargo': (suite, sha256(suite)),
            '/lib/ld-breenix.so': (kd / 'ld-breenix.so', read_json(kd / 'build.json')['ld-breenix.so']),
            '/lib/libc.so': (kd / 'libc.so', read_json(RECORDS / 'hosted-libc-x86_64.json')['sha256']),
            f'{PREFIX}/bin/cargo': (k7_stage / 'bin/cargo', k7['cargo_sha256']),
            f'{PREFIX}/bin/rustc': (rustc / 'rustc', k5['hosted_rustc_sha256']),
            f'{PREFIX}/lib/{driver}': (rustc / driver, k5['hosted_driver_sha256']),
            f'{PREFIX}/bin/bld': (rust_link / 'bld',
                                  read_json(rust_link / 'build.json')['files_sha256']['bld']),
            f'{PREFIX}/lib/linker.ld': (FIXTURE / linker, k6['inputs'][linker]['sha256']),
            f'{PREFIX}/lib/libc.a': (FIXTURE / libc, k6['inputs'][libc]['sha256']),
        }
        for name, digest in k5['sysroot']['files_sha256'].items():
            if name.startswith('lib/rustlib/x86_64-breenix/'):
                entries[f'{PREFIX}/sysroot/{name}'] = (sysroot / name, digest)
        for name in ('x86_64-breenix.json', 'x86_64-breenix-hosted.json'):
            target = ROOT / name
            entries[f'{PREFIX}/targets/{name}'] = (target, sha256(target))
        for source in corpus.rglob('*'):
            if source.is_file():
                entries[f'{CORPUS}/{source.relative_to(corpus)}'] = (source, sha256(source))

        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary) / 'cargo.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-b', '4096', '-F', image, '16M'],
                           check=True, capture_output=True)
            before = image.stat().st_size
            argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image), '--suite', 'cargo']
            stdout = io.StringIO()
            with patch('sys.argv', argv), redirect_stdout(stdout):
                STAGE.main()

            self.assertEqual(json.loads(stdout.getvalue() or '{}'),
                             {path: digest for path, (_, digest) in entries.items()})
            for path, (_, digest) in entries.items():
                with self.subTest(path=path), subprocess.Popen(
                    [debugfs, '-R', f'cat {path}', image], stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL,
                ) as readback:
                    self.assertEqual(hashlib.file_digest(readback.stdout, 'sha256').hexdigest(), digest)
                    self.assertEqual(readback.wait(), 0)


            summary = subprocess.check_output([dumpe2fs, '-h', image], text=True,
                                              stderr=subprocess.DEVNULL)
            superblock = dict(line.split(':', 1) for line in summary.splitlines() if ':' in line)
            block_size = int(superblock['Block size'])
            payload = sum(source.stat().st_size for source, _ in entries.values())
            blocks, partial = divmod(before + payload + HEADROOM, block_size)
            self.assertEqual(image.stat().st_size, (blocks + bool(partial)) * block_size)
            self.assertGreaterEqual(int(superblock['Free blocks']) * block_size, HEADROOM)


if __name__ == '__main__':
    unittest.main()
