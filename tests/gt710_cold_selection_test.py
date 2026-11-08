"""A closed non-disk selection keeps its own verdict after a cold-disk tail."""
import json
import unittest
import shutil
from pathlib import Path
from gt710_wm_step_test import WmStep, API, SHA, NAMES, SELECTED, serial

CHILD = '20261104T000001Z-metal'

class ColdSelection(unittest.TestCase):
    def setUp(self):
        self.fixture = WmStep()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.path = self.fixture.path
        boot = dict(self.fixture.boot, suite=None, suites=['window-manager', 'files-io'])
        (self.path / 'run.json').write_text(json.dumps(boot))
        self.fixture.uploads(self.path / 'native-suites/window-manager', '/suites/window-manager')
        text = serial()
        for name in NAMES:
            if name not in SELECTED:
                text = text.replace(f'CASE {name} FAIL ms=1 msg=pending', f'CASE {name} PASS ms=1')
        root = Path(__file__).resolve().parents[1]
        shutil.copyfile(root/'docs/suites/files-io.json', self.path/'definition/docs/suites/files-io.json')
        empty = [c['id'] for c in json.loads((root/'docs/suites/window-manager.json').read_text())['categories'] if not c['cases']]
        self.verdict = ('FAIL window-manager: planned categories have no cases: '+', '.join(empty)+'; acceptance is blocked\n'
            'PASS files-io: SUITE files-io DONE passed=219 failed=0 skipped=0 total=219; files-io MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48; raw disk checks passed=5\n'
            'FAIL: 1 of 2 suites passed\n')
        (self.path/'suite-verdict.txt').write_text(self.verdict)
        self.text = text.replace(f'DONE passed={len(SELECTED)} failed={len(NAMES)-len(SELECTED)}',
                                 f'DONE passed={len(NAMES)} failed=0')
        (self.path / 'serial.txt').write_text(self.text)
        self.events = [dict(event='final-marker'), dict(event='power-off', off=True),
            dict(event='closed', status=1, endedBy='driver'),
            dict(event='cold-readback-next', readback=CHILD),
            dict(event='suite-verdict', status=1),
            dict(event='cold-disk-verdict', readback=CHILD, passed=False)]
        self.receipt = dict(id=CHILD, bytes=4096, sha256='c'*64, passed=False)
        self.save()

    def save(self):
        (self.path/'events.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in self.events))
        (self.path/'suite-readback.json').write_text(json.dumps(self.receipt))

    def test_completed_selection_keeps_case_and_upload_checks(self):
        originals = {n:(self.path/n).read_bytes() for n in
            ('run.json','events.jsonl','suite-readback.json','exit-status','serial.txt')}
        self.assertEqual(API['record_wm_capture'](self.path, SHA, 7), 0)
        for name, contents in originals.items():
            self.assertEqual((self.path/name).read_bytes(), contents)
        bad = self.text.replace(f'CASE {NAMES[0]} PASS ms=1', f'CASE {NAMES[0]} FAIL ms=1 msg=planted')
        bad = bad.replace(f'DONE passed={len(NAMES)} failed=0', f'DONE passed={len(NAMES)-1} failed=1')
        (self.path/'serial.txt').write_text(bad)
        self.assertEqual(API['record_wm_capture'](self.path, SHA, 7), 1)
        report = json.loads((self.path/'gt710-wm-step.json').read_text())
        self.assertIn('case failure',report['cases'][0]['reason'])
        (self.path/'serial.txt').write_text(self.text)
        (self.path/'native-suites/window-manager/gt710-pixels.rle').write_bytes(b'corrupt')
        self.assertEqual(API['record_wm_capture'](self.path, SHA, 7), 1)

    def test_aggregate_failure_must_only_name_planned_empty_categories(self):
        for text in [self.text+'SUITE files-io CASE io/write FAIL ms=1 msg=planted\n',
                     self.text+'SUITE files-io CASE io/write SKIP ms=1\n']:
            (self.path/'serial.txt').write_text(text)
            with self.assertRaises(ValueError):
                API['completed_suite_capture'](self.path,SHA,'window-manager')
        (self.path/'serial.txt').write_text(self.text)
        (self.path/'suite-verdict.txt').write_text(self.verdict.replace('planned categories have no cases:','unknown refusal:'))
        with self.assertRaises(ValueError):
            API['completed_suite_capture'](self.path,SHA,'window-manager')
        (self.path/'suite-verdict.txt').write_text(self.verdict)
        self.events[3],self.events[4]=self.events[4],self.events[3];self.save()
        with self.assertRaises(ValueError):
            API['completed_suite_capture'](self.path,SHA,'window-manager')

    def test_tail_is_bound_and_disk_failure_stays_a_failure(self):
        with self.assertRaises(ValueError):
            API['completed_suite_capture'](self.path, SHA, 'files-io')
        for name,value in [('passed',True),('id','20261104T000002Z-metal'),('bytes',0),('sha256','invalid')]:
            old=self.receipt[name];self.receipt[name]=value;self.save()
            with self.assertRaises(ValueError):
                API['completed_suite_capture'](self.path,SHA,'window-manager')
            self.receipt[name]=old
        self.save()
        for index,key,value in [(1,'off',False),(3,'readback','20261104T000002Z-metal'),
                                (4,'status',0),(5,'passed',True)]:
            old=self.events[index][key];self.events[index][key]=value;self.save()
            with self.assertRaises(ValueError):
                API['completed_suite_capture'](self.path,SHA,'window-manager')
            self.events[index][key]=old
        self.events[4]['status']=0;self.events[5]['passed']=True;self.receipt['passed']=True
        (self.path/'exit-status').write_text('0\n');self.save()
        self.assertEqual(API['record_wm_capture'](self.path,SHA,7),0)

if __name__=='__main__':unittest.main()
