"""Checks for the pinned LLVM cross-configure."""
import json
import os
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))


class LlvmPortTest(unittest.TestCase):
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
