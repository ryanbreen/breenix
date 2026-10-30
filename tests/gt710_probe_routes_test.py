"""Name the CPU probe, console evidence and GPU step scoring routes."""
from pathlib import Path
import runpy
import unittest

API = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/gt710-suite'))


class Routes(unittest.TestCase):
    def test_categories_have_explicit_probe_console_and_gpu_step_routes(self):
        probe = {'inventory', 'vm', 'aperture', 'lifetime'}
        console = {
            'graphics', 'gr-contexts', 'draw', 'shaders', 'recovery', 'contexts',
            'completion', 'copy', 'channel', 'flips', 'switching', 'native',
            'memory', 'board', 'firmware', 'console', 'core', 'device',
        }
        gpu = {'resources', 'alpha', 'shapes', 'isolation'}
        self.assertEqual(set(API['capture_categories'](dict(mode='probe'))), probe)
        self.assertEqual(set(API['capture_categories'](dict(mode='suite', suite='console'))), console)
        # G12/G13/G17: GPU step rows use the separate companion capture,
        # through alpha_acceptance's explicit STEP_SELECTIONS scoring.
        self.assertEqual(set(API['STEP_SELECTIONS']), gpu)
        self.assertEqual(probe | console | gpu, set(API['CATEGORIES']))
        self.assertFalse(probe & console)
        self.assertFalse(probe & gpu)
        self.assertFalse(console & gpu)
        # GPU boots also retain native evidence, except the CE probes that
        # precede CAPS ownership transfer and the console-suite-specific row.
        self.assertEqual(set(API['capture_categories'](dict(mode='suite', suite='gpu'))),
                         console - {'recovery', 'contexts', 'console'})


if __name__ == '__main__':
    unittest.main()
