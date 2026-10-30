"""Runtime G20 evidence binds requested resources to measured EVO state."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))

def row(mode):
    w,h = [(1280,800),(2560,1600)][mode]
    return dict(schema=1, sequence=mode+1, mode=mode, resource=70+mode,
                requested=0x1000000+mode*0x1000000, head=0, notifier=1,
                active=[0x10000+mode*0x10000,w|h<<16,0x1000000+w*4,0xcf00,1],
                armed=[0x10000+mode*0x10000,w|h<<16,0x1000000+w*4,0xcf00,1],
                viewport_active=[0,0,0,w|h<<16,0,1280|800<<16,1280|800<<16,1280|800<<16],
                viewport_armed=[0,0,0,w|h<<16,0,1280|800<<16,1280|800<<16,1280|800<<16],
                raster=[359|40<<16,1639|1064<<16],firmware_writes=0)

def text(rows):
    return ''.join('GT710_DISPLAY_V1 '+json.dumps(r,separators=(',',':'))+'\n' for r in rows)

class DisplayEvidence(unittest.TestCase):
    def test_both_modes_and_independent_latched_address_are_required(self):
        self.assertTrue('runtime_display' in API,'runtime display proof is absent')
        check=API['runtime_display'];good=[row(0),row(1)]
        check(text(good),(1280,1024))
        for mutate in [lambda r:r.update(requested=r['requested']+4096),
                       lambda r:r['active'].__setitem__(0,r['active'][0]+16),
                       lambda r:r['armed'].__setitem__(0,r['armed'][0]+16),
                       lambda r:r['viewport_active'].__setitem__(3,1280|800<<16),
                       lambda r:r['viewport_armed'].__setitem__(5,1280|1024<<16),
                       lambda r:r['raster'].__setitem__(1,1639|840<<16),
                       lambda r:r.update(firmware_writes=1),lambda r:r.update(notifier=0)]:
            bad=copy.deepcopy(good);mutate(bad[1])
            with self.subTest(mutation=mutate),self.assertRaises(ValueError):check(text(bad),(1280,1024))
        with self.assertRaises(ValueError):check(text(good[:1]),(1280,1024))
        with self.assertRaises(ValueError):check(text(good),(1920,1080))

    def test_upload_digest_anchors_are_required(self):
        records=[row(0),row(1)];body=text(records).encode()
        anchors=''.join(f'GT710_DISPLAY_BULK sequence={r["sequence"]} bytes={len(line.encode())} sha256={hashlib.sha256(line.encode()).hexdigest()}\n'
                        for r,line in zip(records,text(records).splitlines()))
        serial=anchors+f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'gt710-pixels.rle').write_bytes(body);(p/'serial.txt').write_text(serial)
            self.assertIn('GT710_DISPLAY_V1',API['capture_serial'](p))
            for bad in [serial.replace('GT710_DISPLAY_BULK','MISSING',1),serial.replace('sha256=','sha256=0',1),serial+text(records)]:
                (p/'serial.txt').write_text(bad)
                with self.assertRaises(ValueError):API['capture_serial'](p)

if __name__=='__main__':unittest.main()
