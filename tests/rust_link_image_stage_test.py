"""Check hosted linker fixture staging against the build records on an ext2 image."""

from contextlib import redirect_stdout
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path(os.environ.get('BREENIX_K6_FIXTURE', ROOT / 'build/rustc-port/k6/fixture'))
RECORDS = ROOT / 'toolchain/rust-host'
PREFIX = '/usr/libexec/breenix'
SPEC = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
STAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGE)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write(root, name, data):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return digest(data)


def read_json(path):
    return json.loads(path.read_text())


class RustLinkImageStageTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.mke2fs, self.debugfs = STAGE.require_e2fsprogs('mke2fs', 'debugfs')

    def image(self, name):
        path = self.root / name
        subprocess.run([self.mke2fs, '-q', '-t', 'ext2', '-F', path, '16M'],
                       check=True, capture_output=True)
        return path

    def stage(self, root, image, selection, **environment):
        stdout = io.StringIO()
        argv = ['rust_host_stage.py', '--arch', 'x86_64', '--image', str(image), '--suite', selection]
        with (patch.object(STAGE, 'ROOT', root),
              patch.object(STAGE, 'PROGRAMS', root / 'userspace/programs'),
              patch.dict(os.environ, environment, clear=True),
              patch('sys.argv', argv), redirect_stdout(stdout)):
            STAGE.main()
        return json.loads(stdout.getvalue() or '{}')

    def readback(self, image, path):
        with subprocess.Popen([self.debugfs, '-R', f'cat {path}', image],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as process:
            result = hashlib.file_digest(process.stdout, 'sha256').hexdigest()
            self.assertEqual(process.wait(), 0)
            return result

    def test_stage_rust_link_matches_records(self):
        image = self.image('rust-link.img')
        artifacts = self.root / 'userspace/programs/target/rust-link/x86_64'
        files = {name: write(artifacts, name, b'\x7fELF' + name.encode())
                 for name in ('bld', 'suite-rust-link')}
        write(artifacts, 'build.json', json.dumps({'files_sha256': files}).encode())
        for name in ('k5/x86_64.json', 'k6/x86_64.json', 'k6/expect.json'):
            write(self.root / 'toolchain/rust-host', name, (RECORDS / name).read_bytes())
        k5 = read_json(RECORDS / 'k5/x86_64.json')
        k6 = read_json(RECORDS / 'k6/x86_64.json')
        expect = read_json(RECORDS / 'k6/expect.json')
        metadata = expect['metadata']['archive']
        write(self.root / 'toolchain/rust-host/k6', metadata,
              (RECORDS / 'k6' / metadata).read_bytes())
        prefix = f'{PREFIX}/rust-link'
        expected = {'/sbin/suite-rust-link': files['suite-rust-link'], f'{prefix}/bin/bld': files['bld']}
        expected.update({f'{prefix}/{name}': row['sha256'] for name, row in k6['inputs'].items()})
        expected.update({f'{prefix}/sysroot/{name}': sha
                         for name, sha in k5['sysroot']['files_sha256'].items()})
        for name, link in k6['links'].items():
            argv = FIXTURE / 'links' / name / 'argv.json'
            self.assertEqual(read_json(argv), link['argv'])
            expected[f'{prefix}/links/{name}/argv.json'] = digest(argv.read_bytes())
            expected[f'{prefix}/links/{name}/reference.elf'] = link['reference_sha256']
        expected[f"{prefix}/{expect['control']['archive']}"] = expect['control']['sha256']
        expected[f'{prefix}/{metadata}'] = expect['metadata']['sha256']
        for name in ('x86_64.json', 'expect.json'):
            expected[f'{prefix}/{name}'] = digest((RECORDS / 'k6' / name).read_bytes())

        before = image.stat().st_size
        staged = self.stage(self.root, image, 'rust-link', BREENIX_K6_FIXTURE=str(FIXTURE))
        self.assertEqual(staged, expected)
        for path, sha in expected.items():
            with self.subTest(path=path):
                self.assertEqual(self.readback(image, path), sha)
        payload = sum((FIXTURE / path.removeprefix(prefix + '/')).stat().st_size
                      for path in expected if path.startswith(prefix + '/')
                      and path not in (f'{prefix}/bin/bld', f'{prefix}/expect.json',
                                       f'{prefix}/{metadata}'))
        payload += (RECORDS / 'k6' / metadata).stat().st_size
        payload += sum((artifacts / name).stat().st_size for name in files)
        payload += (RECORDS / 'k6/expect.json').stat().st_size
        dumpe2fs, = STAGE.require_e2fsprogs('dumpe2fs')
        summary = subprocess.check_output([dumpe2fs, '-h', image], text=True,
                                          stderr=subprocess.DEVNULL)
        superblock = dict(line.split(':', 1) for line in summary.splitlines() if ':' in line)
        block_size = int(superblock['Block size'])
        headroom = 16 * 1024 * 1024
        requested = before + 2 * payload + headroom
        blocks, partial = divmod(requested, block_size)
        self.assertEqual(image.stat().st_size, (blocks + bool(partial)) * block_size)
        self.assertGreaterEqual(int(superblock['Free blocks']) * block_size, headroom)

        # Give staging the wrong unwinder without changing the retained capture.
        altered = self.root / 'altered'
        for item in FIXTURE.iterdir():
            if item.name != 'lib':
                (altered / item.name).parent.mkdir(parents=True, exist_ok=True)
                (altered / item.name).symlink_to(item.resolve())
        hosted = altered / 'lib/hosted'
        hosted.mkdir(parents=True)
        (altered / 'lib/tools').symlink_to((FIXTURE / 'lib/tools').resolve())
        for name in ('libc.a', 'linker-hosted.ld'):
            (hosted / name).symlink_to((FIXTURE / 'lib/hosted' / name).resolve())
        write(hosted, 'libbunwind.a', b'wrong hosted unwinder')
        another = self.image('bad-unwinder.img')
        before = another.stat().st_size
        with self.assertRaisesRegex(ValueError, r'libbunwind\.a.*build record'):
            self.stage(self.root, another, 'rust-link', BREENIX_K6_FIXTURE=str(altered))
        self.assertEqual(another.stat().st_size, before)

    def test_other_selections_stage_unchanged(self):
        # These are the file sets staged by f06b3116, including rust-host's
        # mmap fixture, which the staging CLI does not include in its stdout.
        root = self.root
        programs = root / 'userspace/programs'
        release = programs / 'target/hosted/x86_64-breenix-hosted/release'
        llvm = programs / 'target/llvm/x86_64'
        kd = programs / 'target/kd'
        rustc = programs / 'target/rustc/x86_64'
        sysroot = root / 'build/rustc-port/sysroot'
        k5 = root / 'toolchain/rust-host/k5'
        k4 = root / 'toolchain/rust-host/k4/corpus'
        expected = {name: {} for name in ('rustc', 'llvm', 'dynamic', 'rust-host')}

        def fixture(selection, base, source, guest, data):
            sha = write(base, source, data)
            expected[selection][guest] = sha
            return sha

        for selection in expected:
            name = f'suite-{selection}'
            fixture(selection, kd if selection == 'dynamic' else release, name, f'/sbin/{name}', b'\x7fELF' + name.encode())
        for name in ('llc', 'llvm-mc', 'bld'):
            fixture('llvm', llvm, name, f'{PREFIX}/llvm/bin/{name}', b'\x7fELF' + name.encode())
        fixture('llvm', llvm, 'libc.a', f'{PREFIX}/llvm/lib/libc.a', b'archive')
        fixture('llvm', programs, 'linker.ld', f'{PREFIX}/llvm/linker.ld', b'linker script')
        fixture('llvm', k4, 'probe.ll', f'{PREFIX}/llvm/corpus/probe.ll', b'probe')
        fixture('llvm', k4, 'expect.json', f'{PREFIX}/llvm/corpus/expect.json',
                b'{"objects":[{"input":"probe.ll"}]}')
        for name in ('dynamic-probe', 'missing-interp', 'libkd-probe.so', 'dynamic-panic-probe', 'libkd-panic.so'):
            fixture('dynamic', kd, name, f'{PREFIX}/dynamic/{name}', b'\x7fELF' + name.encode())
        shared = {name: fixture('dynamic', kd, name, f'/lib/{name}', b'\x7fELF' + name.encode())
                  for name in ('ld-breenix.so', 'libc.so')}
        write(kd, 'build.json', json.dumps(shared).encode())
        expected['rustc'].update({f'/lib/{name}': sha for name, sha in shared.items()})
        compiler = fixture('rustc', rustc, 'rustc', f'{PREFIX}/rustc/bin/rustc', b'\x7fELFrustc')
        driver = fixture('rustc', rustc, 'driver.so', f'{PREFIX}/rustc/lib/driver.so', b'\x7fELFdriver')
        archives = {}
        for target in ('x86_64-breenix', 'x86_64-breenix-hosted'):
            rel = f'lib/rustlib/{target}/lib/libstd-probe.rlib'
            archives[rel] = fixture('rustc', sysroot, rel, f'{PREFIX}/rustc/sysroot/{rel}', target.encode())
            fixture('rustc', root, f'{target}.json', f'{PREFIX}/rustc/targets/{target}.json', b'{}')
        fixture('rustc', k5, 'corpus/hello.rs', f'{PREFIX}/rustc/corpus/hello.rs', b'fn main() {}')
        write(k5, 'x86_64.json', json.dumps({
            'hosted_rustc_sha256': compiler, 'hosted_driver_sha256': driver,
            'hosted_link': {'driver': 'driver.so', 'libc_sha256': shared['libc.so']},
            'sysroot': {'files_sha256': archives},
        }).encode())
        for name in ('startup-probe', 'process-probe', 'unwind-probe'):
            fixture('rust-host', release, f'rust-host-{name}', f'{PREFIX}/rust-host/{name}', b'\x7fELF' + name.encode())
        expected['rust-host'][f'{PREFIX}/rust-host/mmap-fixture'] = (
            '641a09f91a4a48c8fad03a45411ab8e714f1612914175abae70e74d09b92427f'
        )

        for selection, files in expected.items():
            with self.subTest(selection=selection):
                image = self.image(f'{selection}.img')
                staged = self.stage(root, image, selection)
                self.assertEqual(staged, {} if selection == 'rust-host' else files)
                for path, sha in files.items():
                    self.assertEqual(self.readback(image, path), sha)
                # Read the complete staged file set, rather than assuming the
                # receipt lists everything the selector wrote.
                dump = root / f'{selection}-readback'
                dump.mkdir()
                subprocess.run([self.debugfs, '-R', f'rdump / {dump}', image],
                               check=True, capture_output=True)
                self.assertEqual({'/' + str(p.relative_to(dump)) for p in dump.rglob('*') if p.is_file()},
                                 set(files))


if __name__ == '__main__':
    unittest.main()
