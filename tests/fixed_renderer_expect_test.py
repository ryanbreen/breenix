"""U16 full-frame oracle input refuses stale sources, wrong claims and changed pixels."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest
from PIL import Image

ROOT=Path(__file__).resolve().parents[1]
api=runpy.run_path(str(ROOT/'scripts/screens/verify_expect.py'))

class FixedRendererExpect(unittest.TestCase):
    def test_complete_frame_match_and_refusals(self):
        case='panel/handoff'
        expect=next(c['expect'] for g in json.loads((ROOT/'docs/suites/breengel.json').read_text())['categories']
                    for c in g['cases'] if g['id']+'/'+c['id']==case)
        with tempfile.TemporaryDirectory() as tmp:
            directory=Path(tmp)
            pixels=bytes([31,19,12,255])*2560*1600
            frame=dict(case=case,file='panel_handoff.bgra',mode='2560x1600',expect=expect,
                       sha256=hashlib.sha256(pixels).hexdigest())
            doc=dict(version=1,sources=api['fixed_renderer_sources'](),frames=[frame])
            (directory/frame['file']).write_bytes(pixels)
            def write(): (directory/'manifest.json').write_text(json.dumps(doc))
            write()
            image=Image.frombytes('RGB',(2560,1600),pixels,'raw','BGRX')
            verify=lambda im=image,text=expect: api['verify_fixed_renderer'](im,text,case,None,directory)
            self.assertEqual(verify()['verdict'],'pass')
            bad=image.copy();bad.putpixel((1279,799),(13,19,31))
            self.assertEqual(verify(bad)['verdict'],'fail')
            with self.assertRaisesRegex(ValueError,'Expect'):verify(text=expect+' Changed claim.')
            source=next(iter(doc['sources']));doc['sources'][source]='0'*64;write()
            with self.assertRaisesRegex(ValueError,'source'):verify()
            doc['sources']=api['fixed_renderer_sources']();doc['sources'].pop(source);write()
            with self.assertRaisesRegex(ValueError,'source'):verify()

if __name__=='__main__':unittest.main()
