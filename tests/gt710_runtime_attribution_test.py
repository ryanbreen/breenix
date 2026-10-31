"""Retained production proof keeps its real commit across the authorized panic profile fix."""
from pathlib import Path
import runpy,tempfile,unittest,subprocess
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
PRODUCTION='92484cc052c5e992ed1d1d4b7f8e990ab2d1118e'
PANIC_PROFILE='26b26b6c89ab756b1f7ba386d52859f4ec4df0da'
class RuntimeAttribution(unittest.TestCase):
 def test_246_console_survives_only_the_pinned_receiver_repair(self):
  capture='43d5f3cc9369b71e787114f36f6787b4b1e7a4c7'
  repaired='f4cb57064d2e849928d9f329b0b3146566369ff0'
  self.assertEqual(API['alpha_runtime_identity'](capture,repaired),
    (capture,['scripts/metal/gt710_upload.py','tests/gt710_final_upload_test.py']))
  def changed_again(command, **kwargs):
   return b'scripts/metal/gt710_upload.py\0' if command[1]=='diff' else b''
  with patch('subprocess.check_output',side_effect=changed_again):
   with self.assertRaises(ValueError):API['alpha_runtime_identity'](capture,'f'*40)
 def test_246_companions_keep_their_individual_build_and_tools_commits(self):
  import json,copy,sys
  sys.path.insert(0,str(ROOT/'tests'))
  from gt710_alpha_report_test import retained_capture
  from gt710_resources_report_test import SHA
  gpu_sha='f'*40
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);gpu=retained_capture(p)
   boot=json.loads((gpu/'run.json').read_text())
   boot.update(commit=gpu_sha,toolsCommit=gpu_sha);boot['build']['commit']=gpu_sha
   (gpu/'run.json').write_text(json.dumps(boot));(gpu/'commit').write_text(gpu_sha)
   identities=[]
   def identity(actual,head):
    self.assertEqual(head,gpu_sha);identities.append(actual);return actual,[]
   with patch.dict(API['resources_capture'].__globals__,alpha_runtime_identity=identity):
    captures=API['resources_capture'](p,SHA,gpu_sha)
    self.assertEqual(captures['console']['boot']['bootedCommit'],SHA)
    self.assertEqual(captures['gpu']['boot']['bootedCommit'],gpu_sha)
    self.assertEqual(set(identities),{SHA,gpu_sha})
    for field in ('commit','toolsCommit','build'):
     bad=copy.deepcopy(boot);bad[field]=dict(boot['build'],commit=SHA) if field=='build' else SHA
     (gpu/'run.json').write_text(json.dumps(bad))
     with self.subTest(field=field),self.assertRaises(ValueError):API['resources_capture'](p,SHA,gpu_sha)
    (gpu/'run.json').write_text(json.dumps(boot))
   with self.assertRaises(ValueError):API['resources_capture'](p,SHA)
 def test_authorized_profile_change_preserves_the_runtime_commit(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'commit').write_text(PRODUCTION)
   runtime,changed=API['alpha_capture_identity'](p,PANIC_PROFILE)
   self.assertEqual(runtime,PRODUCTION)
   self.assertEqual(set(changed),{'kernel/src/gpu/gk208/native.rs','tests/gt710_panic_probe_route_test.py'})
   self.assertEqual((p/'commit').read_text(),PRODUCTION)
 def test_coordinator_fixture_repairs_preserve_the_retained_runtime(self):
  capture='bc422f5c3c070e9335f4e669c75431db325ad494'
  head='f'*40
  paths=['tests/gt710_isolation_report_test.py',
         'tests/gt710_shapes_report_test.py', 'tests/gt710_logical_mode_path_test.py',
         'tests/gt710_flip_report_test.py']
  def git_output(command, **kwargs):
   return ('\0'.join(paths)+'\0').encode() if command[1]=='diff' else b''
  with patch('subprocess.check_output',side_effect=git_output):
   runtime,changed=API['alpha_runtime_identity'](capture,head)
  self.assertEqual(runtime,capture)
  self.assertEqual(changed,paths)
 def test_g22_retained_panic_keeps_its_own_build_and_tools_identity(self):
  import json,copy,sys
  sys.path.insert(0,str(ROOT/'tests'))
  from gt710_panic_report_test import VALID
  capture='bc422f5c3c070e9335f4e669c75431db325ad494'; head='f'*40
  boot=dict(id='20261031T043900Z-metal',kind='boot',platform='metal',arch='x86_64',
    commit=capture,toolsCommit=capture,build=dict(commit=capture,profile='gpu-panic',
    features=['capture_panic_oracle']),inject=None,evidenceEligible=True,
    gt710Category='panic',mode='probe',suite=[])
  events=[dict(event='power-off',off=True),dict(event='closed',status=1,endedBy='driver')]
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp)
   for name,value in {'run.json':json.dumps(boot),'commit':capture,'dirty':'0','ended':'yes',
     'events.jsonl':'\n'.join(map(json.dumps,events)),'exit-status':'1','serial.txt':VALID}.items():
    (p/name).write_text(value)
   with patch.dict(API['read_panic_capture'].__globals__,
       alpha_runtime_identity=lambda actual,scorer: (actual,[]) if (actual,scorer)==(capture,head) else
       (_ for _ in ()).throw(ValueError('unauthorized runtime'))):
    result=API['read_panic_capture'](p,head)
    self.assertEqual(result['boot'],boot)
    self.assertEqual((p/'commit').read_text(),capture)
    for field in ('commit','toolsCommit','build'):
     bad=copy.deepcopy(boot);bad[field]=dict(boot['build'],commit=head) if field=='build' else head
     (p/'run.json').write_text(json.dumps(bad))
     with self.subTest(field=field),self.assertRaises(ValueError):API['read_panic_capture'](p,head)
    (p/'run.json').write_text(json.dumps(boot))
   with patch.dict(API['read_panic_capture'].__globals__,
       alpha_runtime_identity=lambda *a: (_ for _ in ()).throw(ValueError('unauthorized runtime'))):
    with self.assertRaisesRegex(ValueError,'unauthorized runtime'):API['read_panic_capture'](p,head)
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
  head=PANIC_PROFILE
  console,gpu=boot('console'),boot('gpu')
  for b in (console,gpu):
   b.update(commit=PRODUCTION,bootedCommit=PRODUCTION,toolsCommit=PRODUCTION)
   b['build']['commit']=PRODUCTION
  gpu.update(commit=head,bootedCommit=head,toolsCommit=head)
  gpu['build']['commit']=head
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
   for field in ('commit','toolsCommit','build'):
    broken=copy.deepcopy(producer)
    broken['gpu']['boot'][field]=dict(gpu['build'],commit=PRODUCTION) if field=='build' else PRODUCTION
    with self.subTest(gpu_field=field),self.assertRaises(ValueError):
     API['score']({},effort,records,'',console,PRODUCTION,[],broken,head)
   for field in ('commit','toolsCommit'):
    broken=copy.deepcopy(producer);broken['panic']['boot'][field]=PRODUCTION
    with self.subTest(field=field),self.assertRaises(ValueError):
     API['score']({},effort,records,'',console,PRODUCTION,[],broken,head)
   broken=copy.deepcopy(producer);broken['panic']['boot']['build']['commit']=PRODUCTION
   with self.assertRaises(ValueError):API['score']({},effort,records,'',console,PRODUCTION,[],broken,head)
  self.assertEqual(console['commit'],PRODUCTION)
  self.assertEqual(panic['commit'],head)
if __name__=='__main__':unittest.main()
