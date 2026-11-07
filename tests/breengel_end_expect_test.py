"""End descriptions follow the composed surface at the pre-helper-drop boundary."""
import importlib.util
import json
from pathlib import Path
import unittest
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verify', ROOT/'scripts/screens/verify_expect.py')
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)


def scene(windows):
    image = Image.new('RGB', (2560,1600), 'black')
    for x,y,w,h,z,seed in sorted(windows, key=lambda w:w[4]):
        tile = Image.new('RGB',(w,h))
        tile.putdata([((px^py^seed)&255,(5*py+29*seed)&255,(7*px+13*seed)&255)
                      for py in range(h) for px in range(w)])
        image.paste(tile,(x,y))
    return image


class EndExpect(unittest.TestCase):
    def check_end(self, case, windows):
        category,name=case.split('/')
        doc=json.loads((ROOT/'docs/suites/breengel.json').read_text())
        entry=next(e for c in doc['categories'] if c['id']==category for e in c['cases'] if e['id']==name)
        image=scene(windows)
        expect=entry.get('expectEnd',entry['expect'])
        result=v.verify_picture(image,expect,'end',suite='breengel',case=case)
        self.assertEqual(result['verdict'],'pass',result)
        # A different end surface must fail, even outside the small windows.
        image.putpixel((2500,1500),(1,2,3))
        self.assertEqual(v.verify_picture(image,expect,'end',suite='breengel',case=case)['verdict'],'fail')

    def test_authorized(self):
        self.check_end('isolation/compositor-authorized',[(100,40,20,10,0,13),(150,40,12,8,1,14)])

    def test_many(self):
        self.check_end('composition/compose-many',[(40+60*i,30+40*i,160,120,15-i,10+i) for i in range(16)]+[(0,0,8,8,16,27)])

    def test_rate(self):
        self.check_end('composition/present-rate',[])

    def test_atomic(self):
        self.check_end('scanout/atomic-record',[])


if __name__=='__main__':unittest.main()
