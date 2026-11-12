"""Measure the production shader catalogue through the installed host GL stack."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class AngleCache(unittest.TestCase):
    def test_real_catalogue_links_and_persistent_cache_is_used(self):
        with tempfile.TemporaryDirectory(prefix='a3-angle-check-') as directory:
            subprocess.run([sys.executable, str(ROOT / 'scripts/measure-angle-cache.py'),
                            '--output', directory], check=True)
            result = json.loads((Path(directory) / 'results.json').read_text())
            self.assertEqual([r['mode'] for r in result['runs']],
                             ['disabled', 'populate', 'persistent'])
            for run in result['runs']:
                self.assertIn('ANGLE', run['renderer'])
                self.assertIn('Metal', run['renderer'])
                self.assertEqual(len(run['contexts']), 2)
                for context in run['contexts']:
                    self.assertEqual([p['pipeline'] for p in context], list(range(16)))
                    for program in context:
                        self.assertTrue(program['linked'])
                        self.assertGreater(program['compile_ms'], 0)
                        self.assertGreater(program['link_ms'], 0)
            self.assertEqual(result['runs'][0]['cache']['gets'], 0)
            self.assertGreater(result['runs'][1]['cache']['puts'], 0)
            self.assertGreater(result['runs'][2]['cache']['hits'], 0)
            self.assertEqual(len(result['shaders']), 17)  # shared VS plus 16 FS


if __name__ == '__main__':
    unittest.main()
