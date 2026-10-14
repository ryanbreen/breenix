#!/usr/bin/env python3
"""Fixture checks for scripts/libc-duplicate-check.py."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('check', ROOT / 'scripts/libc-duplicate-check.py')
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)
TOOLS = Path(subprocess.check_output([str(ROOT / 'scripts/breenix-cc'), '--arch', 'aarch64', '--tool-dir'], text=True).strip())
NM = TOOLS / 'llvm-nm'


class DuplicateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def archive(self, name, members):
        """An archive of one object per {member: assembly}."""
        path = self.dir / name
        objects = []
        for member, text in members.items():
            src = self.dir / f'{member}.s'
            src.write_text(text)
            obj = self.dir / f'{member}.o'
            subprocess.run([str(TOOLS / 'clang'), '--target=aarch64-unknown-linux-gnu', '-c', str(src), '-o', str(obj)], check=True)
            objects.append(str(obj))
        subprocess.run([str(TOOLS / 'llvm-ar'), 'rcs', str(path), *objects], check=True)
        return path

    @staticmethod
    def define(name, binding='.global'):
        return f'.section .text.{name},"ax",@progbits\n{binding} {name}\n{name}: ret\n'

    def test_two_strong_members_are_reported(self):
        libc = self.archive('libc.a', {'stub': self.define('setenv'), 'env': self.define('setenv')})
        found = check.duplicates(NM, libc, [])
        self.assertEqual([name for name, _ in found], ['setenv'])
        self.assertEqual(len(found[0][1]), 2)

    def test_weak_definitions_are_left_to_the_linker(self):
        libc = self.archive('libc.a', {'builtins': self.define('memcpy', '.weak'), 'string': self.define('memcpy')})
        self.assertEqual(check.duplicates(NM, libc, []), [])

    def test_a_c_symbol_in_another_link_input_is_reported(self):
        libc = self.archive('libc.a', {'env': self.define('getenv')})
        other = self.archive('libstd.rlib', {'std': self.define('getenv')})
        self.assertEqual([name for name, _ in check.duplicates(NM, libc, [other])], ['getenv'])

    def test_mangled_symbols_shared_with_the_std_rlibs_are_expected(self):
        symbol = '_ZN4core3fmt5write17h0123456789abcdefE'
        libc = self.archive('libc.a', {'core': self.define(symbol)})
        other = self.archive('libcore.rlib', {'core': self.define(symbol)})
        self.assertEqual(check.duplicates(NM, libc, [other]), [])


if __name__ == '__main__':
    unittest.main()
