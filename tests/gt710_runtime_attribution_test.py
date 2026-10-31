"""Retained production proof keeps its real commit across the authorized panic profile fix."""
from pathlib import Path
import runpy,tempfile,unittest,subprocess
from unittest.mock import patch
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
   p=Path(tmp);(p/'commit').write_text(subprocess.check_output(['git','rev-parse','becddfad'],cwd=ROOT,text=True).strip())
   with self.assertRaises(ValueError):API['alpha_capture_identity'](p,PANIC_PROFILE)
 def test_later_terminal_repairs_are_not_covered_by_the_profile_ruling(self):
  # The coordinator retained 92484cc0 only through 26b26b6c. The UART
  # and USERD repairs after that ruling change runtime files, not host files.
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'commit').write_text(PRODUCTION)
   with self.assertRaisesRegex(ValueError, 'runtime or unauthorized files'):
    API['alpha_capture_identity'](p,'ce2e92175c26d39f66429c4cb489502f0ac31981')
 def test_presentation_joins_exact_identities_without_relabeling(self):
  import sys,copy
  sys.path.insert(0,str(ROOT/'tests'))
  from gt710_resources_report_test import boot
  from gt710_panic_report_test import VALID
  head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
  console,gpu=boot('console'),boot('gpu')
  for b in (console,gpu):
   b.update(commit=PRODUCTION,bootedCommit=PRODUCTION,toolsCommit=PRODUCTION)
   b['build']['commit']=PRODUCTION
  panic=dict(id='20261031T000003Z-metal',commit=head,toolsCommit=head,
    build=dict(commit=head,profile='gpu-panic',features=['capture_panic_oracle']),
    gt710Category='panic',mode='probe',suite=[])
  producer=dict(console=dict(boot=console,serial='',events=[]),gpu=dict(boot=gpu,serial=''),panic=dict(boot=panic,serial=VALID))
  rows=[dict(id='presentation/'+n,result='PASS') for n in ('display','panic','suite')]
  records=dict(boot=console['id'],commit=PRODUCTION,milestone='metal-gt710/presentation',producerBoot=gpu['id'],
    gpuCases=[],gpuCaseLines=[],gpuElapsedMS=1,consoleHandoffToDoneMS=1,cases=rows)
  effort=dict(efforts=[dict(id='metal-gt710',milestones=[dict(id='presentation',measure=dict(suite='gt710',categories=['presentation']))])])
  # The existing pixel/transport validators have separate mutation coverage;
  # exercise this join's real runtime, profile, tools and panic checks.
  replacements=dict(validate_manifest=lambda _:None,resources_suite=lambda *a:dict(cases=[],elapsed_ms=1),
    resources_console=lambda *a:1,cases=lambda *a:rows)
  with patch.dict(API['score'].__globals__,replacements):
   self.assertEqual(API['score']({},effort,records,'',console,PRODUCTION,[],producer,head),'presentation: 3 cases passed')
   for field in ('commit','toolsCommit'):
    broken=copy.deepcopy(producer);broken['panic']['boot'][field]=PRODUCTION
    with self.subTest(field=field),self.assertRaises(ValueError):
     API['score']({},effort,records,'',console,PRODUCTION,[],broken,head)
   broken=copy.deepcopy(producer);broken['panic']['boot']['build']['commit']=PRODUCTION
   with self.assertRaises(ValueError):API['score']({},effort,records,'',console,PRODUCTION,[],broken,head)
  self.assertEqual(console['commit'],PRODUCTION)
  self.assertEqual(panic['commit'],head)
if __name__=='__main__':unittest.main()
