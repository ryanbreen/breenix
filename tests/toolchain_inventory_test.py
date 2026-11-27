import importlib.util
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('inventory', ROOT / 'scripts/toolchain-busybox-c-inventory.py')
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class Inventory(unittest.TestCase):
    def test_busybox_config_pins(self):
        digest = inventory.digest(ROOT / 'vendor/busybox/breenix-libc.config')
        for name in ('busybox-c-inventory.json', 'kbuild-make-inventory.json'):
            with self.subTest(inventory=name):
                report = json.loads((ROOT / 'toolchain/docs' / name).read_text())
                self.assertEqual(report['inputs']['config'], digest)

    def test_classifies_diagnostic_messages(self):
        fixtures = ROOT / 'tests/toolchain_inventory_fixtures'
        for name, message in {
            'statement expression': 'statement expression is unsupported',
            'typeof': "unexpected token '__typeof__'",
            'attribute': "unexpected token '__attribute__'",
            'builtin': "unknown builtin '__builtin_foo'",
            'GNU preprocessor': "unsupported directive '#include_next'",
            'inline asm': "unexpected token '__asm__'",
            'unknown option': "unknown option '-funknown'",
            'other': 'unexpected token mystery',
        }.items():
            with self.subTest(name=name):
                fixture = fixtures / (name.replace(' ', '_') + '.c')
                self.assertTrue(fixture.is_file())
                diagnostic = f'{fixture}:1:1: error: {message}'
                self.assertEqual(inventory.classify_diagnostic(diagnostic), name)

    def test_classifies_fixture_diagnostics(self):
        fixtures = ROOT / 'tests/toolchain_inventory_fixtures'
        bcc = os.environ.get('BCC', str(ROOT / 'toolchain/target/debug/bcc'))
        for name in (
            'statement expression', 'typeof', 'attribute', 'builtin',
            'GNU preprocessor', 'inline asm', 'unknown option', 'other',
        ):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as work:
                fixture = fixtures / (name.replace(' ', '_') + '.c')
                source = pathlib.Path(work) / 'probe.c'
                shutil.copyfile(fixture, source)
                flags = ['-finventory-fixture-invalid'] if name == 'unknown option' else []
                result = subprocess.run(
                    [bcc, '-fsyntax-only', '--target=x86_64', *flags, str(source)],
                    text=True, capture_output=True, timeout=10, check=False,
                )
                self.assertEqual(result.returncode, 1, result.stderr)
                location = '<command-line>' if flags else f'{source}:1'
                diagnostic = next(
                    line for line in result.stderr.splitlines()
                    if re.fullmatch(r'.+:\d+:\d+: error: .+', line)
                )
                self.assertTrue(diagnostic.startswith(location + ':'), diagnostic)
                self.assertEqual(inventory.classify_diagnostic(diagnostic), name, diagnostic)


if __name__ == '__main__':
    unittest.main()
