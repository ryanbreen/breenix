"""The metal producer must retain the commit required by the panic joiner."""
from pathlib import Path
import json,runpy,sys,unittest
from types import SimpleNamespace
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from metal_tools_test import StateTest,boot,m
from gt710_panic_report_test import VALID
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
SHA='a'*40

class PanicIdentity(StateTest):
    def test_actual_producer_metadata_joins_and_rejects_identity_mismatches(self):
        args=SimpleNamespace(id='20261030T000001Z-metal',mode='probe',payload='image',
            target='branch:metal-fixture',task='',commit=SHA,inject=None,suite=None,nmi=False,gt710_category='panic')
        with patch.object(m,'load_site',return_value={}),patch.object(boot,'tools_commit',return_value=SHA):
            d=boot.Driver(args);d.create_record()
        p=d.dir;r=m.read_json(p/'run.json');r['build']=dict(commit=SHA,profile='gpu-panic',features=['capture_panic_oracle']);m.write_json(p/'run.json',r)
        (p/'serial.txt').write_text(VALID);(p/'ended').write_text('1\n');(p/'exit-status').write_text('1\n')
        events=[dict(event='fatal'),dict(event='power-off',off=True),dict(event='closed',status=1,endedBy='driver')]
        (p/'events.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in events))
        self.assertEqual(API['read_panic_capture'](p,SHA)['boot']['commit'],SHA)
        for field in ('commit','toolsCommit'):
            bad=dict(r);bad[field]='b'*40;m.write_json(p/'run.json',bad)
            with self.assertRaises(ValueError):API['read_panic_capture'](p,SHA)
        m.write_json(p/'run.json',r);(p/'commit').write_text('b'*40+'\n')
        with self.assertRaises(ValueError):API['read_panic_capture'](p,SHA)

if __name__=='__main__':unittest.main()
