#!/usr/bin/env python3
"""Test the C driver's final-link audit contract without a guest or real link."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DriverTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='breenix-cc-host-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scripts = self.root / 'scripts'
        scripts.mkdir()
        shutil.copy(ROOT / 'scripts/breenix-cc', scripts / 'breenix-cc')
        (self.root / 'resource/include').mkdir(parents=True)
        archive = self.root / 'libs/libbreenix-libc/target/aarch64-breenix/release/libc.a'
        archive.parent.mkdir(parents=True)
        archive.touch()
        compiler = scripts / 'clang'
        compiler.write_text('''#!/usr/bin/env python3
import sys
from pathlib import Path
root = Path(__file__).resolve().parents[1]
args = sys.argv[1:]
if '-print-resource-dir' in args:
    print(root / 'resource')
    sys.exit(0)
assert '--no-default-config' in args
assert Path(next(a.split('=', 1)[1] for a in args if a.startswith('--sysroot='))).resolve() == (root / 'libs/libbreenix-libc').resolve()
assert '-D__breenix__=1' in args
assert '-U__linux__' in args and '-U__gnu_linux__' in args
if any(a in args for a in ('-c', '-S', '-E', '-M', '-MM', '-fsyntax-only')):
    sys.exit(0)
output = 'a.out'
for i, arg in enumerate(args):
    if arg == '-o': output = args[i + 1]
    elif arg.startswith('-o') and len(arg) > 2: output = arg[2:]
Path(output).write_bytes(b'synthetic output')
''')
        compiler.chmod(0o755)
        (scripts / 'libc-symbol-audit.py').write_text('''import json, sys
from pathlib import Path
root = Path(__file__).resolve().parents[1]
args = sys.argv[1:]
output = Path(args[args.index('--elf') + 1])
assert output.is_file()
(root / 'audit.json').write_text(json.dumps(args))
sys.exit(7 if output.name == 'tainted.elf' else 0)
''')
        self.env = dict(os.environ, BREENIX_CC_CLANG=str(compiler), CARGO_BUILD_JOBS='4')
        # Use the checkout's toolchain so the temporary tree does not select a
        # different default installation without the linker component.
        self.env['RUSTUP_TOOLCHAIN'] = subprocess.check_output(
            ['rustup', 'show', 'active-toolchain'], cwd=ROOT, text=True).split()[0]

    def run_driver(self, *args):
        return subprocess.run([str(self.root / 'scripts/breenix-cc'), '--arch', 'aarch64', *args],
                              cwd=self.root, env=self.env, text=True, capture_output=True)

    def test_output_spellings_are_audited(self):
        for args, output in [((), 'a.out'), (('-o', 'separate.elf'), 'separate.elf'),
                             (('-ojoined.elf',), 'joined.elf')]:
            result = self.run_driver('input.o', *args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads((self.root / 'audit.json').read_text()),
                             ['--arch', 'aarch64', '--elf', output])

    def test_audit_failure_is_a_link_failure(self):
        result = self.run_driver('input.o', '-o', 'tainted.elf')
        self.assertEqual(result.returncode, 7)
        self.assertTrue((self.root / 'audit.json').is_file())

    def test_compilation_and_relocatable_links_are_not_final_outputs(self):
        for args in [('-fsyntax-only', 'input.c'), ('-r', 'input.o', '-o', 'combined.o')]:
            result = self.run_driver(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((self.root / 'audit.json').exists())


if __name__ == '__main__':
    unittest.main()
