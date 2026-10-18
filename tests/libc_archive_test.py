#!/usr/bin/env python3
"""Focused ELF fixture checks for libc archive cleanup and the symbol audit."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result

cleaner = module('cleaner', ROOT / 'scripts/libc-clean-archive.py')
audit = module('audit', ROOT / 'scripts/libc-symbol-audit.py')
TOOLS = Path(subprocess.check_output([str(ROOT / 'scripts/breenix-cc'), '--arch', 'aarch64', '--tool-dir'], text=True).strip())

class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.archive = self.dir / 'libc.a'
        self.old = audit.LIBC['aarch64']
        audit.LIBC['aarch64'] = self.archive
        # Minimal artificial symbols, no foreign implementation or constants.
        src = self.dir / 'fixture.s'
        src.write_text('''
.section .text.sqrt,"ax",@progbits
.weak sqrt
sqrt: ret
.section .text.__runtime_helper,"ax",@progbits
.global __runtime_helper
__runtime_helper: ret
.section .text._ZN17compiler_builtins4math9libm_math7fixture,"ax",@progbits
.global _ZN17compiler_builtins4math9libm_math7fixture
_ZN17compiler_builtins4math9libm_math7fixture: ret
.section .rodata.foreign,"a",@progbits
.global _ZN17compiler_builtins4math9libm_math7table
_ZN17compiler_builtins4math9libm_math7table: .byte 0
''')
        obj = self.dir / 'compiler_builtins-fixture.o'
        subprocess.run([str(TOOLS / 'clang'), '--target=aarch64-unknown-linux-gnu', '-c', str(src), '-o', str(obj)], check=True)
        subprocess.run([str(TOOLS / 'llvm-ar'), 'rcs', str(self.archive), str(obj)], check=True)

    def tearDown(self):
        audit.LIBC['aarch64'] = self.old
        self.tmp.cleanup()

    def test_audit_rejects_foreign_even_with_strong_definition(self):
        src = self.dir / 'ours.s'
        src.write_text('.text\n.global sqrt\nsqrt: ret\n')
        obj = self.dir / 'ours.o'
        subprocess.run([str(TOOLS / 'clang'), '--target=aarch64-unknown-linux-gnu', '-c', str(src), '-o', str(obj)], check=True)
        subprocess.run([str(TOOLS / 'llvm-ar'), 'r', str(self.archive), str(obj)], check=True)
        import sys
        from unittest.mock import patch
        with patch.object(sys, 'argv', ['audit', '--arch', 'aarch64']):
            self.assertEqual(audit.main(), 1)

    def test_linked_elf_rejects_weak_math(self):
        cleaner.clean(self.archive, TOOLS)
        src = self.dir / 'linked.s'
        src.write_text('.text\n.weak sqrt\nsqrt: ret\n')
        obj = self.dir / 'linked.o'
        elf = self.dir / 'linked.elf'
        subprocess.run([str(TOOLS / 'clang'), '--target=aarch64-unknown-linux-gnu', '-c', str(src), '-o', str(obj)], check=True)
        sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
        host = next(line.split(': ', 1)[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
        linker = sysroot / 'lib/rustlib' / host / 'bin/gcc-ld/ld.lld'
        subprocess.run([str(linker), '-e', 'sqrt', str(obj), '-o', str(elf)], check=True)
        import sys
        from unittest.mock import patch
        with patch.object(sys, 'argv', ['audit', '--arch', 'aarch64', '--elf', str(elf)]):
            self.assertEqual(audit.main(), 1)

    def test_cleanup_removes_code_and_data_preserves_runtime(self):
        os.utime(self.archive, ns=(1, 1))
        cleaner.clean(self.archive, TOOLS)
        self.assertGreater(self.archive.stat().st_mtime_ns, 1)
        found = audit.definitions('aarch64')
        self.assertIn('__runtime_helper', found)
        self.assertNotIn('sqrt', found)
        self.assertFalse(any(cleaner.foreign(name) for name in found))
        timestamp = self.archive.stat().st_mtime_ns
        cleaner.clean(self.archive, TOOLS)
        self.assertEqual(self.archive.stat().st_mtime_ns, timestamp)
        self.assertEqual(found, audit.definitions('aarch64'))

if __name__ == '__main__':
    unittest.main()
