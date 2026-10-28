"""M5.2 table/aperture evidence never certifies engine translation."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
BASE=runpy.run_path(str(ROOT/'tests/gt710_inventory_report_test.py'))
VA=(128<<20)-4096
LIMIT=1<<40
TITLE='Private GPU tables preserve distinct same-VA backing, ordered remaps and reserved storage; engine translation remains pending'


def snapshot(space,phase,base,bits):
    instance=(300+space*2)<<20;directory=instance+12288
    tables=[[0,instance+0x20000],[1,instance+0x70000]]
    pd=bytearray(65536);hashes=[]
    for index,start in tables:
        struct.pack_into('<Q',pd,index*8,((start>>8)|1)<<32)
        table=bytearray(262144)
        if base is not None:
            for n in range(3):
                v=VA+n*4096
                if v//(128<<20)==index:
                    struct.pack_into('<Q',table,(v%(128<<20))//4096*8,((base+n*4096)>>8)|bits)
        hashes.append(hashlib.sha256(table).hexdigest())
    return dict(space=space,phase=phase,instance=instance,directory=directory,tables=tables,
                binding=[directory,LIMIT-1],directory_sha256=hashlib.sha256(pd).hexdigest(),tables_sha256=hashes,
                ptes=[0]+([((base+n*4096)>>8)|bits for n in range(3)] if base is not None else [0]*3)+[0])


def fixture():
    m,inventory=BASE['fixture']();baseline=inventory['baseline'];seed=0xabcdef01
    backing=[[512<<20,12288],[513<<20,12288],[514<<20,12288]]
    snaps=[snapshot(0,'map',backing[0][0],7),snapshot(1,'map',backing[1][0],1),
           snapshot(0,'remap',backing[2][0],(1<<32)|1),snapshot(0,'unmap',None,0),
           snapshot(0,'restore',backing[0][0],7),snapshot(1,'unaffected',backing[1][0],1)]
    pd=[snaps[i]['directory'] for i in [0,1]]
    engine=2*(4096+65536+2*262144)+65536
    peak=dict(baseline,engine=engine,probe=36864,allocations=18)
    peak['payload']+=engine+36864;peak['guards']+=12*8192
    peak['charged']=peak['payload']+peak['guards'];peak['free']=m['total']-peak['reserved']-peak['charged']
    hashes=[]
    for start,size in backing:
        hashes.append([hashlib.sha256(b''.join(struct.pack('<I',BASE['pattern'](a,seed,d)) for a in range(start,start+size,4))).hexdigest() for d in range(2)])
    e=dict(schema=1,bdf=m['bdf'],seed=seed,va=VA,limit=LIMIT,baseline=baseline,peak=peak,after=baseline,
           backing=backing,backing_sha256=hashes,view=[310<<20,65536],snapshots=snaps,
           invalidations=[[d,d>>8,0x80000001,0x18000] for d in [pd[0],pd[1],pd[0],pd[1],pd[0],pd[0],pd[0],pd[0],pd[1]]],
           protected_bytes=inventory['protected_bytes'],protected_before='a'*64,protected_after='a'*64,
           guard_bytes=12*8192,guard_before='b'*64,guard_after='b'*64,
           bar_before=inventory['bar_before'],bar_after=inventory['bar_after'],pramin_before=0,pramin_after=0,
           faults_before=0,faults_after=0,fault_records_before=[],fault_records_after=[],translation='pending-m6')
    serial=BASE['BASE']['SERIAL']+'GT710_MEMORY '+json.dumps(m)+'\nGT710_VRAM '+json.dumps(inventory)+'\n'
    return serial,e


class VmCapture(unittest.TestCase):
    def test_vm_gate_rejects_missing_stale_aliased_or_invented_translation_evidence(self):
        serial,e=fixture()
        def check(e):return API['virtual_memory'](serial+'GT710_VM '+json.dumps(e)+'\n')
        check(e)
        for mutate in [lambda b:b.update(translation='passed'),lambda b:b['snapshots'].pop(),
                       lambda b:b['snapshots'][2].update(tables_sha256=b['snapshots'][0]['tables_sha256']),
                       lambda b:b['snapshots'][5].update(ptes=b['snapshots'][4]['ptes']),
                       lambda b:b['snapshots'][0].update(binding=[0,LIMIT-1]),
                       lambda b:b['snapshots'][1].update(instance=b['snapshots'][0]['instance']),
                       lambda b:b['snapshots'][0]['tables'][0].__setitem__(1,b['backing'][0][0]),
                       lambda b:b['snapshots'][3].update(ptes=b['snapshots'][0]['ptes']),
                       lambda b:b['backing_sha256'][1].__setitem__(0,b['backing_sha256'][0][0]),
                       lambda b:b['invalidations'][4].__setitem__(2,0x80000005),
                       lambda b:b['invalidations'][4].__setitem__(3,0),
                       lambda b:b['invalidations'].pop(),lambda b:b.update(faults_after=1),
                       lambda b:b.update(fault_records_before=[[0,0,0,0,0]]),
                       lambda b:b.update(protected_after='c'*64),lambda b:b.update(guard_after='c'*64),
                       lambda b:b.update(bar_after=0),lambda b:b['after'].update(engine=4096),
                       lambda b:b['peak'].update(engine=0),lambda b:b.update(seed=0)]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)

    def test_vm_category_is_measured_by_the_metal_milestone(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='vm')['cases'],[dict(id='tables',title=TITLE)])
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='vm')['measure'],dict(suite='gt710',categories=['vm']))
        score=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(score['host_milestone']('gt710-vm.json'),'metal-gt710/vm')

if __name__=='__main__':unittest.main()
