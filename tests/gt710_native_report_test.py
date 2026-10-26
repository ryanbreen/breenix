"""Native acceptance needs executed display state and complete device pixels."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
M=runpy.run_path(str(ROOT/'tests/gt710_memory_report_test.py'))

def capture(change=None, short=False):
    memory=M['evidence'](); t=M['board']['modes'][0]; seed=12345
    start=memory['objects'][0]['start']; pitch=memory['pitch']; h=t['height']; w=t['width']
    active={0x414:t['vtotal']<<16|t['htotal'],
            0x418:(t['vsync_end']-t['vsync_start']-1)<<16|t['hsync_end']-t['hsync_start']-1,
            0x41c:(t['vtotal']-t['vsync_start']-1)<<16|t['htotal']-t['hsync_start']-1,
            0x420:(t['vtotal']-t['vsync_start']+h-1)<<16|t['htotal']-t['hsync_start']+w-1,
            0x450:t['clock_khz']*1000,0x460:start>>8,0x468:h<<16|w,
            0x46c:0x1000000|(pitch>>4)<<8,0x470:0xcf00,0x474:1,0x404:0x140,0x200:0x101}
    e=dict(schema=1,phase='native',bdf='01:00.0',head=0,sor=0,protocol=1,timing=t,
           surface=start,pitch=pitch,seed=seed,active=[[r+0x640000,v] for r,v in active.items()],
           notifier_before=0,notifier_after=1,put=256,get=256,errors=0,underflow=0,
           power=1,power_status=0,raster_before=1<<16,raster_after=3<<16,
           instance=memory['objects'][4]['start'],push=memory['objects'][2]['start'],
           skipped_address=start,skipped_notifier=0,clean_notifier=1,firmware_writes=0,
           pll_register=0x614100,pll_reference=27000,pll_coefficients=0x94702,pll_fraction=0x10000000)
    data=bytearray(); rows=[]
    for y in range(h):
        runs=[]
        for x in range(0,pitch//4,64):
            count=min(64,pitch//4-x)
            value=0xff000000|((seed+y*257+(x//64)*0x10203)&0xffffff)
            runs.append(f'{count:x}:{value:08x}')
            data+=struct.pack('<I',value)*count
        rows.append('GT710_PIX '+str(y)+' '+','.join(runs)+'\n')
    e['bytes']=len(data);e['sha256']=hashlib.sha256(data).hexdigest()
    if change:change(e)
    serial=M['SERIAL']+'GT710_MEMORY '+json.dumps(memory)+'\n'
    return serial+''.join(rows[:-1] if short else rows)+'GT710_MODE '+json.dumps(e)+'\n'+f'GT710_NATIVE_CONSOLE surface={start} firmware_writes=0\n'

class NativeCapture(unittest.TestCase):
    def test_native_state_and_pixels_with_negative_controls(self):
        API['native_mode'](capture())
        mutations=[lambda e:e.update(notifier_after=0),lambda e:e.update(notifier_before=1),
                   lambda e:e.update(get=e['put']-4),lambda e:e.update(errors=1),
                   lambda e:e.update(underflow=16),lambda e:e.update(power=0),
                   lambda e:e.update(raster_after=e['raster_before']),
                   lambda e:e.update(surface=0),lambda e:e['active'][5].__setitem__(1,1),
                   lambda e:e.update(skipped_notifier=1),lambda e:e.update(skipped_address=0),
                   lambda e:e.update(clean_notifier=0),lambda e:e.update(firmware_writes=1),
                   lambda e:e.update(pll_coefficients=0),lambda e:e.update(pll_fraction=0),
                   lambda e:e.update(sha256='0'*64),lambda e:e.update(bytes=e['bytes']-4)]
        for mutate in mutations:
            with self.subTest(mutation=mutate),self.assertRaises(ValueError):
                API['native_mode'](capture(mutate))
        with self.assertRaises(ValueError):API['native_mode'](capture(short=True))
        with self.assertRaises(ValueError):API['native_mode'](capture().replace('firmware_writes=0','firmware_writes=1'))
        with self.assertRaises(ValueError):API['native_mode'](capture().replace('GT710_NATIVE_CONSOLE','MISSING_CONSOLE'))
        serial=capture().replace('40:ff003039','40:ff003038',1)
        self.assertNotEqual(serial,capture())
        with self.assertRaises(ValueError):API['native_mode'](serial)

if __name__=='__main__':unittest.main()
