"""K3 runtime archive checks (the symbol closure remains a milestone gate)."""
import os
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))
from x0_runtime import llvm_tool, symbols


class CxxRuntimeTest(unittest.TestCase):
    def test_cxx_runtime_undefined_set_is_defined(self):
        nm = llvm_tool(ROOT, 'llvm-nm')
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                base = Path(os.environ.get('BREENIX_CXX_RUNTIME_ROOT', ROOT / 'build/cxx-runtime')) / arch
                archives = [base / 'lib/libc++.a', base / 'lib/libc++abi.a']
                providers = archives + [base / 'lib/libc.a', base / 'lib/compiler_builtins.rlib']
                for path in providers:
                    self.assertTrue(path.is_file(), f'missing required archive: {path}')
                undefined = set().union(*(symbols(nm, path, '--undefined-only') for path in archives))
                defined = set().union(*(symbols(nm, path, '--defined-only') for path in providers))
                remaining = undefined - defined
                self.assertFalse({s for s in undefined if s.startswith('_Unwind_') or s == '__gxx_personality_v0'},
                                 f'{arch}: unexpected unwinder references: {sorted(remaining)}')
                self.assertFalse(remaining, f'{arch}: undefined symbols: {sorted(remaining)}')


if __name__ == '__main__':
    unittest.main()
