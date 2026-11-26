#!/usr/bin/env python3
"""K5 image staging must preserve the recorded bytes through ext2."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
STAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGE)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class RustcImageStageTest(unittest.TestCase):
    def test_rustc_stage_digests_match_build_record(self):
        mke2fs, debugfs = STAGE.require_e2fsprogs('mke2fs', 'debugfs')
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'],
                           check=True, capture_output=True)
            artifacts = root / 'artifacts'
            sysroot = root / 'sysroot'
            targets = root / 'targets'
            corpus = root / 'k5/corpus'
            kd = root / 'kd'
            for directory in (artifacts, targets, corpus, kd):
                directory.mkdir(parents=True)
            def write(path, data):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                return path
            suite = write(root / 'suite-rustc', b'\x7fELFsuite')
            compiler = write(artifacts / 'rustc', b'\x7fELFcompiler')
            driver = write(artifacts / 'librustc_driver-abcd.so', b'\x7fELF')
            # 264 MB driver plus sysroot bytes must fit, with ext2 metadata headroom.
            with driver.open('ab') as stream:
                stream.truncate(264_000_000)
            loader = write(kd / 'ld-breenix.so', b'\x7fELFloader')
            libc = write(kd / 'libc.so', b'\x7fELFlibc')
            files = {}
            for target in ('x86_64-breenix', 'x86_64-breenix-hosted'):
                rel = f'lib/rustlib/{target}/lib/libstd-example.rlib'
                files[rel] = digest(write(sysroot / rel, target.encode()))
                write(targets / f'{target}.json', target.encode())
            for name in ('expect.json', 'hello.rs', 'type-error.rs', 'uses-std.rs'):
                write(corpus / name, name.encode())
            k5 = {'hosted_rustc_sha256': digest(compiler), 'hosted_driver_sha256': digest(driver),
                  'hosted_link': {'driver': driver.name, 'libc_sha256': digest(libc)},
                  'sysroot': {'files_sha256': files}}
            kd_record = {'ld-breenix.so': digest(loader), 'libc.so': digest(libc)}
            before = image.stat().st_size
            with patch.object(STAGE, 'ROOT', targets):
                record = STAGE.stage_hosted_toolchain(
                    image, ['rustc'], root, artifacts, None, sysroot, None,
                    kd, None, root, k5, kd_record, {},
                )
            prefix = '/usr/libexec/breenix/rustc'
            expected = {'/sbin/suite-rustc': digest(suite), f'{prefix}/bin/rustc': digest(compiler),
                        f'{prefix}/lib/{driver.name}': digest(driver), '/lib/ld-breenix.so': digest(loader),
                        '/lib/libc.so': digest(libc)}
            expected.update({f'{prefix}/sysroot/{p}': h for p, h in files.items()
                             if p.startswith('lib/rustlib/x86_64-breenix/')})
            expected.update({f'{prefix}/targets/{p.name}': digest(p) for p in targets.iterdir()})
            expected.update({f'{prefix}/corpus/{p.name}': digest(p) for p in corpus.iterdir()})
            self.assertEqual(record, expected)
            staged_bytes = sum(p.stat().st_size for p in [suite, compiler, driver, loader, libc]
                               + list((sysroot / 'lib/rustlib/x86_64-breenix').rglob('*.rlib')) + list(targets.iterdir()) + list(corpus.iterdir()))
            self.assertGreaterEqual(image.stat().st_size - before, staged_bytes + 64 * 1024 * 1024)
            for path, expected_digest in expected.items():
                with self.subTest(path=path), subprocess.Popen(
                    [debugfs, '-R', f'cat {path}', image], stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL) as process:
                    actual = hashlib.file_digest(process.stdout, 'sha256').hexdigest()
                    self.assertEqual(process.wait(), 0)
                    self.assertEqual(actual, expected_digest)


if __name__ == '__main__':
    unittest.main()
