import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('inventory', ROOT / 'scripts/toolchain-busybox-c-inventory.py')
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class Inventory(unittest.TestCase):
    def test_classifies_fixture_diagnostics(self):
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


if __name__ == '__main__':
    unittest.main()
