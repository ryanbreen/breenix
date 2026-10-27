"""M5.3 rejects CPU evidence that omits cache, DMA, boundary or restore checks."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
BASE=runpy.run_path(str(ROOT/'tests/gt710_vm_report_test.py'))
TITLE='BAR1 and BAR2 preserve nonce-bearing boundary patterns with explicit cache types and bounded coherent DMA; engine access remains pending'


def fixture():
    serial,vm=BASE['fixture']();serial+='GT710_VM '+json.dumps(vm)+'\n'
    m=API['display_memory'](serial);baseline=vm['baseline'];seed=0x87654321
    scratch=[[600<<20,(1<<20)+4096],[602<<20,12288]]
    tables=[[604<<20,65536],[605<<20,65536]]
    peak=dict(baseline);probe=sum(r[1] for r in scratch);engine=131072
    peak['payload']+=probe+engine;peak['guards']+=32768;peak['charged']=peak['payload']+peak['guards']
    peak['probe']+=probe;peak['engine']+=engine;peak['free']-=probe+engine+32768;peak['allocations']+=4
    views=[]
    pattern=BASE['BASE']['pattern']
    for bar in [1,2]:
        for index in [0,1,0]:
            start,size=scratch[index];table=tables[bar-1][0]
            ptes=b''.join(struct.pack('<Q',((start+i*4096)>>8)|1 if i<size//4096 else 0) for i in range(2048))
            hashes=[hashlib.sha256(b''.join(struct.pack('<I',pattern(a,seed,d)) for a in range(start,start+size,4))).hexdigest() for d in [0,bar]]
            views.append(dict(bar=bar,index=index,instance=table,directory=table+4096,table=table+8192,
                              binding=[table+4096,(8<<20)-1],pde=[0,((table+8192)>>8)|1],
                              ptes_sha256=hashlib.sha256(ptes).hexdigest(),sha256=hashes))
    pages=[]
    for address in [0x12345000,0x12347000,0x12349000]:
        data=b''.join(struct.pack('<I',pattern(a,seed,3)) for a in range(address,address+4096,4))
        pages.append([address,(address>>8)|0x500000001,hashlib.sha256(data).hexdigest()])
    e=dict(schema=1,bdf=m['bdf'],seed=seed,baseline=baseline,peak=peak,after=baseline,scratch=scratch,tables=tables,
           views=views,dma=pages,dma_bits=40,dma_access='pending-m6',translation='pending-m6',dmar_status=[0],
           memory_types=[3,1,1,0,0,0],bar_before=[0x80012345,0x40000000],bar_after=[0x80012345,0x40000000],
           pramin_before=0,pramin_after=0,protected_bytes=vm['protected_bytes'],protected_before='a'*64,protected_after='a'*64,
           guard_bytes=32768,guard_before='b'*64,guard_after='b'*64,faults_before=0,faults_after=0,
           fault_records_before=[],fault_records_after=[])
    return serial,e


class Apertures(unittest.TestCase):
    def test_independent_capture_rejects_aliases_stale_tables_cache_types_and_invented_dma(self):
        serial,e=fixture()
        def check(e):return API['cpu_apertures'](serial+'GT710_APERTURE '+json.dumps(e)+'\n')
        check(e)
        for mutate in [lambda b:b['views'].pop(),lambda b:b['views'][3].update(bar=1),
                       lambda b:b['views'][2].update(ptes_sha256=b['views'][1]['ptes_sha256']),
                       lambda b:b['views'][4].update(sha256=b['views'][3]['sha256']),
                       lambda b:b['views'][0].update(binding=[0,0]),
                       lambda b:b['dma'][0].__setitem__(0,1<<40),lambda b:b['dma'][0].__setitem__(1,1),
                       lambda b:b['dma'][1].__setitem__(0,b['dma'][0][0]),
                       lambda b:b.update(dmar_status=[1<<31]),lambda b:b.update(dma_access='passed'),
                       lambda b:b.update(translation='passed'),lambda b:b.update(memory_types=[3,3,1,0,0,0]),
                       lambda b:b['bar_after'].__setitem__(1,0),lambda b:b.update(pramin_after=1),
                       lambda b:b.update(protected_after='c'*64),lambda b:b.update(guard_after='c'*64),
                       lambda b:b.update(faults_after=1),lambda b:b['peak'].update(probe=0),
                       lambda b:b.update(after=b['peak']),lambda b:b['scratch'][1].__setitem__(0,b['scratch'][0][0])]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)

    def test_aperture_case_is_registered_for_the_metal_gate(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text());API['validate_manifest'](manifest)
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='aperture')['cases'],[dict(id='cpu-dma',title=TITLE)])
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='aperture')['measure'],dict(suite='gt710',categories=['aperture']))
        score=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(score['HOST_MILESTONES']['gt710-aperture.json'],'metal-gt710/aperture')

if __name__=='__main__':unittest.main()
