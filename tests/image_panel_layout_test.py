"""Use affected fixture inventories and production tile geometry, including packing."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts/metal'))
import intent_manifest as manifest
spec=importlib.util.spec_from_file_location('verify',ROOT/'scripts/screens/verify_expect.py')
v=importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)
CASES=('bmp/orientation','bmp/roundtrip','png-decode/independent-fixtures','png-encode/sizes')


class PanelLayout(unittest.TestCase):
    def test_labels_and_tiles_are_disjoint_before_and_after_packing(self):
        source=(ROOT/'userspace/programs/src/suite_breengel/images.rs').read_text()
        code='''mod checks {
            pub struct Image {pub width:u32,pub height:u32}
            pub struct JudgedImage {pub actual:Image,pub expected:Image,pub label:String}
        }
        '''+source[source.index('const PANEL_PIXELS'):source.index('fn handoff_images')]
        code+='\nfn main(){\n'
        inventories=[]
        for case in CASES:
            fixtures,_=v.panel_fixtures(case)
            inventories.append(next(r for r in manifest.inventory() if r['suite']=='breengel' and r['case']==case))
            items=','.join('checks::JudgedImage {actual:checks::Image {width:%d,height:%d},expected:checks::Image {width:%d,height:%d},label:%s.into()}'%(f['width'],f['height'],f['width'],f['height'],json.dumps(f['label'])) for f in fixtures)
            code+='let plan=plan_panel(&['+items+']); let p=&plan.pages[0]; println!("{} {} {}",plan.target,p.width,p.height);\n'
            code+='''for t in &p.tiles {
                println!("{} {} {} {} {} {} {} {} {} {}",t.x,t.y,t.width,t.height,t.picture_w,t.picture_h,t.scale,t.divisor,t.title.len(),t.title_height);
                for (line,title) in t.title.iter().enumerate() {
                    println!("{} {} {} {}",t.x+PANEL_PAD,t.y+PANEL_PAD+line*PANEL_LINE,title.len()*6-1,7);
                }
                for (row,title) in ["actual","reference","difference"].iter().enumerate() {
                    let label=if t.divisor>1 {format!("{title} shown at 1/{}",t.divisor)} else {(*title).into()};
                    println!("{} {} {} {}",t.x+PANEL_PAD,t.y+PANEL_PAD+t.title_height+row*(16+t.picture_h+2+PANEL_GAP),label.len()*6-1,7);
                }
            }\n'''
        code+='}'
        with tempfile.TemporaryDirectory() as tmp:
            src=Path(tmp)/'check.rs';exe=Path(tmp)/'check';src.write_text(code)
            subprocess.run(['taskpolicy','-b','rustc','--edition=2021','-Dwarnings',str(src),'-o',str(exe)],check=True,capture_output=True,text=True)
            lines=iter(subprocess.check_output([str(exe)],text=True).splitlines())
        def contains(outer,inner):
            x,y,w,h=outer;a,b,c,d=inner
            return x<=a and y<=b and a+c<=x+w and b+d<=y+h
        def overlap(a,b):
            x,y,w,h=a;u,v,s,t=b
            return x<u+s and u<x+w and y<v+t and v<y+h
        for inventory in inventories:
            _,width,height=map(int,next(lines).split())
            fixtures,_=v.panel_fixtures(inventory['case'])
            tiles=[];labels=[]
            for _ in fixtures:
                x,y,w,h,pw,ph,scale,divisor,title_count,title_height=map(int,next(lines).split())
                tile=(x,y,w,h);tiles.append(tile)
                for _ in range(title_count+3):
                    label=tuple(map(int,next(lines).split()));labels.append(label)
                    self.assertTrue(contains(tile,label),(inventory['case'],tile,label))
            for i,tile in enumerate(tiles):
                self.assertFalse(any(overlap(tile,other) for other in tiles[i+1:]),inventory['case'])
            for i,label in enumerate(labels):
                self.assertFalse(any(overlap(label,other) for other in labels[i+1:]),inventory['case'])
            self.assertEqual((width,height),inventory['size'])
            for label in labels:
                mapped=[]
                for page in manifest.row_plans(inventory):
                    for dst,src in page['tiles']:
                        if contains(src,label):
                            scale=dst[2]//src[2]
                            mapped.append((dst[0]+(label[0]-src[0])*scale,dst[1]+(label[1]-src[1])*scale,label[2]*scale,label[3]*scale))
                self.assertEqual(len(mapped),1,(inventory['case'],label,'split label'))
                self.assertTrue(contains((0,0,2560,1600),mapped[0]))
        self.assertEqual(list(lines),[])


if __name__=='__main__':unittest.main()
