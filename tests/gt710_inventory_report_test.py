"""M5.1 judges physical ownership and cross-aperture bytes, never engine translation."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
BASE=runpy.run_path(str(ROOT/'tests/gt710_memory_report_test.py'))


def pattern(address,seed,direction):
    x=((address//4)^seed^(0xa511e9b3*direction))&0xffffffff
    x^=x>>16;x=x*0x7feb352d&0xffffffff;x^=x>>15;x=x*0x846ca68b&0xffffffff
    return (x^(x>>16))&0xffffffff


def fixture():
    memory=BASE['evidence']()
    payload=sum(o['size'] for o in memory['objects'])
    reserved=sum(r[1] for r in memory['reserved'])
    baseline=dict(reserved=reserved,payload=payload,guards=49152,charged=payload+49152,
                  display=payload,engine=0,probe=0,free=memory['total']-reserved-payload-49152,allocations=6)
    scratch=[[256<<20,(1<<20)+4096],[(258<<20)+4096,12288]]
    tables=[260<<20,65536]
    peak=dict(baseline)
    peak.update(engine=65536,probe=sum(r[1] for r in scratch),allocations=9)
    peak['payload']+=peak['engine']+peak['probe'];peak['guards']+=24576
    peak['charged']=peak['payload']+peak['guards'];peak['free']=memory['total']-reserved-peak['charged']
    views=[]
    for index in [0,1,0]:
        start,size=scratch[index]
        ptes=[((start+i*4096)>>8)|1 if i<size//4096 else 0 for i in range(2048)]
        views.append(dict(index=index,instance=tables[0],directory=tables[0]+4096,table=tables[0]+8192,
                          pde=[0,((tables[0]+8192)>>8)|1],ptes_sha256=hashlib.sha256(struct.pack('<2048Q',*ptes)).hexdigest(),
                          sha256=[hashlib.sha256(b''.join(struct.pack('<I',pattern(a,0x12345679,d)) for a in range(start,start+size,4))).hexdigest() for d in range(2)]))
    e=dict(schema=1,phase='inventory',bdf=memory['bdf'],total=memory['total'],aperture=memory['aperture'],seed=0x12345679,
           baseline=baseline,peak=peak,after=baseline,scratch=scratch,tables=tables,views=views,
           protected_bytes=reserved+payload+49152,protected_before='a'*64,protected_after='a'*64,
           bar_before=0x80000000|((memory['objects'][4]['start']+8192)>>12),bar_after=0x80000000|((memory['objects'][4]['start']+8192)>>12),pramin_before=0,pramin_after=0,
           faults_before=0,faults_after=0,fault_records=[],translation='pending-m6',
           scratch_guard_bytes=24576,scratch_guard_before='b'*64,scratch_guard_after='b'*64)
    return memory,e


class Inventory(unittest.TestCase):
    def test_inventory_gate_rejects_aliases_undercharges_and_invented_engine_proof(self):
        memory,e=fixture()
        def check(e):
            return API['memory_inventory'](BASE['SERIAL']+'GT710_MEMORY '+json.dumps(memory)+'\nGT710_VRAM '+json.dumps(e)+'\n')
        check(e)
        for change in [lambda b:b['after'].update(free=b['after']['free']-4096),
                       lambda b:b['peak'].update(engine=0),
                       lambda b:b.update(aperture=b['total']),
                       lambda b:b['scratch'][1].__setitem__(0,b['scratch'][0][0]),
                       lambda b:b.update(protected_after='b'*64),
                       lambda b:b.update(protected_bytes=4096),
                       lambda b:b.update(bar_after=0),lambda b:b.update(faults_after=16),
                       lambda b:b.update(bar_before=0x80000001,bar_after=0x80000001),
                       lambda b:b.update(scratch_guard_after='c'*64),
                       lambda b:b.update(scratch_guard_bytes=4096),
                       lambda b:b.update(translation='passed'),lambda b:b['views'].pop(),
                       lambda b:b['views'][0].update(ptes_sha256='0'*64),
                       lambda b:b['views'][1]['sha256'].__setitem__(0,b['views'][0]['sha256'][0]),
                       lambda b:b['views'][2]['sha256'].__setitem__(1,b['views'][2]['sha256'][0])]:
            bad=copy.deepcopy(e);change(bad)
            with self.assertRaises(ValueError):check(bad)

    def test_inventory_is_measured_through_the_normal_metal_scorer(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='inventory')['cases'][0]['id'],'allocator')
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='inventory')['measure'],dict(suite='gt710',categories=['inventory']))
        score=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(score['host_milestone']('gt710-inventory.json'),'metal-gt710/inventory')

if __name__=='__main__':unittest.main()
