"""Read back the Cargo selection on a private ext2 image."""

from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tests.rust_link_image_stage_test import FIXTURE, RECORDS, ROOT, STAGE, read_json

PREFIX = '/usr/libexec/breenix/cargo'
HEADROOM = 64 * 1024 * 1024
PROGRAMS = ROOT / 'userspace/programs/target'


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class CargoImageStageTest(unittest.TestCase):
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
                    '--suite', 'rustc,cargo,d1', '--d1-tree', str(image.parent / 'd1')]
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
            tree = directory / 'd1'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'],
                           check=True, capture_output=True)
            argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image),
                    '--suite', 'd1', '--d1-tree', str(tree)]
            with patch('sys.argv', argv), redirect_stdout(io.StringIO()):
                STAGE.main()

            # The cross target is explicit, but hosted rustc also resolves its
            # own target through RUST_TARGET_PATH even when Cargo asks for -vV.
            for target in ('x86_64-breenix-hosted', 'x86_64-breenix'):
                name = f'targets/{target}.json'
                spec = tree / 'fixture' / name
                with self.subTest(target=target):
                    self.assertTrue(spec.is_file(), f'D1 fixture omits {name}')
                    self.assertEqual(sha256(spec), pins[name])

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
            for suite in ('rustc', 'cargo'):
                prefix = f'/usr/libexec/breenix/{suite}'
                self.assertEqual(staged[f'{prefix}/bin/rustc'], k5['hosted_rustc_sha256'])
                driver = k5['hosted_link']['driver']
                self.assertEqual(staged[f'{prefix}/lib/{driver}'], k5['hosted_driver_sha256'])
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
        config = corpus / 'no-network/.cargo/config.toml'
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
            f'{PREFIX}/cargo-home/config.toml': (config, sha256(config)),
        }
        for name, digest in k5['sysroot']['files_sha256'].items():
            entries[f'{PREFIX}/sysroot/{name}'] = (sysroot / name, digest)
        for name in ('x86_64-breenix.json', 'x86_64-breenix-hosted.json'):
            target = ROOT / name
            entries[f'{PREFIX}/targets/{name}'] = (target, sha256(target))
        for source in corpus.rglob('*'):
            if source.is_file():
                entries[f'{PREFIX}/corpus/{source.relative_to(corpus)}'] = (source, sha256(source))

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
            home = subprocess.check_output([debugfs, '-R', f'ls -p {PREFIX}/cargo-home', image],
                                           text=True, stderr=subprocess.DEVNULL)
            self.assertEqual({line.split('/')[5] for line in home.splitlines() if line},
                             {'.', '..', 'config.toml'})

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
