"""CPU memory evidence moves to its own probe boot; console keeps native gates."""
from pathlib import Path
import runpy
import unittest
API=runpy.run_path(str(Path(__file__).resolve().parents[1]/'scripts/metal/gt710-suite'))
class Routes(unittest.TestCase):
    def test_cpu_memory_records_have_their_own_route(self):
        cpu={'inventory','vm','aperture','lifetime'}
        console=set(API['capture_categories'](dict(mode='suite',suite='console')))
        probe=set(API['capture_categories'](dict(mode='probe')))
        self.assertEqual(probe,cpu)
        self.assertEqual(console,set(API['CATEGORIES'])-cpu)
        self.assertEqual(probe|console,set(API['CATEGORIES']))
        self.assertFalse(probe&console)
if __name__=='__main__':unittest.main()
