"""Native context records must reach the retained metal milestone scorer."""
import runpy
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
class ContextMilestone(unittest.TestCase):
    def test_context_record_is_admitted(self):
        api=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(api['host_milestone']('gt710-contexts.json'),'metal-gt710/contexts')
        import json
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text())
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='contexts')['cases'][0]['id'],'concurrent')
if __name__=='__main__':unittest.main()
