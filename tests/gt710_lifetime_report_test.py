"""M5.4 capture checks distinguish CPU lifetime exercises from engine recovery."""
import copy
import json
from pathlib import Path
import runpy
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
BASE=runpy.run_path(str(ROOT/'tests/gt710_vm_report_test.py'))
TITLE='Mapping and table holds retain faulted storage through EIO and failed reset, attribute contexts and reclaim the CPU-only baseline; engine recovery remains pending'
def fixture():
    serial,vm=BASE['fixture']();serial+='GT710_VM '+json.dumps(vm)+'\n'
    before=copy.deepcopy(vm['snapshots'][:2])
    fault=[before[0]['instance']>>12,0x1234,0,2]
    stages=[dict(phase=p,holds=h,stats=vm['peak']) for p,h in [
        ('held',[1,1,1]),('eio',[1,1,1]),('reset-failed',[1,1,1]),
        ('queued-done',[0,1,1]),('readback-done',[0,0,1]),('scanout-done',[0,0,0]),('cpu-cancel',[0,0,0])]]
    e=dict(schema=1,bdf=vm['bdf'],baseline=vm['baseline'],peak=vm['peak'],after=vm['after'],
           stages=stages,blocked=dict(release_backing=1,release_tables=4,remap=1,unmap=1,retire=6),
           fault=dict(raw=fault,instance=before[0]['instance'],address=0x1234,reason=2),
           attributed=[True,False,False],unaffected=True,snapshots_before=before,snapshots_retained=copy.deepcopy(before),
           exercise='cpu-only',translation='pending-m6',reset_execution='pending-m6')
    return serial,e
class Lifetimes(unittest.TestCase):
    def test_capture_requires_holds_retention_attribution_and_exact_reclamation(self):
        serial,e=fixture()
        def check(e):return API['memory_lifetimes'](serial+'GT710_LIFETIME '+json.dumps(e)+'\n')
        check(e)
        for mutate in [lambda b:b['stages'].pop(),lambda b:b['stages'][2].update(holds=[0,0,0]),
                       lambda b:b['stages'][5].update(stats=b['baseline']),
                       lambda b:b['blocked'].update(release_tables=0),lambda b:b['blocked'].update(retire=5),
                       lambda b:b.update(attributed=[True,True,False]),lambda b:b.update(unaffected=False),
                       lambda b:b['fault'].update(instance=b['snapshots_before'][1]['instance']),
                       lambda b:b['fault']['raw'].__setitem__(0,0),
                       lambda b:b['snapshots_retained'][0]['ptes'].__setitem__(1,0),
                       lambda b:b['snapshots_retained'][1].update(directory_sha256='0'*64),
                       lambda b:b['snapshots_before'][0].update(instance=0),lambda b:b.update(after=b['peak']),
                       lambda b:b.update(exercise='engine'),lambda b:b.update(translation='passed'),
                       lambda b:b.update(reset_execution='passed')]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)
    def test_lifetime_case_has_a_measured_metal_route(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text());API['validate_manifest'](manifest)
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='lifetime')['cases'],[dict(id='retention',title=TITLE)])
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='lifetime')['measure'],dict(suite='gt710',categories=['lifetime']))
        score=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(score['HOST_MILESTONES']['gt710-lifetime.json'],'metal-gt710/lifetime')
if __name__=='__main__':unittest.main()
