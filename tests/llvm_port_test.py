"""Checks for the pinned LLVM cross build."""
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))


class LlvmPortTest(unittest.TestCase):
    def test_support_patch_is_breenix_only(self):
        patch = ROOT / 'toolchain/rust-host/k4/patches/llvm-support-breenix.patch'
        allowed = re.compile(r'llvm/lib/(?:Support/Unix/[^/]+|TargetParser/Host\.cpp)')
        hunks = re.split(r'(?m)^@@ .* @@[^\n]*\n|^--- ', patch.read_text())
        current = None
        count = 0
        for block in hunks[1:]:
            if block.startswith('a/'):
                before, after, *_ = block.splitlines()
                current = before[2:]
                self.assertIsNotNone(allowed.fullmatch(current), current)
                self.assertEqual(after, '+++ b/' + current)
                continue
            self.assertIsNotNone(current)
            inside = False
            found_branch = False
            count += 1
            for line in block.splitlines():
                if line.startswith('diff --git '):
                    break
                self.assertFalse(line.startswith('-'), f'{current}: {line}')
                if line in ('+#if defined(__breenix__)', '+#elif defined(__breenix__)'):
                    self.assertFalse(inside, f'{current}: {line}')
                    inside = True
                    found_branch = True
                elif re.match(r'^[ +]#(?:elif|else|endif)\b', line):
                    if line.startswith('+'):
                        self.assertTrue(inside, f'{current}: {line}')
                    inside = False
                elif line.startswith('+'):
                    self.assertTrue(inside, f'{current}: {line}')
                    self.assertFalse(line.startswith('+#'), f'{current}: {line}')
            self.assertTrue(found_branch, current)
        self.assertGreater(count, 0)

    def test_support_patch_leaves_pinned_llvm_clean(self):
        import llvm_port

        llvm = llvm_port.default_llvm().resolve()
        pin = llvm_port.verify_llvm(ROOT, llvm)
        with tempfile.TemporaryDirectory(prefix='breenix-llvm-support-') as temp:
            source = llvm_port.prepare_support_source(llvm, Path(temp))
            self.assertFalse((source / 'llvm').is_symlink())
            subprocess.run(['git', 'apply', '--no-index', '--reverse', '--check',
                            llvm_port.SUPPORT_PATCH], cwd=source, check=True)
        self.assertEqual(llvm_port.verify_llvm(ROOT, llvm), pin)

    def test_llvm_libraries_build_both_arches(self):
        import llvm_port

        llvm = llvm_port.default_llvm().resolve()
        pin = llvm_port.verify_llvm(ROOT, llvm)
        runs = Path(os.environ.get('BREENIX_LLVM_PORT_BUILDS', ROOT / 'build/llvm-port'))
        for arch, machine in (('x86_64', 62), ('aarch64', 183)):
            with self.subTest(arch=arch):
                run = runs / arch
                record = json.loads((run / 'record.json').read_text())
                self.assertTrue(record.get('archives'), 'build record has no archive digests')
                checked_in = json.loads((ROOT / f'toolchain/rust-host/k4/{arch}.json').read_text())
                self.assertEqual(record['arch'], arch)
                self.assertEqual(record['llvm_project'], pin)
                self.assertEqual(record['build']['exit_status'], 0, record['build']['first_error'])
                self.assertIsNone(record['build']['first_error'])
                self.assertEqual(record['pinned_llvm_status'], {'before': '', 'after': ''})
                self.assertEqual(record['support_patch']['text'], llvm_port.SUPPORT_PATCH.read_text())
                self.assertEqual(record['support_patch']['sha256'], llvm_port.digest(llvm_port.SUPPORT_PATCH))
                self.assertEqual(record['support_patch'], checked_in['support_patch'])
                self.assertEqual(record['archives'], checked_in['archives'])
                self.assertEqual(record['objects'], checked_in['objects'])

                build = run / 'build'
                ninja = (build / 'build.ninja').read_text()
                archives = set(re.findall(r'^build (lib/libLLVM\S+\.a): \S*STATIC_LIBRARY_LINKER', ninja, re.M))
                objects = set(re.findall(r'^build (tools/(?:llc|llvm-mc)/\S+\.o): \S*COMPILER', ninja, re.M))
                self.assertIn('lib/libLLVMCore.a', archives)
                self.assertTrue(any(path.startswith('tools/llc/') for path in objects))
                self.assertTrue(any(path.startswith('tools/llvm-mc/') for path in objects))
                self.assertEqual(record['archives'].keys(), archives)
                self.assertEqual(record['objects'].keys(), objects)
                for name, sha256 in record['archives'].items():
                    archive = build / name
                    with archive.open('rb') as stream:
                        self.assertEqual(stream.read(8), b'!<arch>\n', name)
                    self.assertEqual(llvm_port.digest(archive), sha256, name)
                for name, sha256 in record['objects'].items():
                    obj = build / name
                    with obj.open('rb') as stream:
                        header = stream.read(20)
                    self.assertEqual(header[:6], b'\x7fELF\x02\x01', name)
                    self.assertEqual(struct.unpack_from('<HH', header, 16), (1, machine), name)
                    self.assertEqual(llvm_port.digest(obj), sha256, name)
        self.assertEqual(llvm_port.verify_llvm(ROOT, llvm), pin)

    def test_llvm_diagnostics_match_record(self):
        import llvm_port
        from cxx_runtime import FIRST_ERROR, diagnostics_from_log

        llvm = llvm_port.default_llvm().resolve()
        runs = Path(os.environ.get('BREENIX_LLVM_PORT_BUILDS', ROOT / 'build/llvm-port'))
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                run = (runs / arch).resolve()
                expected = json.loads((ROOT / f'toolchain/rust-host/k4/{arch}.json').read_text())
                record = json.loads((run / 'record.json').read_text())
                self.assertTrue(expected.get('archives'), 'build record has no archive digests')
                self.assertEqual(record['clang++'], expected['clang++'])
                log = run / 'build.log'
                self.assertEqual(llvm_port.digest(log), record['build']['log_sha256'])
                warnings = diagnostics_from_log(log, run, llvm)
                for warning in warnings:
                    self.assertNotIn('<repo>/libs/libbreenix-libc/', warning.splitlines()[0])
                self.assertEqual(warnings, expected['build']['allowlist'])
                self.assertEqual(warnings, record['build']['allowlist'])
                self.assertEqual(warnings, record['build']['diagnostics'])
                self.assertEqual(warnings, record['build']['warnings'])
                self.assertEqual([line for line in log.read_text().splitlines()
                                  if FIRST_ERROR.search(line)], [])
                self.assertIsNone(record['build']['first_error'])

    def test_configure_follows_ruling_90(self):
        import llvm_port

        llvm = llvm_port.default_llvm()
        pin = llvm_port.verify_llvm(ROOT, llvm)
        runs = Path(os.environ.get('BREENIX_LLVM_PORT_BUILDS', ROOT / 'build/llvm-port'))
        targets = llvm_port.bootstrap_targets(ROOT)
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                run = runs / arch
                record = json.loads((run / 'record.json').read_text())
                self.assertEqual(record['llvm_project'], pin)
                self.assertEqual(record['targets'], targets)
                self.assertEqual(record['clang++']['path'], '/usr/bin/clang++')
                self.assertTrue(record['clang++']['version'])
                for stage in ('host_configure', 'host_build', 'configure'):
                    self.assertEqual(record[stage]['exit_status'], 0, record[stage]['first_error'])
                    self.assertIsNone(record[stage]['first_error'])
                    self.assertEqual(llvm_port.digest(run / (stage + '.log')),
                                     record[stage]['log_sha256'])
                cmd = record['configure']['command']
                for option in ('-DCMAKE_SYSTEM_NAME=Breenix',
                               '-DCMAKE_MODULE_PATH=<repo>/toolchain/cmake',
                               '-DLLVM_ENABLE_THREADS=ON', '-DBUILD_SHARED_LIBS=OFF',
                               '-DLLVM_BUILD_LLVM_DYLIB=OFF', '-DLLVM_LINK_LLVM_DYLIB=OFF',
                               '-DLLVM_ENABLE_ZLIB=OFF', '-DLLVM_ENABLE_ZSTD=OFF',
                               '-DLLVM_ENABLE_LIBXML2=OFF', '-DLLVM_ENABLE_TERMINFO=OFF',
                               '-DLLVM_TARGETS_TO_BUILD=' + targets):
                    self.assertIn(option, cmd)
                self.assertTrue(any(x.startswith('-DLLVM_TABLEGEN=') for x in cmd))
                self.assertTrue(any(x.startswith('-DLLVM_NATIVE_TOOL_DIR=') for x in cmd))
                self.assertTrue(any(llvm_port.POSIX_FLAG in x for x in cmd))
                self.assertFalse(any(re.search(r'(^|[ =])-W(?:error|no-)|(^|[ =])-w(?:\s|$)', x)
                                     for x in cmd))
                self.assertFalse(any('libunwind' in x.lower() for x in cmd))
                self.assertFalse(any(x.startswith(('-DLLVM_ENABLE_EH=', '-DLLVM_ENABLE_RTTI='))
                                     for x in cmd))
                ninja = (run / 'build/build.ninja').read_text()
                self.assertRegex(ninja, r'(?m)^build llc:')
                self.assertRegex(ninja, r'(?m)^build llvm-mc:')
        self.assertEqual(llvm_port.verify_llvm(ROOT, llvm), pin)


if __name__ == '__main__':
    unittest.main()
