"""Judge mode switches from retained device state, EDID and full pixels."""
from pathlib import Path
import runpy
import unittest
import copy
import hashlib
import json
import struct
from functools import lru_cache

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


@lru_cache(maxsize=1)
def switch_capture():
    fixture = runpy.run_path(str(ROOT / 'tests/gt710_native_report_test.py'))
    serial = fixture['capture']()
    native = json.loads(next(s[11:] for s in serial.splitlines() if s.startswith('GT710_MODE ')))
    old_board_line = next(s for s in serial.splitlines() if s.startswith('GT710_NATIVE '))
    board = json.loads(old_board_line[13:])
    raw = bytearray.fromhex(board['edid'])
    # A second progressive DTD: 256x256, 65 MHz, negative H/V sync.
    raw[72:90] = bytes.fromhex('641900801000201010101100000000000018')
    raw[127] = 0
    raw[127] = -sum(raw) & 255
    low = dict(width=256,height=256,clock_khz=65000,hsync_start=272,
               hsync_end=288,htotal=384,vsync_start=257,vsync_end=258,vtotal=288,flags=24)
    board.update(edid=raw.hex(),edid_sha256=hashlib.sha256(raw).hexdigest(),
                 modes=[native['timing'],low])
    serial = serial.replace(old_board_line,'GT710_NATIVE '+json.dumps(board))
    prior = native
    for step,t in enumerate([low,native['timing'],low]):
        pitch = (t['width']*4+255)&~255
        e = copy.deepcopy(native)
        e.update(phase='switch',step=step,timing=t,pitch=pitch,seed=12346+step,
                 before_active=prior['active'],put_before=prior['put'],put=prior['put']+320,
                 get=prior['put']+320,detached_output=0,detached_iso=0,detached_notifier=1)
        pair=lambda x,y:x|y<<16
        values={0x414:pair(t['htotal'],t['vtotal']),
                0x418:pair(t['hsync_end']-t['hsync_start']-1,t['vsync_end']-t['vsync_start']-1),
                0x41c:pair(t['htotal']-t['hsync_start']-1,t['vtotal']-t['vsync_start']-1),
                0x420:pair(t['htotal']-t['hsync_start']+t['width']-1,t['vtotal']-t['vsync_start']+t['height']-1),
                0x450:t['clock_khz']*1000,0x468:pair(t['width'],t['height']),0x46c:0x1000000|pitch,
                0x404:0x140|(8 if not t['flags']&2 else 0)|(16 if not t['flags']&4 else 0)}
        for r in e['active']:
            r[1]=values.get(r[0]-0x640000,r[1])
        if t==low:e['pll_coefficients']=0x94003
        data=bytearray()
        for y in range(t['height']):
            first=0xff000000|((e['seed']+y*(pitch//4))&0xffffff)
            serial+=f'GT710_SWITCH_PIX {step} {y} {pitch//4:x}:{first:08x}\n'
            data+=b''.join(struct.pack('<I',0xff000000|((e['seed']+y*(pitch//4)+x)&0xffffff))
                           for x in range(pitch//4))
        e.update(bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
        serial+='GT710_SWITCH '+json.dumps(e)+'\n'
        prior=e
    return serial


class SwitchingCapture(unittest.TestCase):
    def test_low_high_low_requires_each_latched_mode_and_every_device_pixel(self):
        serial=switch_capture()
        frames=API['native_switches'](serial)
        self.assertEqual(len(frames),3)
        self.assertNotEqual(frames[0][1],frames[2][1])
        lines=serial.splitlines(keepends=True)
        indices=[i for i,s in enumerate(lines) if s.startswith('GT710_SWITCH ')]
        for change in [lambda e:e.update(step=2),lambda e:e.update(timing=frames[1][0]['timing']),
                       lambda e:e['active'][0].__setitem__(1,0),
                       lambda e:e['before_active'][0].__setitem__(1,0),
                       lambda e:e.update(notifier_before=1),lambda e:e.update(notifier_after=0),
                       lambda e:e.update(detached_notifier=0),lambda e:e.update(detached_output=1),
                       lambda e:e.update(detached_iso=1),lambda e:e.update(put_before=0),
                       lambda e:e.update(get=e['put']-4),lambda e:e.update(errors=1),
                       lambda e:e.update(underflow=17),lambda e:e.update(power=0),
                       lambda e:e.update(raster_after=e['raster_before']),
                       lambda e:e.update(pll_coefficients=0x94702),
                       lambda e:e.update(surface=0),lambda e:e.update(firmware_writes=1),
                       lambda e:e.update(bytes=e['bytes']-4),lambda e:e.update(sha256='0'*64)]:
            broken=lines.copy(); e=json.loads(broken[indices[0]][13:]);change(e)
            broken[indices[0]]='GT710_SWITCH '+json.dumps(e)+'\n'
            with self.subTest(change=change),self.assertRaises(ValueError):
                API['native_switches'](''.join(broken))
        for broken in [serial.replace('GT710_SWITCH_PIX 0 0 ','MISSING 0 0 ',1),
                       serial.replace('100:ff00303a','100:ff00303b',1),
                       ''.join(s for i,s in enumerate(lines) if i!=indices[1]),
                       serial+lines[indices[2]]]:
            self.assertNotEqual(serial,broken)
            with self.assertRaises(ValueError):API['native_switches'](broken)


class AdvertisedMode(unittest.TestCase):
    def test_established_and_standard_dmt_need_the_exact_advertisement(self):
        raw = bytearray(128)
        raw[:8] = bytes([0, 255, 255, 255, 255, 255, 255, 0])
        raw[18:21] = bytes([1, 3, 128])
        expected = dict(width=1024, height=768, clock_khz=65000,
                        hsync_start=1048, hsync_end=1184, htotal=1344,
                        vsync_start=771, vsync_end=777, vtotal=806, flags=24)
        for bit, code, count in [(8, [1, 1], 1), (0, [0x61, 0x40], 1),
                                 (8, [0x61, 0x40], 1), (4, [1, 1], 0),
                                 (16, [1, 1], 0), (0, [0x61, 0x4a], 0)]:
            raw[36] = bit
            raw[38:40] = bytes(code)
            raw[127] = 0
            raw[127] = -sum(raw) & 255
            with self.subTest(bit=bit, code=code):
                self.assertEqual(API['edid_timings'](raw).count(expected), count)


if __name__ == '__main__':
    unittest.main()
