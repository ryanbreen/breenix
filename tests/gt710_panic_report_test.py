"""The native terminal capture must prove the bound diagnostics, never fallback."""
import runpy
from pathlib import Path
from types import SimpleNamespace
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
VALID = ('KERNEL PANIC: capture_panic_oracle: deliberate panic\n'
         '[BXCAP:BEGIN seq=1 edge=PANIC]\n'
         '[BXCAP:END seq=1 edge=PANIC verdict=complete truncated=0 sections_skipped=0]\n'
         '[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0]\n'
         '[GPU_PANIC_NATIVE target=42 bound=42 poisons=55,aa equal=1 firmware_writes=0]\n')


def producer(text):
    return {'panic': {'serial': text, 'boot': {
        'gt710Category': 'panic', 'mode': 'probe', 'suite': [],
        'build': {'profile': 'gpu-panic', 'features': ['capture_panic_oracle']}}}}


class NativePanic(unittest.TestCase):
    def test_bound_panic_and_each_required_piece(self):
        API['check_case']('presentation', 'panic', '', producer(VALID))
        for bad in (
            VALID.replace('edge=PANIC', 'edge=FAULT'),
            VALID.replace('KERNEL PANIC:', 'missing banner:'),
            VALID.replace('capture_panic_oracle: deliberate panic', 'unexpected panic'),
            VALID.replace('target=42 ', ''), VALID.replace('bound=42', 'bound=43'),
            VALID.replace('poisons=55,aa', 'poisons=55'),
            VALID.replace('equal=1', 'equal=0'),
            VALID.replace('mismatches=0', 'mismatches=1'),
            VALID.replace('firmware_writes=0', 'firmware_writes=1'),
            VALID.replace('firmware_writes=0', ''),
            VALID.replace('observed=1', 'observed=0'),
            VALID.replace('shown=1', 'shown=0'),
            VALID + '[BXCAP:BEGIN seq=2 edge=LOCKUP]\n',
            VALID + '[GPU_PANIC_NATIVE target=42 bound=42 poisons=55,aa equal=1 firmware_writes=0]\n',
            VALID.split('[GPU_PANIC_SCREEN')[0],
        ):
            with self.subTest(serial=bad), self.assertRaises(ValueError):
                API['check_case']('presentation', 'panic', '', producer(bad))

    def test_timeout_retains_serial_but_fails(self):
        serial = VALID.split('[GPU_PANIC_SCREEN')[0] + '[GPU_PANIC_SCREEN shown=0 observed=0]\n'
        self.assertIn('KERNEL PANIC:', serial)
        with self.assertRaises(ValueError):
            API['check_case']('presentation', 'panic', '', producer(serial))

    def test_explicit_diagnostic_feature_only(self):
        import sys
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        run = runpy.run_path(str(ROOT / 'scripts/metal/metal-run'))
        boot = runpy.run_path(str(ROOT / 'scripts/metal/metal-boot'))
        build = runpy.run_path(str(ROOT / 'scripts/metal/metal-build'))
        self.assertEqual(run['kernel_features']('probe', '', None, 'panic'),
                         ['--features', 'capture_panic_oracle'])
        self.assertEqual(boot['kernel_profile'](SimpleNamespace(mode='probe', suite=[], inject=None, gt710_category='panic')),
                         (['capture_panic_oracle'], 'gpu-panic'))
        self.assertEqual(build['PROFILES']['capture_panic_oracle'], 'gpu-panic')
        self.assertEqual(run['kernel_features']('suite', 'gpu', None), [])
        self.assertEqual(run['kernel_features']('default', '', None), [])
        for mode, suites, inject in [('tests', '', None), ('suite', 'gpu', None), ('probe', '', 'panic-reboot')]:
            with self.assertRaises(ValueError):
                run['kernel_features'](mode, suites, inject, 'panic')


if __name__ == '__main__':
    unittest.main()
