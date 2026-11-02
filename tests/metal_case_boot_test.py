"""Wire existing one-case suite selection through a diagnostic metal boot."""
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch, Mock
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parent))
import metal_tools_test as existing
boot,m=existing.boot,existing.m

class CaseBoot(existing.StateTest):
 def args(self,**changes):
  return SimpleNamespace(**(dict(id='20261101T000099Z-metal',mode='suite',payload='image',session=False,input=None,
   suite='breengel',case='text/size-range',inject=None,target='branch:fixture',task='',commit='a'*40,nmi=False,
   disk='root',sha=None,branch='fixture',no_wait=True,serve_as=None,content_type=None,ramdisk=False)|changes))
 def test_selector_is_production_and_cannot_be_full_suite_evidence(self):
  args=self.args()
  self.assertEqual(boot.kernel_profile(args),([], 'production'))
  with patch.object(m,'load_site',return_value={}),patch.object(boot,'tools_commit',return_value='a'*40):
   d=boot.Driver(args);d.create_record()
  row=m.read_json(d.dir/'run.json')
  self.assertFalse(row['evidenceEligible']);self.assertTrue(row['watch'])
  self.assertEqual(row['onlyCase'],'text/size-range')
  with self.assertRaisesRegex(ValueError,'filtered suite'):boot.metal_score.score(d.dir)
 def test_intent_uses_existing_case_and_hostkeys_grammar(self):
  self.assertTrue(callable(getattr(boot,'suite_target',None)),'compose the existing suite case boot intent')
  self.assertEqual(boot.suite_target(self.args()),'suite breengel case text/size-range hostkeys '+boot.metal_root.HOSTKEYS)
  self.assertEqual(boot.suite_target(self.args(case=None)),'suite breengel hostkeys '+boot.metal_root.HOSTKEYS)
 def test_wrong_context_or_unsafe_selector_is_refused(self):
  for changes in [dict(mode='default'),dict(payload='persistent'),dict(suite='all'),dict(suite='breengel,gpu'),
    dict(suite=None),dict(inject='panic-reboot'),dict(case='x y'),dict(case='x,y'),dict(case='x\ny')]:
   with self.subTest(changes=changes),self.assertRaises(ValueError):boot.kernel_profile(self.args(**changes))
 def test_stopped_case_watch_skips_scoring(self):
  with patch.object(m,'load_site',return_value={}):d=boot.Driver(self.args())
  d.dir.mkdir(parents=True);d.t0=100
  definition=d.dir/'definition/docs/metal/boot-path.json';definition.parent.mkdir(parents=True);definition.write_text('{}')
  d.create_record=Mock();d.claim=d.prepare_payload=d.preflight=d.do_stage=d.start_console=d.power_on=Mock()
  d.watch=Mock(side_effect=boot.Finish(m.EXIT_STOPPED,'targeted records captured'));d.power_off=Mock(return_value=True);d.cleanup=Mock();d.score_suite=Mock()
  with patch.object(boot.metal_score,'score') as score:self.assertEqual(d.run(),m.EXIT_STOPPED)
  d.score_suite.assert_not_called();score.assert_not_called()
 def test_host_forwards_case_to_driver(self):
  runner=existing.load_script('metal-run');args=self.args()
  with patch.object(runner,'host_env',return_value={'METAL_INSTANCE':'fixture'}), \
   patch.object(runner,'resolve',return_value='a'*40),patch.object(runner,'resolve_suites',return_value='breengel'), \
   patch.object(runner,'in_metal',return_value=SimpleNamespace(returncode=0,stdout='claimed',stderr='')) as send, \
   patch.object(runner.subprocess,'Popen') as processes,patch.object(runner.Path,'home',return_value=m.STATE):
   self.assertEqual(runner.boot(args),0)
  command=send.call_args.args
  self.assertIn('--case',command);self.assertEqual(command[command.index('--case')+1],'text/size-range')
  self.assertFalse(any('_resume' in call.args[0] for call in processes.call_args_list),'a stopped diagnostic must not spawn another boot')
if __name__=='__main__':unittest.main()
