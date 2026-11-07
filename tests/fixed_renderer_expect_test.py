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
            doc=dict(version=1,sources=dict(api['fixed_renderer_sources']()),frames=[frame])
            (directory/frame['file']).write_bytes(pixels)
            def write(): (directory/'manifest.json').write_text(json.dumps(doc))
            write()
            image=Image.frombytes('RGB',(2560,1600),pixels,'raw','BGRX')
            verify=lambda im=image,text=expect: api['verify_fixed_renderer'](im,text,case,None,directory)
            result=verify()
            self.assertEqual(result['verdict'],'unparseable')
            self.assertEqual(result['checked_claims'][0]['verdict'],'pass')
            self.assertEqual(result['unparseable_claims'][0]['claim'],expect)
            bad=image.copy();bad.putpixel((1279,799),(13,19,31))
            self.assertEqual(verify(bad)['verdict'],'fail')
            with self.assertRaisesRegex(ValueError,'Expect'):verify(text=expect+' Changed claim.')
            source=next(iter(doc['sources']));doc['sources'][source]='0'*64;write()
            with self.assertRaisesRegex(ValueError,'source'):verify()
            doc['sources']=dict(api['fixed_renderer_sources']());doc['sources'].pop(source);write()
            with self.assertRaisesRegex(ValueError,'source'):verify()

    def test_shared_atomic_mutation_cannot_bypass_independent_formula(self):
        case='scanout/atomic-record'
        expect=next(c['expect'] for g in json.loads((ROOT/'docs/suites/breengel.json').read_text())['categories']
                    for c in g['cases'] if g['id']+'/'+c['id']==case)
        image=Image.new('RGB',(2560,1600),'black')
        for y in range(64):
            for x in range(64):
                image.putpixel((600+x,400+y),((x^y^20)&255,(5*y+29*20)&255,(7*x+13*20)&255))
        with tempfile.TemporaryDirectory() as tmp:
            directory=Path(tmp)
            def verify(im):
                # Mutate both candidate and shared renderer reference together.
                data=bytearray(im.tobytes('raw','BGRX'));data[3::4]=b'\xff'*(2560*1600)
                frame=dict(case=case,file='scanout_atomic-record.bgra',mode='2560x1600',expect=expect,
                           sha256=hashlib.sha256(data).hexdigest())
                (directory/frame['file']).write_bytes(data)
                (directory/'manifest.json').write_text(json.dumps(dict(version=1,
                    sources=api['fixed_renderer_sources'](),frames=[frame])))
                return api['verify_picture'](im,expect,case=case,suite='breengel',renderer_frames=directory)
            result=verify(image)
            self.assertEqual(result['verdict'],'unparseable')
            self.assertEqual([c['kind'] for c in result['checked_claims']],
                             ['production-fixed-renderer','seed-scene'])
            image.putpixel((600,400),(1,2,3))
            result=verify(image)
            self.assertEqual(result['checked_claims'][0]['verdict'],'pass')
            self.assertEqual(result['verdict'],'fail')
            self.assertEqual(result['failed_claims'][0]['kind'],'seed-scene')

if __name__=='__main__':unittest.main()
