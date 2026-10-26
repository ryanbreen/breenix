"""Display-memory scoring requires complete physical read-back and retained reservations."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
SERIAL=runpy.run_path(str(ROOT/'tests/gt710_report_test.py'))['SERIAL']

def digest(start,size,seed,phase):
    return hashlib.sha256(b''.join(struct.pack('<I',(((a//4)*0x9e3779b1)^seed^(0xffffffff if phase else 0))&0xffffffff)
                                  for a in range(start,start+size,4))).hexdigest()

# Independent live-board fixture: preferred 512x512, plus a different second DTD.
edid=bytearray(128)
edid[:8]=bytes([0,255,255,255,255,255,255,0])
edid[18:21]=bytes([1,4,128])
edid[54:72]=bytes.fromhex('302a0080200080201010110000000000001e')
edid[72:90]=bytes.fromhex('302a0080400080201010110000000000001e')
edid[127]=-sum(edid)&255
board=dict(schema=1,phase='board',bdf='01:00.0',boot0=0x106000a1,
           chipset=0x106,revision=0xa1,post=2,rom_bytes=4096,rom_sha256='a'*64,
           bit_version=0x100,dcb_version=0x40,i2c_version=0x41,connector_version=0x40,
           pll_version=0x40,dcb_entry=0x01000102,dcb_conf=0x30,i2c_entry=2,
           connector=0,connector_type=0x30,drive=2,sor=0,sor_state=0x501,head=0,
           head_total=0x033f0540,head_clock=65000000,ddc='live',edid=edid.hex(),
           edid_sha256=hashlib.sha256(edid).hexdigest(),modes=API['edid_timings'](edid))
SERIAL += 'GT710_NATIVE '+json.dumps(board)+'\n'

def evidence(width=512,height=512):
    seed=0x12345678
    pitch=(width*4+255)&~255
    scanout=(pitch*height+4095)&~4095
    objects=[]; p=128<<20
    for name,size,align in [('scanout-a',scanout,65536),('scanout-b',scanout,65536),
                            ('evo',4096,4096),('notifier',4096,4096),
                            ('instance',65536,65536),('dma',4096,4096)]:
        start=(p+4096+align-1)&-align
        objects.append(dict(name=name,start=start,size=size,align=align,
                            sha256=[digest(start,size,seed,phase) for phase in range(2)]))
        p=start+size+4096
    return dict(schema=1,phase='memory',bdf='01:00.0',total=1<<30,aperture=128<<20,
                pitch=pitch,width=width,height=height,seed=seed,
                reserved=[[0,128<<20],[0x3f000000,(1<<30)-0x3f000000]],
                references=[[0x3f000000,65536]],
                registers=[[0x22438,1],[0x22554,0],[0x11020c,1024],[0x1704,0],
                           [0x1714,0],[0x610010,0x003f0009]]+
                          [[0x610494+i*16,0] for i in range(9)]+[[0x610490+i*16,0] for i in range(9)]+[[0x2243c,1]],
                pramin_before=0x1234,pramin_after=0x1234,
                protected_bytes=1310720+65536,protected_before='a'*64,protected_after='a'*64,
                guard_bytes=12*4096,guard_before='b'*64,guard_after='b'*64,
                objects=objects,verified_bytes=2*sum(o['size'] for o in objects))

class MemoryCapture(unittest.TestCase):
    def test_full_carveout_and_negative_controls(self):
        e=evidence()
        def check(e):
            API['check_case']('memory','carveout',SERIAL+'GT710_MEMORY '+json.dumps(e)+'\n')
        check(e)
        # These are complete, internally consistent allocation reports; only
        # their mismatch with the attached sink makes them invalid.
        for width,height in [(1,1),(512,256),(1024,512)]:
            with self.subTest(geometry=(width,height)),self.assertRaisesRegex(ValueError,'preferred timing'):
                check(evidence(width,height))
        low=copy.deepcopy(e)
        low['references']=[[1<<20,65536]]
        low['registers'][5][1]=0x1009
        low['reserved'][1]=[(1<<30)-(1<<20),1<<20]
        check(low)
        idle=copy.deepcopy(e)
        idle['registers'][5][1]=0
        idle['references']=[]
        idle['protected_bytes']=1310720
        idle['reserved'][1]=[(1<<30)-(1<<20),1<<20]
        check(idle)
        idle['registers'][15][1]=0x13
        with self.assertRaises(ValueError):check(idle)
        opaque=copy.deepcopy(e)
        opaque['registers'][5][1]=1
        opaque['registers'][3][1]=0x7ff00
        opaque['registers'][4][1]=0x40000000
        opaque['references']=[]
        opaque['protected_bytes']=1310720
        opaque['reserved'][1]=[(1<<30)-(1<<20),1<<20]
        check(opaque)
        for i in range(9):
            for control in (0x13,0x10,0x80000000):
                bad=copy.deepcopy(opaque)
                bad['registers'][15+i][1]=control
                with self.assertRaises(ValueError):check(bad)
        for control in (2,3,5,0x1001):
            bad=copy.deepcopy(opaque)
            bad['registers'][5][1]=control
            with self.assertRaises(ValueError):check(bad)
        retained=copy.deepcopy(opaque)
        retained['references']=[[0,4096]]
        retained['protected_bytes']+=4096
        for i in range(9):
            retained['registers'][6+i][1]=1
            retained['registers'][15+i][1]=0x0d0500a9 if i==0 else 0x40000088
        check(retained)
        bad=copy.deepcopy(retained);bad['references']=[]
        with self.assertRaises(ValueError):check(bad)
        for i in range(9):
            for control in (0x10,0x13,0x80000000):
                bad=copy.deepcopy(retained);bad['registers'][15+i][1]=control
                with self.assertRaises(ValueError):check(bad)
        multi=copy.deepcopy(e)
        multi['registers'][-1][1]=2
        multi['registers'].append([0x11120c,1024])
        multi['total']=2<<30
        multi['reserved'][1][1]=(2<<30)-0x3f000000
        check(multi)
        for field,value in [('pramin_after',0),('protected_after','c'*64),('guard_after','c'*64),
                            ('verified_bytes',4096),('bdf','02:00.0'),('total',128<<20),
                            ('protected_bytes',4096),('guard_bytes',4096),('seed',0)]:
            bad=dict(e,**{field:value})
            with self.subTest(field=field),self.assertRaises(ValueError):check(bad)
        for mutate in [lambda b:b['objects'][1].update(start=b['objects'][0]['start']),
                       lambda b:b['objects'][0].update(start=4096),
                       lambda b:b['objects'][0].update(size=4096),
                       lambda b:b['objects'][0]['sha256'].__setitem__(1,b['objects'][0]['sha256'][0]),
                       lambda b:b['objects'].pop(),
                       lambda b:b['reserved'].pop(),
                       lambda b:b['registers'][2].__setitem__(1,512)]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)
        with self.assertRaises(ValueError):API['check_case']('memory','carveout',SERIAL)
        serial=SERIAL+'GT710_MEMORY '+json.dumps(e)+'\n'
        with self.assertRaises(ValueError):API['check_case']('memory','carveout',serial+serial)

if __name__=='__main__':unittest.main()
