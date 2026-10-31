"""Retained production proof keeps its real commit across the authorized panic profile fix."""
from pathlib import Path
import runpy,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
PRODUCTION='92484cc052c5e992ed1d1d4b7f8e990ab2d1118e'
PANIC_PROFILE='26b26b6c89ab756b1f7ba386d52859f4ec4df0da'
class RuntimeAttribution(unittest.TestCase):
 def test_authorized_profile_change_preserves_the_runtime_commit(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'commit').write_text(PRODUCTION)
   runtime,changed=API['alpha_capture_identity'](p,PANIC_PROFILE)
   self.assertEqual(runtime,PRODUCTION)
   self.assertEqual(set(changed),{'kernel/src/gpu/gk208/native.rs','tests/gt710_panic_probe_route_test.py'})
   self.assertEqual((p/'commit').read_text(),PRODUCTION)
 def test_prior_runtime_changes_are_not_covered_by_the_profile_ruling(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'commit').write_text('becddfad'+__import__('subprocess').check_output(['git','rev-parse','becddfad'],cwd=ROOT,text=True).strip()[8:])
   with self.assertRaises(ValueError):API['alpha_capture_identity'](p,PANIC_PROFILE)
if __name__=='__main__':unittest.main()
