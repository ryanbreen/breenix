"""Offline DDR3 table selection, PMU stream ordering and unresolved capture inputs."""
import copy
import importlib.util
import json
from pathlib import Path
import struct
import re
import subprocess
import tempfile
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('gt710_memx', ROOT/'scripts/metal/gt710_memx.py')
API = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = API
SPEC.loader.exec_module(API)
CARD = json.loads((ROOT/'tests/fixtures/gt710_c5a_card.json').read_text())


class Memx(unittest.TestCase):
    def test_card_strap_ranges_timings_and_pll_limits(self):
        d = API.decode(CARD)
        self.assertEqual((d['strap'], d['ramcfg_index'], d['crystal_khz']), (1, 1, 27000))
        self.assertEqual([(c['min_mhz'], c['max_mhz']) for c in d['configs']],
                         [(0,162), (163,460), (461,750), (750,1200)])
        lo, hi = API.select(d, 405000), API.select(d, 900000)
        self.assertEqual((lo['timing_index'], hi['timing_index']), (1, 2))
        self.assertEqual(lo['ramcfg_raw'], '0114a00300020081050100')
        self.assertEqual(hi['ramcfg_raw'], '0214405100020014040100')
        self.assertEqual(hi['timing'], [0x0d40ed2d,0x30d3448d,0x550e0711,0x4abe,
                                       0x46c38069,0x27000,0x1009c1c,0x34bc00,0x3700,8,0x80000])
        self.assertEqual((d['refpll']['min_freq'],d['refpll']['max_freq']), (600000,1200000))
        self.assertEqual((d['mempll']['min_freq'],d['mempll']['max_freq']), (1500000,3000000))
        self.assertEqual(API.pll(d['refpll'],900000), (1,32,1,0x1aab))  # M,N,P,fN
        self.assertEqual(API.pll(d['refpll'],405000), (1,29,2,0x1000))

    def test_synthetic_rammap_stride_inclusive_range_and_timing_bitfields(self):
        # v11: two 18-byte entries, two ten-byte strap configs each.
        raw = bytes.fromhex('1106120a0202') + bytes(76)
        b = bytearray(raw)
        for i,(low,high) in enumerate([(0,750),(750,1000)]):
            q = 6+i*38
            struct.pack_into('<HH',b,q,low,high)
            b[q+8] = 0x1d
            struct.pack_into('<I',b,q+9,0x0abcdef1)
            b[q+18:q+28] = bytes.fromhex('0025ccf9000000dc2107')
            b[q+28:q+38] = bytes.fromhex('0121c0a6000000182009')
        configs = API.parse_rammap(API.Image(bytes(b)),0,1)
        self.assertEqual(configs[1]['ramcfg_raw'],'0121c0a6000000182009')
        self.assertEqual(configs[0]['ramcfg_DLLoff'],1)
        self.assertEqual(configs[0]['ramcfg_11_03_c0'],2)
        self.assertEqual(configs[0]['ramcfg_11_03_30'],2)
        self.assertEqual(API.select({'configs':configs},750999)['min_mhz'],0)
        t = bytearray(51);struct.pack_into('<H',t,44,0x1fc0|0x25);t[48]=0xfd
        x = API.parse_timing(bytes(t))
        self.assertEqual((x['timing_20_2c_003f'],x['timing_20_2c_1fc0'],x['timing_20_30_07'],x['timing_20_30_f8']), (37,127,5,31))
        with self.assertRaises(ValueError): API.parse_rammap(API.Image(bytes(b[:-1])),0,1)

    def test_card_missing_capture_is_not_replaced_with_assumed_state(self):
        p = API.generate(CARD)
        self.assertFalse(p['exact'])
        self.assertIsNone(p['words'])
        self.assertTrue({0x132034,0x10f300,0x10f304,0x10f320,0x10f200,0x10f808,
                         0x10f824,0x1373ec,0x132040}.issubset(set(p['missing_registers'])))
        self.assertEqual([p['frequency_khz'] for p in p['phases']], [405000,900000])
        self.assertEqual(p['phases'][0]['config']['timing_20_30_07'],6)
        for phase in p['phases']:
            self.assertTrue(all(o['source'] for o in phase['ops']))
            self.assertFalse(any(o['op'] in ('VBLANK','TRAIN') for o in phase['ops']))

    def test_synthetic_complete_state_lowers_order_and_preserves_unmasked_bits(self):
        # Synthetic register completion for this test only.
        f = copy.deepcopy(CARD)
        for a in API.generate(f)['missing_registers']:
            f['registers'][f'0x{a:06x}'] = '0x00000000'
        f['registers']['0x10f304'] = '0x80000244'  # preserve ODT and unrelated high bit
        f['registers']['0x10f300'] = '0x10000000'
        f['registers']['0x10f320'] = '0x20000000'
        p = API.generate(f)
        self.assertTrue(p['exact']);self.assertIsNotNone(p['words'])
        self.assertEqual(p['final_registers']['0x10f300'],0x10000e14)
        self.assertEqual(p['final_registers']['0x10f304'],0x80000244)
        self.assertEqual(p['final_registers']['0x10f320'],0x20000020)
        self.assertEqual(p['final_registers']['0x132024'],0x12001)
        self.assertEqual(p['final_registers']['0x132034']&0xffff,0x1aab)
        for phase in p['phases']:
            ops=[o for o in phase['ops'] if o['emitted']]
            self.assertLess(next(i for i,o in enumerate(ops) if o['op']=='ENTER'),
                            next(i for i,o in enumerate(ops) if o.get('register')==0x10f210))
            self.assertEqual([o['args'] for o in ops if o['op']=='WAIT'],
                             [[0x137390,0x20000,0x20000,64000]])
            self.assertEqual(len([o for o in ops if o['op']=='LEAVE']),1)
            self.assertTrue(any(o.get('register')==0x10f300 and o.get('force') for o in ops))
        self.assertEqual(API.unpack(p['words']),[(o['op'],o['args']) for s in p['phases'] for o in s['ops'] if o['emitted']])

    def test_synthetic_emission_matches_original_mit_c_ddr3_routines(self):
        source=(ROOT/'tests/fixtures/gt710_ddr3_oracle.c').read_text()
        decl=source.split('struct nvbios_ramcfg {',1)[1].split('};',1)[0]
        names=re.findall(r'u32 (\w+);',decl)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            exe=str(Path(tmp)/'oracle')
            subprocess.run(priority+['cc','-std=c11','-Wall','-Werror',
                           str(ROOT/'tests/fixtures/gt710_ddr3_oracle.c'),'-o',exe],check=True)
            for dll_off in (False,True):
                f=copy.deepcopy(CARD)
                if dll_off:
                    # Synthetic target with DLL disabled; current table stays unchanged.
                    raw=bytearray.fromhex(f['segments']['0x7259'])
                    raw[0x73cd-0x7259+1]|=0x20
                    f['segments']['0x7259']=raw.hex()
                for a in API.generate(f)['missing_registers']:
                    f['registers'][f'0x{a:06x}']='0x00000000'
                f['registers']['0x10f304']='0x80000244'
                f['registers']['0x10f300']='0x10000000'
                f['registers']['0x10f320']='0x20000000'
                p=API.generate(f)
                regs={int(a,0):int(v,0) for a,v in f['registers'].items()}
                chunks=[f'{len(regs)} {len(p["phases"])} 1']
                chunks += [f'{a:x} {v:x}' for a,v in regs.items()]
                expected=[]
                for phase in p['phases']:
                    c=phase['config'];co=phase['coefficients']
                    chunks.append(' '.join(str(co[n]) for n in ('M','N','P','fN'))+' 1')
                    chunks.append(' '.join(str(c.get(n,0)) for n in names))
                    chunks.append(' '.join(str(v) for v in c['timing']))
                    expected.extend((o['op'],o['args']) for o in phase['ops'] if o['emitted'])
                output=subprocess.check_output([exe],input='\n'.join(chunks)+'\n',text=True)
                actual=[]
                for line in output.splitlines():
                    if line=='PHASE':continue
                    op,*args=line.split();actual.append((op,[int(a,16) for a in args]))
                self.assertEqual(actual,expected)

    def test_memx_packet_coalescing_and_wait_delay_flush(self):
        ops=[('WR32',[i,i+1]) for i in range(32)]+[('WAIT',[1,2,2,64000]),('DELAY',[1000]),('ENTER',[]),('LEAVE',[])]
        words=API.pack(ops)
        self.assertEqual(words[0],(62<<16)|3)
        self.assertEqual(words[63],(2<<16)|3)
        self.assertEqual(API.unpack(words),ops)

    def test_false_source_branch_does_not_read_missing_registers(self):
        s=API.Script({})
        s.guard=False
        s.mask(0x132034,0xffff,0x1000,'fb/ramgk104.c:798')
        s.command('WAIT',[0x137390,0x20000,0x20000,64000],'fb/ramgk104.c:802')
        self.assertEqual(s.missing,set())
        self.assertEqual(s.ops,[])

    def test_ramfuc_mask_data_outside_mask_and_forced_dll_reset(self):
        s=API.Script({0x10:0x100,0x20:0})
        s.mask(0x10,0x100,0x200,'ramfuc.h:121')
        s.mask(0x20,1,0,'ramfuc.h:121')
        s.mask(0x20,1,0,'ramfuc.h:121',force=True)
        self.assertEqual([(o['emitted'],o['result']) for o in s.ops],[(True,0x200),(False,0),(True,0)])


if __name__ == '__main__':unittest.main()
