#!/usr/bin/env python3
"""Exercise the production ratchet's scan failures without compiling a kernel."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ScannerTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.elf = self.root / 'kernel'
        self.elf.write_text('fixture')
        self.image = self.root / 'breenix-uefi.img'
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'])
        messages = [
            {'reason': 'compiler-artifact', 'target': {'name': 'kernel'},
             'executable': str(self.elf)},
            {'reason': 'build-script-executed', 'package_id': 'fixture#breenix@0',
             'out_dir': str(self.root)},
        ]
        self.tool('cargo', 'printf \'%s\\n\' ' + shlex.quote('\n'.join(map(json.dumps, messages))))
        self.tool('llvm-nm', 'cat "$2" >/dev/null || exit 1\nprintf \'%s\\n\' "$SYMBOLS"')

    def tool(self, name, body):
        tool = self.bin / name
        tool.write_text('#!/bin/bash\n' + body + '\n')
        tool.chmod(0o755)

    def run_scan(self, prove):
        self.env['SYMBOLS'] = 'fault_exercise' if prove else 'ordinary_symbol'
        return subprocess.run(
            ['bash', str(ROOT / 'scripts/check-xhci-fault-production-clean.sh')]
            + (['--prove'] if prove else []), env=self.env,
            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    def test_valid_scans_in_both_modes(self):
        for prove in (False, True):
            with self.subTest(prove=prove):
                self.image.write_text('xhci-fault' if prove else 'ordinary image')
                result = self.run_scan(prove)
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertIn('PASSED', result.stdout)

    def test_scan_errors_and_invalid_counts_fail_in_both_modes(self):
        real_grep = shlex.quote(shutil.which('grep'))
        for prove in (False, True):
            for failure in ('missing-image', 'unreadable-image', 'missing-elf',
                            'nm-error', 'nm-partial-error', 'grep-error',
                            'empty-count', 'text-count', 'negative-count', 'multiple-counts'):
                with self.subTest(prove=prove, failure=failure):
                    self.elf.write_text('fixture')
                    self.image.write_text('xhci-fault' if prove else 'ordinary image')
                    self.tool('llvm-nm', 'cat "$2" >/dev/null || exit 1\nprintf \'%s\\n\' "$SYMBOLS"')
                    self.tool('grep', 'exec ' + real_grep + ' "$@"')
                    if failure == 'missing-image':
                        self.image.unlink()
                    elif failure == 'missing-elf':
                        self.elf.unlink()
                    elif failure == 'unreadable-image':
                        # A deterministic EACCES equivalent, also when run as root.
                        self.tool('grep', 'if [ "$2" = -aF ]; then echo "permission denied" >&2; exit 2; fi\nexec ' + real_grep + ' "$@"')
                    elif failure.startswith('nm-'):
                        self.tool('llvm-nm', ('echo fault_exercise\n' if failure == 'nm-partial-error' else '') + 'exit 1')
                    elif failure == 'grep-error':
                        self.tool('grep', 'echo 0; exit 2')
                    else:
                        count = {'empty-count': '', 'text-count': 'bad',
                                 'negative-count': '-1', 'multiple-counts': '0\n1'}[failure]
                        self.tool('grep', "printf '%s\\n' " + shlex.quote(count))
                    result = self.run_scan(prove)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn('FAILED (scan error)', result.stdout)
                    self.assertNotIn('PASSED', result.stdout)


if __name__ == '__main__':
    unittest.main()
