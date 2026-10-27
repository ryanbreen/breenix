"""Same-mode A-B-A needs distinct private bindings and retained full pixels."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
SWITCH = runpy.run_path(str(ROOT/'tests/gt710_switch_report_test.py'))


def capture():
    serial = SWITCH['switch_capture']()
    memory = API['display_memory'](serial)
    modes = API['native_switches'](serial)
    low, high = modes[0][0], modes[1][0]
    for mode, template in enumerate([low, high]):
        # Low flips precede preferred return; high flips follow it.
        insertion = serial.index('GT710_PREFERRED_RETURN ') if mode == 0 else serial.rindex('GT710_NATIVE_CONSOLE ')
        before = copy.deepcopy(template['active'])
        start = modes[2][0]['put'] if mode == 0 else json.loads(next(l.split(' ',1)[1] for l in serial.splitlines() if l.startswith('GT710_PREFERRED_RETURN ')))['put']
        a, b = [o['start'] for o in memory['objects'][:2]]
        skipped = dict(mode=mode,expected=b,active=before,notifier=0,put=start+16,get=start+16,
                       errors=0,firmware_writes=0)
        addition = 'GT710_FLIP_SKIPPED '+json.dumps(skipped)+'\n'
        previous = start+16
        for step,address in enumerate([a,b,a]):
            e = copy.deepcopy(template)
            e.update(phase='flip',mode=mode,step=step,surface=address,before_active=before,
                     put_before=previous,put=previous+24,get=previous+24,
                     seed=0x123450+mode*0x34567+(0x789ab if step==1 else 0),
                     notifier_before=0,notifier_after=1)
            e['active'] = copy.deepcopy(before)
            next(r for r in e['active'] if r[0]==0x640460)[1]=address>>8
            data=bytearray()
            for y in range(e['timing']['height']):
                first=0xff000000|((e['seed']+y*(e['pitch']//4))&0xffffff)
                addition+=f"GT710_FLIP_PIX {mode} {step} {y} {e['pitch']//4:x}:{first:08x}\n"
                data+=b''.join(struct.pack('<I',0xff000000|((first+x)&0xffffff)) for x in range(e['pitch']//4))
            e.update(bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
            addition+='GT710_FLIP '+json.dumps(e)+'\n'
            before=e['active'];previous=e['put']
        serial=serial[:insertion]+addition+serial[insertion:]
    return serial


class Flips(unittest.TestCase):
    def test_each_edid_mode_requires_latched_aba_full_pixels_and_clean_next_update(self):
        serial=capture()
        frames=API['native_flips'](serial)
        self.assertEqual(len(frames),6)
        for offset in [0,3]:
            self.assertEqual(frames[offset][1],frames[offset+2][1])
            self.assertNotEqual(frames[offset][1],frames[offset+1][1])
        lines=serial.splitlines(keepends=True)
        index=next(i for i,l in enumerate(lines) if l.startswith('GT710_FLIP ') and json.loads(l[11:])['step']==1)
        mutations=[lambda e:e.update(surface=frames[0][0]['surface']),
                   lambda e:e.update(step=0),lambda e:e.update(mode=1),
                   lambda e:e.update(notifier_before=1),lambda e:e.update(notifier_after=0),
                   lambda e:e.update(get=e['put']-4),lambda e:e.update(put_before=0),
                   lambda e:e['active'][5].__setitem__(1,0),
                   lambda e:e.update(before_active=[]),lambda e:e.update(timing=frames[3][0]['timing']),
                   lambda e:e.update(errors=1),lambda e:e.update(underflow=17),
                   lambda e:e.update(power=0),lambda e:e.update(power_status=0x10000000),
                   lambda e:e.update(raster_after=e['raster_before']),
                   lambda e:e.update(firmware_writes=1),lambda e:e.update(bytes=4),
                   lambda e:e.update(sha256='0'*64)]
        for change in mutations:
            broken=lines.copy();e=json.loads(broken[index][11:]);change(e)
            broken[index]='GT710_FLIP '+json.dumps(e)+'\n'
            with self.subTest(change=change),self.assertRaises(ValueError):API['native_flips'](''.join(broken))
        skipped=next(l for l in lines if l.startswith('GT710_FLIP_SKIPPED '))
        for field,value in [('notifier',1),('expected',0),('get',0),('errors',1),('active',[])]:
            e=json.loads(skipped.split(' ',1)[1]);e[field]=value
            with self.subTest(field=field),self.assertRaises(ValueError):
                API['native_flips'](serial.replace(skipped,'GT710_FLIP_SKIPPED '+json.dumps(e)+'\n'))
        pixel=next(l for l in lines if l.startswith('GT710_FLIP_PIX '))
        bad_pixel=pixel.replace(pixel.split(':')[-1][:8],'ff000000')
        for broken in [serial.replace(pixel,''),serial.replace(pixel,bad_pixel),
                       serial.replace(lines[index],''),serial+lines[index],
                       serial.replace(skipped,''),serial+'GT710_FLIP_REFUSAL runtime=timeout\n',
                       serial+'GT710_NATIVE_CONSOLE surface=0 firmware_writes=1\n']:
            with self.assertRaises(ValueError):API['native_flips'](broken)


if __name__=='__main__':unittest.main()
