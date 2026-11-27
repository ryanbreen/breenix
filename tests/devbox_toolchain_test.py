"""Mac-side checks for staging the pinned development toolchain."""

import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/devbox'))
import toolchain


TOOL_RLIB = 'lib/rustlib/x86_64-breenix/lib/libstd.rlib'
HOSTED_RLIB = 'lib/rustlib/x86_64-breenix-hosted/lib/libstd.rlib'
DRIVER = 'librustc_driver-fixture.so'


class ToolchainStageTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.out = self.root / 'publish/toolchain'
        self.records = self.root / 'records'
        self.inputs = {name: self.root / name for name in (
            'artifacts', 'sysroot', 'targets', 'fixture', 'bld', 'kd')}
        self.inputs['cargo'] = self.root / 'cargo'
        self.put(self.inputs['cargo'], b'cargo')
        compiler = {
            'fork_commit': 'a' * 40,
            'mac_rustc_sha256': '1' * 64,
            'hosted_rustc_sha256': self.put(self.inputs['artifacts'] / 'rustc', b'rustc'),
            'hosted_driver_sha256': self.put(self.inputs['artifacts'] / DRIVER, b'driver'),
            'hosted_link': {'driver': DRIVER},
            'sysroot': {'files_sha256': {
                TOOL_RLIB: self.put(self.inputs['sysroot'] / TOOL_RLIB, b'tool std'),
                HOSTED_RLIB: self.put(self.inputs['sysroot'] / HOSTED_RLIB, b'hosted std'),
            }},
        }
        self.record(self.records / 'k5/x86_64.json', compiler)
        link_inputs = {f'lib/tools/{name}': {
            'sha256': self.put(self.inputs['fixture'] / 'lib/tools' / name, name.encode()),
        } for name in ('linker.ld', 'libc.a')}
        self.record(self.records / 'k6/x86_64.json', {
            'rustc_sha256': compiler['mac_rustc_sha256'],
            'rustc_commit': compiler['fork_commit'],
            'inputs': link_inputs,
        })
        target_pins = {f'targets/{name}.json': self.put(
            self.inputs['targets'] / f'{name}.json', name.encode(),
        ) for name in ('x86_64-breenix', 'x86_64-breenix-hosted')}
        self.record(self.records / 'k7/x86_64.json', {
            'fork_commit': compiler['fork_commit'],
            'k5_compiler_digests': {key: compiler[key] for key in (
                'mac_rustc_sha256', 'hosted_rustc_sha256', 'hosted_driver_sha256')},
            'fixture_sha256': target_pins,
            'cargo_sha256': hashlib.sha256(b'cargo').hexdigest(),
        })
        self.record(self.inputs['bld'] / 'build.json', {'files_sha256': {
            'bld': self.put(self.inputs['bld'] / 'bld', b'bld'),
        }})
        self.runtime = {name: self.put(self.inputs['kd'] / name, name.encode())
                        for name in ('ld-breenix.so', 'libc.so')}
        self.record(self.inputs['kd'] / 'build.json', self.runtime)
        self.record(self.records / 'hosted-libc-x86_64.json', {'sha256': self.runtime['libc.so']})

    @staticmethod
    def put(path, data):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return hashlib.sha256(data).hexdigest()

    def record(self, path, record):
        self.put(path, json.dumps(record).encode())

    def stage(self):
        # The ELF dependency audit takes real x86-64 ELF files and the host LLVM
        # reader. Keep the real KD receipt and digest checks for these fixtures.
        with patch.object(toolchain, 'verify_hosted_elf'):
            return toolchain.stage(self.out, self.records, **self.inputs)

    def test_digest_refusal_leaves_no_candidate(self):
        for path in (self.inputs['artifacts'] / DRIVER, self.inputs['sysroot'] / TOOL_RLIB,
                     self.inputs['cargo'], self.inputs['targets'] / 'x86_64-breenix.json',
                     self.inputs['fixture'] / 'lib/tools/libc.a', self.inputs['bld'] / 'bld',
                     self.inputs['kd'] / 'ld-breenix.so', self.inputs['kd'] / 'libc.so'):
            with self.subTest(path=path.name):
                original = path.read_bytes()
                path.write_bytes(b'tampered')
                with self.assertRaisesRegex(ValueError, 'SHA-256'):
                    self.stage()
                self.assertFalse(list(self.out.glob('x86_64-*')))
                self.assertFalse((self.out.parent / 'runtime/x86_64').exists())
                path.write_bytes(original)

    def test_layout_and_manifest(self):
        toolchain_id = self.stage()
        version = self.out / toolchain_id
        manifest = json.loads((version / 'MANIFEST.json').read_text())
        expected = {
            'bin/rustc', 'bin/cargo', 'bin/bld', f'lib/{DRIVER}',
            'lib/libc.a', 'lib/linker.ld', f'sysroot/{TOOL_RLIB}',
            'targets/x86_64-breenix.json', 'targets/x86_64-breenix-hosted.json',
        }
        self.assertRegex(toolchain_id, r'^x86_64-aaaaaaaa-[0-9a-f]{8}$')
        self.assertEqual(manifest['id'], toolchain_id)
        self.assertEqual(manifest['fork_commit'], 'a' * 40)
        self.assertEqual(set(manifest['files']), expected)
        self.assertEqual({str(p.relative_to(version)) for p in version.rglob('*') if p.is_file()},
                         expected | {'MANIFEST.json'})
        for name, facts in manifest['files'].items():
            path = version / name
            mode = 0o755 if name.startswith('bin/') or name.endswith('.so') else 0o444
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), mode)
            self.assertEqual(facts, {
                'size': path.stat().st_size, 'mode': f'{mode:04o}',
                'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            })
        self.assertEqual(set(manifest['runtime']), {f'/lib/{name}' for name in self.runtime})
        for name, digest in self.runtime.items():
            path = self.out.parent / 'runtime/x86_64' / name
            self.assertEqual(manifest['runtime'][f'/lib/{name}'], {
                'size': path.stat().st_size, 'mode': '0755', 'sha256': digest,
            })
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o755)

    def test_current_is_an_explicit_single_line_activation(self):
        existing = self.out / 'CURRENT'
        self.put(existing, b'x86_64-12345678-abcdef01\n')
        toolchain_id = self.stage()
        self.assertEqual(existing.read_bytes(), b'x86_64-12345678-abcdef01\n')

        def current(candidate):
            return subprocess.run(
                [sys.executable, toolchain.__file__, 'current', '--out', self.out, candidate],
                capture_output=True, check=False, timeout=10,
            )

        valid = current(toolchain_id)
        self.assertEqual(valid.returncode, 0, valid.stderr)
        self.assertEqual(valid.stdout, f'{toolchain_id}\n'.encode())
        for candidate in ('../' + toolchain_id, toolchain_id + '\n', 'x86_64-12345678-abcdef01'):
            with self.subTest(candidate=candidate):
                invalid = current(candidate)
                self.assertNotEqual(invalid.returncode, 0)
                self.assertEqual(invalid.stdout, b'')
        self.assertEqual(existing.read_bytes(), b'x86_64-12345678-abcdef01\n')


if __name__ == '__main__':
    unittest.main()
