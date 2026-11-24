"""K3 runtime archive checks (the symbol closure remains a milestone gate)."""
import os
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))
from x0_runtime import llvm_tool, symbols


class CxxRuntimeTest(unittest.TestCase):
    def test_configure_follows_ruling_86(self):
        import json
        import cxx_runtime

        licenses = json.loads((ROOT / 'toolchain/rust-host/x0/licenses.json').read_text())
        self.assertIn('libc++', licenses)
        self.assertIn('libc++abi', licenses)
        for arch in ('x86_64', 'aarch64'):
            command = cxx_runtime.configure_command(ROOT, Path('/llvm'), Path('/build'), arch)
            options = dict(arg[2:].split('=', 1) for arg in command if arg.startswith('-D'))
            for key, value in {
                'LLVM_ENABLE_RUNTIMES': 'libcxx;libcxxabi',
                'LIBCXX_CXX_ABI': 'libcxxabi',
                'LIBCXX_ENABLE_EXCEPTIONS': 'OFF',
                'LIBCXXABI_ENABLE_EXCEPTIONS': 'OFF',
                'LIBCXXABI_USE_LLVM_UNWINDER': 'OFF',
                'LIBCXX_ENABLE_LOCALIZATION': 'OFF',
                'LIBCXX_ENABLE_THREADS': 'ON',
                'LIBCXX_HAS_PTHREAD_API': 'ON',
                'LIBCXXABI_HAS_PTHREAD_API': 'ON',
                'LIBCXX_ENABLE_SHARED': 'OFF',
                'LIBCXXABI_ENABLE_SHARED': 'OFF',
                'LIBCXX_ENABLE_STATIC': 'ON',
                'LIBCXXABI_ENABLE_STATIC': 'ON',
            }.items():
                self.assertEqual(options[key], value, key)
            self.assertNotIn('LIBCXX_ENABLE_RTTI', options)
            self.assertNotIn('LIBCXXABI_ENABLE_RTTI', options)
            self.assertIn(str(ROOT / 'libs/libbreenix-libc/include'), options['CMAKE_CXX_FLAGS'])
        cxx_runtime.verify_llvm(ROOT, cxx_runtime.default_llvm())

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
